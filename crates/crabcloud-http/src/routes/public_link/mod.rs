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
//! - `POST /s/{token}/upload/{filename}` — file-drop upload. Filename is
//!   sanitized, collisions are resolved by suffixing ` (N)`, the request
//!   body is streamed straight into the owner's home storage at the
//!   linked subroot.
//!
//! E7 (folder zip download) is **deferred** — see the PR description.
//!
//! The page itself (`/s/{token}` and `/s/{token}/*path`) is rendered by
//! the dx fullstack SSR pipeline; those routes flow through the same
//! auth middleware but their handlers live in `crabcloud-app`'s server
//! function set, not here.
//!
//! `clippy::result_large_err` is allowed at the module level: the natural
//! error type for these helpers is `axum::response::Response`, which clippy
//! flags as "large" because it carries a `Body`. Boxing it would buy nothing
//! — every `Err` here is on the slow path and is immediately consumed by
//! `into_response()` upstream.
#![allow(clippy::result_large_err)]

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use crabcloud_core::AppState;
use crabcloud_fs::{MountResolver, PublicLinkMountResolver, UserPath, View};
use crabcloud_publiclinks::{PublicLinkAuthContext, RateLimitDecision, Token, UnlockCookie};
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::FileKind;
use futures::StreamExt as _;
use serde::Deserialize;
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::routes::dav::headers::parse_range;

/// Unlock-cookie lifetime in seconds. Matches the SP8 design (one hour).
const UNLOCK_COOKIE_TTL_SECS: i64 = 3600;

/// Build the public-link router. The CALLER is responsible for layering
/// `public_link_auth(AuthSurface::Browser)` on top — see
/// `crate::router::build_router`. Without that layer the handlers will
/// panic when they try to extract `PublicLinkAuthContext`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/s/{token}/unlock", post(unlock_handler))
        .route("/s/{token}/download/{*path}", get(download_handler))
        .route("/s/{token}/upload/{filename}", post(upload_handler))
}

/// Form body for POST /s/{token}/unlock.
#[derive(Debug, Deserialize)]
struct UnlockForm {
    password: String,
}

/// `POST /s/{token}/unlock` — verify the password and mint a `pl_<token>`
/// cookie. Intentionally NOT gated on `password_gate_required`; this is the
/// endpoint that LEAVES that state.
async fn unlock_handler(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<UnlockForm>,
) -> Response {
    // The middleware already validated the token shape (and 404'd unknown
    // tokens), so the extension exists — but the handler still does a
    // defensive parse so the same body works if it ever gets called from
    // an alternate mount point.
    let Some(_t) = Token::parse(&token) else {
        return (StatusCode::NOT_FOUND, "").into_response();
    };

    let auth = &state.publiclinks_auth;
    if let RateLimitDecision::Throttled { retry_after_secs } =
        auth.rate_limiter.check_password_attempt(&token)
    {
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
        resp.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry_after_secs.to_string())
                .unwrap_or(HeaderValue::from_static("3600")),
        );
        return resp;
    }

    let row = match auth.lookup.lookup(&token).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "unlock: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // Expired link → indistinguishable from missing.
    if let Some(exp) = row.expiration {
        if exp < chrono::Utc::now() {
            return (StatusCode::NOT_FOUND, "").into_response();
        }
    }

    let Some(stored_hash) = row.password_hash.as_deref() else {
        // Link doesn't require a password — caller is confused.
        return (StatusCode::BAD_REQUEST, "link has no password").into_response();
    };

    let hashed = crabcloud_publiclinks::HashedPassword::from_stored(stored_hash.to_string());
    if !auth.passwords.verify(&form.password, &hashed) {
        return (StatusCode::UNAUTHORIZED, "wrong password").into_response();
    }

    let exp_unix = chrono::Utc::now().timestamp() + UNLOCK_COOKIE_TTL_SECS;
    let cookie_value = UnlockCookie::sign(&auth.secret, &token, exp_unix);
    let cookie_name = UnlockCookie::cookie_name_for(&token);
    let secure_attr = if state
        .config
        .overwrite_protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
    {
        " Secure;"
    } else {
        ""
    };
    let set_cookie = format!(
        "{cookie_name}={cookie_value}; Path=/; Max-Age={ttl}; HttpOnly;{secure_attr} SameSite=Lax",
        ttl = UNLOCK_COOKIE_TTL_SECS
    );
    let redirect_to = format!("/s/{token}");
    let mut resp = (StatusCode::SEE_OTHER, "").into_response();
    {
        let h = resp.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&set_cookie) {
            h.insert(header::SET_COOKIE, v);
        }
        if let Ok(v) = HeaderValue::from_str(&redirect_to) {
            h.insert(header::LOCATION, v);
        }
    }
    resp
}

/// `GET /s/{token}/download/{*path}` — stream a file body. Refuses when the
/// password gate is still required or the link lacks the read bit (file-drop
/// links are upload-only).
async fn download_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((_token, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.contains_read() {
        return (StatusCode::FORBIDDEN, "read_not_permitted").into_response();
    }
    let user_path = match decoded_user_path(&path) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let view = match build_view(&state, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let meta = match view.stat(&user_path).await {
        Ok(m) => m,
        Err(e) => return fs_err_to_response(e),
    };
    if matches!(meta.kind, FileKind::Directory) {
        return (StatusCode::BAD_REQUEST, "is a directory").into_response();
    }

    // Reuse the DAV range parser — keeps behaviour identical to the auth'd
    // download path. Suppress the `DavError` variant by mapping to a plain
    // 416 (we don't ship XML on the public-link surface).
    let range = match parse_range(&headers, meta.size) {
        Ok(r) => r,
        Err(_) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{}", meta.size))],
                "",
            )
                .into_response();
        }
    };

    let (status, content_length, content_range, body) = match range {
        None => {
            let reader = match view.read(&user_path).await {
                Ok(r) => r,
                Err(e) => return fs_err_to_response(e),
            };
            (
                StatusCode::OK,
                meta.size,
                None,
                Body::from_stream(ReaderStream::new(reader)),
            )
        }
        Some(r) => {
            let length = r.end - r.start;
            let cr = format!("bytes {}-{}/{}", r.start, r.end - 1, meta.size);
            let reader = match view.read_range(&user_path, r).await {
                Ok(rdr) => rdr,
                Err(e) => return fs_err_to_response(e),
            };
            (
                StatusCode::PARTIAL_CONTENT,
                length,
                Some(cr),
                Body::from_stream(ReaderStream::new(reader)),
            )
        }
    };

    let last_mod = httpdate::fmt_http_date(meta.mtime);
    let etag = format!("\"{}\"", meta.etag.as_str());
    let mut resp = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::CONTENT_TYPE, meta.mimetype.as_str())
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_mod)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cr) = content_range {
        resp = resp.header(header::CONTENT_RANGE, cr);
    }
    resp.body(body)
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "").into_response())
}

/// `POST /s/{token}/upload/{filename}` — file-drop upload.
///
/// Steps:
/// 1. Refuse if the gate is in force or the link lacks the create bit.
/// 2. Sanitize the filename (`is_safe_filename`); reject with 400 otherwise.
/// 3. Resolve `filename` against an existing entry — if it exists, append
///    ` (1)`, ` (2)`, … up to ` (50)` to find a free spot. 50 collisions →
///    409 Conflict.
/// 4. Stream the request body straight into the storage via
///    `View::put_file`.
///
/// Quota is intentionally not enforced — the project has no quota service
/// yet (the design doc lists it as deferred). When the service lands a
/// `Content-Length`-aware check can slot in between (1) and (4) without
/// changing the response shape.
async fn upload_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((_token, filename)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.allows_create() {
        return (StatusCode::FORBIDDEN, "create_not_permitted").into_response();
    }
    // axum's `Path<String>` extractor already percent-decodes the captured
    // segment; decoding again here would mangle filenames containing a
    // literal `%` (a client sending `foo%2520bar.txt` to upload `foo%20bar.txt`
    // would otherwise land as `foo bar.txt`). Use the extracted value as-is.
    let decoded_name = filename.as_str();
    if !is_safe_filename(decoded_name) {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
    }

    // Per-IP rate limit (best-effort: the proxy headers layer normalises
    // X-Forwarded-For upstream, but anonymous-link upload abuse is bursty
    // so a simple per-IP counter is enough).
    let ip = client_ip(&headers);
    if let RateLimitDecision::Throttled { retry_after_secs } =
        state.publiclinks_auth.rate_limiter.check_upload(&ip)
    {
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
        resp.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry_after_secs.to_string())
                .unwrap_or(HeaderValue::from_static("60")),
        );
        return resp;
    }

    let view = match build_view(&state, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let final_name = match resolve_collision(&view, decoded_name).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let user_path = match UserPath::new(format!("/{final_name}")) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };

    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let pinned: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = Box::pin(reader);
    if let Err(e) = view.put_file(&user_path, pinned).await {
        return fs_err_to_response(e);
    }

    let body = serde_json::json!({ "name": final_name });
    (StatusCode::CREATED, axum::Json(body)).into_response()
}

/// Build a per-request `View` from a `PublicLinkAuthContext`. Constructs
/// the mounts directly via `PublicLinkMountResolver` rather than going
/// through `AppState::view_for`, which would use the recipient-side
/// `ShareMountResolver` (wrong here — anonymous traffic has no recipient).
async fn build_view(state: &AppState, ctx: &PublicLinkAuthContext) -> Result<View, Response> {
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
    ))
}

fn decoded_user_path(raw: &str) -> Result<UserPath, Response> {
    // axum's `Path<String>` extractor already percent-decoded the captured
    // segment; decoding again here would mangle paths containing a literal
    // `%`. Use the captured value verbatim.
    UserPath::new(format!("/{raw}"))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid path: {e}")).into_response())
}

/// Search for the first unused name in the same directory by appending
/// ` (1)`, ` (2)`, … to the stem. The collision search lives inside the
/// linked subroot (the wrapped view's root *is* the subroot), so we don't
/// have to track the parent path separately.
async fn resolve_collision(view: &View, name: &str) -> Result<String, Response> {
    use crabcloud_fs::FsError;
    let initial = UserPath::new(format!("/{name}"))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid path: {e}")).into_response())?;
    match view.stat(&initial).await {
        Err(FsError::NotFound) => return Ok(name.to_string()),
        Err(FsError::Storage(crabcloud_storage::StorageError::NotFound)) => {
            return Ok(name.to_string())
        }
        Err(e)
            if matches!(
                &e,
                FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound)
            ) =>
        {
            return Ok(name.to_string());
        }
        Err(e) => return Err(fs_err_to_response(e)),
        Ok(_) => {}
    }
    let (stem, ext) = split_ext(name);
    for i in 1..=50_u32 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let p = UserPath::new(format!("/{candidate}"))
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid path: {e}")).into_response())?;
        match view.stat(&p).await {
            Err(FsError::NotFound)
            | Err(FsError::Storage(crabcloud_storage::StorageError::NotFound))
            | Err(FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound)) => {
                return Ok(candidate);
            }
            Err(e) => return Err(fs_err_to_response(e)),
            Ok(_) => continue,
        }
    }
    Err((StatusCode::CONFLICT, "too many name collisions").into_response())
}

/// Split a filename into `(stem, Some(extension))` for the last `.`-segment,
/// or `(name, None)` for files without an extension. Dotfiles (`.hidden`) are
/// treated as having no extension (the leading dot is part of the stem).
fn split_ext(name: &str) -> (&str, Option<&str>) {
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
pub fn is_safe_filename(s: &str) -> bool {
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
fn client_ip(headers: &HeaderMap) -> String {
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
fn fs_err_to_response(err: crabcloud_fs::FsError) -> Response {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
