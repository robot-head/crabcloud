//! Browser-facing public-link HTTP surface mounted under `/s/{token}`.
//!
//! Routes (all anonymous; the `public_link_auth` middleware attaches a
//! `PublicLinkAuthContext` extension before any of these handlers run):
//!
//! - `POST /s/{token}/unlock` — verify the link password, mint an
//!   `pl_<token>` cookie, redirect back to the viewer.
//! - `GET  /s/{token}/download/{*path}` — stream a file body. Honors Range.
//!   Refuses when the password gate is still in force or the link lacks
//!   the read bit (file-drop links).
//! - `GET  /s/{token}/preview/{*path}?size=N` — anonymous thumbnail. Same
//!   provider/cache backend as the authed preview endpoint, gated by the
//!   read bit + password state.
//! - `POST /s/{token}/upload/{filename}` — file-drop upload. Filename is
//!   sanitized, collisions are resolved by suffixing ` (N)`, the request
//!   body is streamed straight into the owner's home storage at the
//!   linked subroot.
//! - `GET  /s/{token}/zip/{*path}` — stream a deflate-where-helpful zip
//!   archive of a directory inside the linked subtree. Trailing-slash form
//!   zips the link root and names the archive `<owner-folder>.zip` (the
//!   basename of `owner_path`, falling back to the token). Refuses when the
//!   password gate is still in force or the link lacks the read bit.
//!
//! The page itself (`/s/{token}` and `/s/{token}/*path`) is rendered by
//! the dx fullstack SSR pipeline and is NOT covered by this nested
//! router; its handlers live in `crabcloud-app`'s server function set
//! and resolve the share row themselves from the token query parameter.
//!
//! Per-handler bodies live in sibling files (`unlock.rs`, `download.rs`,
//! `preview.rs`, `upload.rs`, `zip.rs`) and call back into the shared
//! helpers in this module via `pub(super)` visibility.
//!
//! `clippy::result_large_err` is allowed at the module level: the natural
//! error type for these helpers is `axum::response::Response`, which clippy
//! flags as "large" because it carries a `Body`. Boxing it would buy nothing
//! — every `Err` here is on the slow path and is immediately consumed by
//! `into_response()` upstream.
#![allow(clippy::result_large_err)]

mod download;
mod preview;
mod unlock;
mod upload;
mod zip;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use crabcloud_core::AppState;
use crabcloud_fs::{MountResolver, PublicLinkMountResolver, UserPath, View};
use crabcloud_publiclinks::PublicLinkAuthContext;
use crabcloud_sharing::SharePermissions;
use std::sync::Arc;

use download::download_handler;
use preview::preview_handler;
use unlock::unlock_handler;
use upload::upload_handler;
use zip::{zip_handler, zip_handler_root};

/// Unlock-cookie lifetime in seconds. Matches the SP8 design (one hour).
pub(super) const UNLOCK_COOKIE_TTL_SECS: i64 = 3600;

/// Build the public-link router. The CALLER mounts this under `/s` via
/// `Router::nest` and layers `public_link_auth(AuthSurface::Browser)` on
/// top — see `crate::router::build_router`. Without that layer the
/// handlers will panic when they try to extract `PublicLinkAuthContext`.
/// Routes are written nest-relative (no `/s/` prefix) so axum strips the
/// mount prefix before the auth middleware sees the path.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{token}/unlock", post(unlock_handler))
        .route("/{token}/download/{*path}", get(download_handler))
        .route("/{token}/preview/{*path}", get(preview_handler))
        .route("/{token}/upload/{filename}", post(upload_handler))
        .route("/{token}/zip/", get(zip_handler_root))
        .route("/{token}/zip/{*path}", get(zip_handler))
}

/// Build a per-request `View` from a `PublicLinkAuthContext`. Constructs
/// the mounts directly via `PublicLinkMountResolver` rather than going
/// through `AppState::view_for`, which would use the recipient-side
/// `ShareMountResolver` (wrong here — anonymous traffic has no recipient).
pub(super) async fn build_view(
    state: &AppState,
    ctx: &PublicLinkAuthContext,
) -> Result<View, Response> {
    let perms = SharePermissions::from_wire(ctx.permissions);
    let resolver = Arc::new(PublicLinkMountResolver::new(
        state.storage_factory.clone(),
        ctx.owner_uid.clone(),
        ctx.owner_path.clone(),
        perms,
    ));
    let mounts = resolver
        .mounts_for(&ctx.owner_uid)
        .await
        .map_err(fs_err_to_response)?;
    Ok(View::new(
        ctx.owner_uid.clone(),
        mounts,
        state.filecache.clone(),
        state.storage_sink.clone(),
        state.trash.clone(),
        crabcloud_fs::VersionsHooks {
            versions: state.versions.clone(),
            min_interval_secs: state.config.versions_min_interval_secs as i64,
            max_bytes: state.config.versions_max_bytes,
        },
    ))
}

pub(super) fn decoded_user_path(raw: &str) -> Result<UserPath, Response> {
    // axum's `Path<String>` extractor already percent-decoded the captured
    // segment; decoding again here would mangle paths containing a literal
    // `%`. Use the captured value verbatim.
    UserPath::new(format!("/{raw}"))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid path: {e}")).into_response())
}

/// Split a filename into `(stem, Some(extension))` for the last `.`-segment,
/// or `(name, None)` for files without an extension. Dotfiles (`.hidden`) are
/// treated as having no extension (the leading dot is part of the stem).
pub(super) fn split_ext(name: &str) -> (&str, Option<&str>) {
    // Find the last `.` that isn't the very first character.
    let bytes = name.as_bytes();
    for (i, b) in bytes.iter().enumerate().rev() {
        if *b == b'.' && i > 0 {
            return (&name[..i], Some(&name[i + 1..]));
        }
    }
    (name, None)
}

/// True iff the filename is safe to use as a single-segment path. Rejects:
/// empty, contains `/`, `\`, `\0`, control chars, starts with `..`. Matches
/// the SP8 design §11 — sanitization happens before any storage interaction
/// so a path like `../../../etc/passwd` never reaches the filesystem.
pub(super) fn is_safe_filename(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with("..") {
        return false;
    }
    !s.chars()
        .any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
}

/// Best-effort client IP extraction. The trusted-proxy layer rewrites
/// `X-Forwarded-For` upstream, so the first value is the client we want
/// to rate-limit against. When the header is absent (direct connect), we
/// fall back to a constant — better than minting "unknown" buckets per
/// request because abuse from a single IP would all share that bucket and
/// the limiter would still throttle correctly.
pub(super) fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "direct".to_string())
}

/// Map a `FsError` to a `Response` for the public-link surface. We use plain
/// text bodies (the surface is browser-facing and the dx page does its own
/// error rendering; XML wouldn't help here).
pub(super) fn fs_err_to_response(err: crabcloud_fs::FsError) -> Response {
    use crabcloud_filecache::FileCacheError;
    use crabcloud_fs::FsError;
    use crabcloud_storage::StorageError;
    match err {
        FsError::NotFound => (StatusCode::NOT_FOUND, "").into_response(),
        FsError::InvalidPath(m) => (StatusCode::BAD_REQUEST, m).into_response(),
        FsError::CrossMount => (StatusCode::BAD_GATEWAY, "").into_response(),
        FsError::MountNotFound => (StatusCode::NOT_FOUND, "").into_response(),
        FsError::Storage(StorageError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
        FsError::Storage(StorageError::PermissionDenied) => {
            (StatusCode::FORBIDDEN, "").into_response()
        }
        FsError::Storage(StorageError::AlreadyExists) => (StatusCode::CONFLICT, "").into_response(),
        FsError::Storage(StorageError::NotEmpty) => (StatusCode::CONFLICT, "").into_response(),
        FsError::Storage(StorageError::InvalidPath(m)) => {
            (StatusCode::BAD_REQUEST, m).into_response()
        }
        FsError::Storage(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("storage error: {e}"),
        )
            .into_response(),
        FsError::FileCache(FileCacheError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
        FsError::FileCache(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("filecache error: {e}"),
        )
            .into_response(),
        FsError::Upload(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
        FsError::Forbidden => (StatusCode::FORBIDDEN, "").into_response(),
        FsError::Conflict => (StatusCode::CONFLICT, "").into_response(),
        FsError::Unsupported => (StatusCode::METHOD_NOT_ALLOWED, "").into_response(),
        FsError::CrossStorage => (StatusCode::BAD_GATEWAY, "").into_response(),
        FsError::Trash(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn split_ext_simple() {
        assert_eq!(split_ext("photo.jpg"), ("photo", Some("jpg")));
    }

    #[test]
    fn split_ext_multi_dot() {
        assert_eq!(split_ext("archive.tar.gz"), ("archive.tar", Some("gz")));
    }

    #[test]
    fn split_ext_dotfile_has_no_ext() {
        assert_eq!(split_ext(".bashrc"), (".bashrc", None));
    }

    #[test]
    fn split_ext_no_dot() {
        assert_eq!(split_ext("Makefile"), ("Makefile", None));
    }

    #[test]
    fn is_safe_filename_rejects_empty() {
        assert!(!is_safe_filename(""));
    }

    #[test]
    fn is_safe_filename_rejects_slash() {
        assert!(!is_safe_filename("a/b"));
        assert!(!is_safe_filename("..\\foo"));
    }

    #[test]
    fn is_safe_filename_rejects_dotdot_prefix() {
        assert!(!is_safe_filename(".."));
        assert!(!is_safe_filename("../etc/passwd"));
    }

    #[test]
    fn is_safe_filename_accepts_unicode() {
        assert!(is_safe_filename("résumé.pdf"));
    }

    #[test]
    fn is_safe_filename_rejects_null_and_controls() {
        assert!(!is_safe_filename("foo\0bar"));
        assert!(!is_safe_filename("foo\nbar"));
        assert!(!is_safe_filename("foo\tbar"));
    }

    #[test]
    fn client_ip_picks_first_xff_entry() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.5, 10.0.0.1"),
        );
        assert_eq!(client_ip(&h), "203.0.113.5");
    }

    #[test]
    fn client_ip_falls_back_when_header_absent() {
        let h = HeaderMap::new();
        assert_eq!(client_ip(&h), "direct");
    }
}
