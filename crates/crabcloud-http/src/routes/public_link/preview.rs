//! `GET /s/{token}/preview/{*path}?size=N` — anonymous thumbnail download.
//!
//! Symmetric public counterpart to the authed
//! `GET /api/files/preview/{fileid}` endpoint (see
//! `crate::routes::files_preview`). Shares the same provider dispatch,
//! cache backend, and response builder; differs in three ways:
//!
//! 1. Defensive `password_gate_required` check first — same 403 +
//!    `password_required` body the sibling `download` / `zip` handlers
//!    emit so the dx page can render its unlock form consistently.
//! 2. Read-bit check via `SharePermissions::from_wire(ctx.permissions)` —
//!    file-drop links (create-only, bit 4) collapse to 403 +
//!    `read_not_granted`.
//! 3. The cache row is looked up by `(storage_id, storage_path)` rather
//!    than by fileid: anonymous viewers don't carry fileids, only the
//!    user-facing path inside the linked subtree.
//!
//! The View is built via `super::build_view`, which constructs mounts
//! through `PublicLinkMountResolver` rather than the recipient-side
//! `ShareMountResolver` used by `AppState::view_for`. That keeps the
//! anonymous traffic path off the recipient-share machinery.

use super::{build_view, fs_err_to_response};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_preview::{provider_for_mime, PreviewError};
use crabcloud_publiclinks::PublicLinkAuthContext;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::FileKind;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub(super) struct PreviewQuery {
    #[serde(default = "default_size")]
    size: u32,
}

fn default_size() -> u32 {
    64
}

/// `GET /s/{token}/preview/{*path}?size=N`. See the module doc for the
/// surface description.
pub(super) async fn preview_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((_token, raw_path)): Path<(String, String)>,
    Query(q): Query<PreviewQuery>,
    headers: HeaderMap,
) -> Response {
    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.contains_read() {
        return (StatusCode::FORBIDDEN, "read_not_granted").into_response();
    }

    let user_path = match UserPath::new(format!("/{}", raw_path.trim_start_matches('/'))) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };

    let view = match build_view(&state, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Stat the target — 400 if it's a directory, propagate FsError-shaped
    // 404 / etc. via the shared mapper.
    let meta = match view.stat(&user_path).await {
        Ok(m) => m,
        Err(e) => return fs_err_to_response(e),
    };
    if !matches!(meta.kind, FileKind::File) {
        return (StatusCode::BAD_REQUEST, "not a file").into_response();
    }

    // Resolve to the underlying (storage, storage_path) tuple — translates
    // share-mount wrappers through `Storage::inner_storage` so we look up
    // the OWNER's cache row, not a non-existent recipient-rooted one.
    let (cache_storage, cache_path) = match view.cache_key_for(&user_path) {
        Ok(t) => t,
        Err(e) => return fs_err_to_response(e),
    };
    let storage_id = cache_storage.id().to_string();
    let row = match state.filecache.lookup(&storage_id, &cache_path).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    let provider = match provider_for_mime(row.mimetype.as_str()) {
        Some(p) => p,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
    };

    let snapped = match crabcloud_preview::round_up_to_ladder(q.size) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "").into_response(),
    };

    // Composite ETag matches the authed surface byte-for-byte so a
    // shared in-browser cache (same `/s/...` page hitting `/api/...` via
    // the dx pipeline) revalidates without re-rendering.
    let composite_etag = format!("\"{}-{}\"", row.etag.as_str(), snapped);
    if let Some(req_etag) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
    {
        if req_etag == composite_etag {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }

    let view = Arc::new(view);
    let view_for_read = view.clone();
    let user_path_for_read = user_path.clone();
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
                    .read(&user_path_for_read)
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

    let (cache_file, _) = match render_result {
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
            tracing::warn!(error = %e, "public preview render failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    crate::routes::files_preview::serve_cache_file(cache_file, composite_etag).await
}
