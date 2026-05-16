//! `GET /api/files/preview/{fileid}?size=N` — authenticated thumbnail
//! download. Resolves the source via filecache, dispatches by mime,
//! returns the cached preview (or generates one).
//!
//! The handler is intentionally fileid-keyed (not path-keyed): clients
//! coming from `/api/files/list` already carry stable fileids, and a
//! cross-storage move that updates the file's path doesn't invalidate
//! the preview cache (which is keyed on `(storage_id, fileid, size,
//! source_etag)`). Mismatched-storage requests collapse to 404 so the
//! endpoint can't be used as a fileid-existence oracle.

use crate::extractors::auth::AuthenticatedUser;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use crabcloud_core::AppState;
use crabcloud_fs::path::UserPath;
use crabcloud_preview::{provider_for_mime, PreviewError};
use serde::Deserialize;
use std::sync::Arc;

/// Build the authed preview sub-router. Mounted under the global
/// `AuthLayer` in `build_router`, so authenticated traffic carries the
/// `AuthContext` extension our `AuthenticatedUser` extractor reads
/// (and unauthenticated traffic 401s via that extractor's rejection).
pub fn router() -> Router<AppState> {
    Router::new().route("/api/files/preview/{fileid}", get(handler))
}

#[derive(Deserialize)]
struct SizeQuery {
    #[serde(default = "default_size")]
    size: u32,
}

fn default_size() -> u32 {
    64
}

async fn handler(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path(fileid): Path<i64>,
    Query(q): Query<SizeQuery>,
    headers: HeaderMap,
) -> Response {
    let uid = match crabcloud_users::UserId::new(authed.user_id.as_str()) {
        Ok(u) => u,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    // Look up the filecache row by fileid. A missing row is 404.
    let row = match state.filecache.lookup_by_id(fileid).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    // Authorize: the row's storage_id must match a storage the user has
    // a mount on. Cheapest correct check: build the View and confirm at
    // least one mount surfaces that storage. Cross-user requests collapse
    // to 404 (not 403) so the endpoint can't be used as an oracle to
    // probe whether a fileid exists.
    let view = match state.view_for(&uid).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    if !view
        .mounts()
        .iter()
        .any(|m| m.storage.id() == row.storage_id)
    {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    // Provider dispatch by source mime. Unsupported sources are 415.
    let provider = match provider_for_mime(row.mimetype.as_str()) {
        Some(p) => p,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
    };

    // Snap size to the ladder (also catches > 1024 → 400).
    let snapped = match crabcloud_preview::round_up_to_ladder(q.size) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "").into_response(),
    };

    // Conditional GET: composite ETag is `"<source_etag>-<snapped>"`. The
    // quotes are part of the value (RFC 7232 entity-tag syntax), matched
    // verbatim against `If-None-Match`.
    let composite_etag = format!("\"{}-{}\"", row.etag.as_str(), snapped);
    if let Some(req_etag) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
    {
        if req_etag == composite_etag {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }

    // Materialise a user-facing path that `View::read` will resolve to
    // the storage we just authorized. For home mounts this is `/<row.path>`;
    // for share mounts the recipient sees the path through the mount's
    // path_prefix.
    let source_path = match user_path_for_row(&view, &row) {
        Some(p) => p,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    let view = Arc::new(view);
    let view_for_read = view.clone();
    let row_size = row.size;
    let render_result = state
        .preview
        .get_or_render(
            &row.storage_id,
            row.fileid,
            q.size,
            row.etag.as_str(),
            provider,
            || async move {
                use tokio::io::AsyncReadExt;
                let mut reader = view_for_read
                    .read(&source_path)
                    .await
                    .map_err(PreviewError::from)?;
                let mut buf = Vec::with_capacity(row_size as usize);
                reader
                    .read_to_end(&mut buf)
                    .await
                    .map_err(PreviewError::from)?;
                Ok(buf)
            },
        )
        .await;

    let (cache_path, _size) = match render_result {
        Ok(t) => t,
        Err(PreviewError::SizeOutOfRange(_)) => {
            return (StatusCode::BAD_REQUEST, "").into_response()
        }
        Err(PreviewError::Unsupported(_)) => {
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response()
        }
        Err(PreviewError::SourceTooLarge { .. }) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "").into_response()
        }
        Err(PreviewError::SourceNotFound(_)) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, fileid, "preview render failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    serve_cache_file(cache_path, composite_etag).await
}

/// Convert a filecache row into a user-facing path for `View::read`.
///
/// The row's `path` is the storage-relative path; we must surface it as
/// the user-facing path through whichever mount surfaces that storage.
/// For the home mount this is `/<row.path>` directly. For share mounts
/// (`SharedSubrootStorage`) the recipient sees `/<mount_prefix>/...` —
/// we subtract the inner-storage owner prefix and then prepend the
/// mount's `path_prefix`. Returns `None` if no mount matches or the
/// suffix subtraction fails.
fn user_path_for_row(
    view: &crabcloud_fs::View,
    row: &crabcloud_filecache::FilecacheRow,
) -> Option<UserPath> {
    for mount in view.mounts() {
        if mount.storage.id() != row.storage_id {
            continue;
        }
        if let Some((_, owner_path)) = mount.storage.inner_storage() {
            // Share-mount: row.path is the owner-side path. Strip the
            // owner prefix to recover the recipient-visible suffix.
            let owner = owner_path.as_str();
            let row_str = row.path.as_str();
            let suffix = if owner.is_empty() {
                row_str.to_string()
            } else if row_str == owner {
                String::new()
            } else if let Some(stripped) = row_str.strip_prefix(&format!("{owner}/")) {
                stripped.to_string()
            } else {
                // Not under this share's owner prefix — try the next
                // matching mount (e.g. a different share rooted in a
                // different subtree of the same owner storage).
                continue;
            };
            let candidate = if mount.path_prefix.is_root() {
                if suffix.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{suffix}")
                }
            } else if suffix.is_empty() {
                format!("/{}", mount.path_prefix.as_str())
            } else {
                format!("/{}/{}", mount.path_prefix.as_str(), suffix)
            };
            return UserPath::new(candidate).ok();
        }
        // Home mount: row.path IS the storage-relative path under `/`.
        let candidate = if row.path.is_root() {
            "/".to_string()
        } else {
            format!("/{}", row.path.as_str())
        };
        return UserPath::new(candidate).ok();
    }
    None
}

/// Stream a cache file as the HTTP response body. Shared between the
/// authed handler and (in Batch C) the public-link handler.
pub(crate) async fn serve_cache_file(path: std::path::PathBuf, etag: String) -> Response {
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    if let Ok(v) = HeaderValue::from_str(&meta.len().to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    (StatusCode::OK, headers, body).into_response()
}
