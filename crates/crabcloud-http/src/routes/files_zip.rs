//! `GET /api/files/zip/{*path}` — authenticated folder zip download.
//!
//! Streams a deflate-where-helpful zip archive of the directory at
//! `/api/files/zip/<path>`. The trailing-slash form (`/api/files/zip/`)
//! zips the user's home root and names the archive `<uid>.zip`.
//!
//! Pre-flight cap enforcement runs before any byte hits the response — if
//! the folder exceeds `FileConfig::folder_zip_max_entries` or
//! `folder_zip_max_bytes`, the handler returns 413 with a JSON summary so
//! the caller learns the actual overflow values alongside the configured
//! limits. On success the response is `200 OK` with
//! `Content-Type: application/zip` and a streamed body.

use crate::auth_context::AuthContext;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use bytes::Bytes;
use crabcloud_core::AppState;
use crabcloud_fs::path::UserPath;
use crabcloud_storage::FileKind;
use crabcloud_zip::{stream_folder, MpscBytesWriter, OverCapBody, WalkError, ZipCaps};
use tokio_stream::wrappers::ReceiverStream;

/// Build the authed folder-zip sub-router. Mounted under the global
/// `AuthLayer` in `build_router`, so `Extension<AuthContext>` is present
/// for any request that reached the handlers.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/files/zip/", get(handler_root))
        .route("/api/files/zip/{*path}", get(handler))
}

async fn handler_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Response {
    handle_zip(state, ctx, String::new()).await
}

async fn handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(path): Path<String>,
) -> Response {
    handle_zip(state, ctx, path).await
}

async fn handle_zip(state: AppState, ctx: AuthContext, raw_path: String) -> Response {
    let user_path_str = if raw_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", raw_path.trim_start_matches('/'))
    };
    let user_path = match UserPath::new(user_path_str) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };
    let view = match state.view_for(&ctx.user_id).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    // 400 if path resolves to a regular file rather than a directory; 404
    // if it doesn't exist at all (either FsError::NotFound or the storage
    // backend's surfaced StorageError::NotFound).
    match view.stat(&user_path).await {
        Ok(meta) if matches!(meta.kind, FileKind::Directory) => {}
        Ok(_) => return (StatusCode::BAD_REQUEST, "not a directory").into_response(),
        Err(crabcloud_fs::FsError::NotFound) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(crabcloud_fs::FsError::Storage(crabcloud_storage::StorageError::NotFound)) => {
            return (StatusCode::NOT_FOUND, "").into_response();
        }
        Err(crabcloud_fs::FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound)) => {
            return (StatusCode::NOT_FOUND, "").into_response();
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }
    let caps = ZipCaps {
        max_entries: state.config.folder_zip_max_entries,
        max_bytes: state.config.folder_zip_max_bytes,
    };
    let archive_basename = basename_for_zip(&user_path, ctx.user_id.as_str());

    // Pre-walk so the 413 branch never has to retract a 200. On success
    // we discard the plan and let `stream_folder` re-walk inside the
    // spawned task — cheap relative to the actual zip body, and avoids
    // threading the plan through a second public entry point.
    match crabcloud_zip::walk_for_caps(&view, &user_path, &caps).await {
        Ok(_) => {
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
            let writer = MpscBytesWriter::new(tx);
            let view_clone = view;
            let user_path_clone = user_path;
            let caps_clone = caps;
            tokio::spawn(async move {
                if let Err(e) =
                    stream_folder(&view_clone, &user_path_clone, caps_clone, writer).await
                {
                    tracing::warn!(error = %e, "authed zip stream failed mid-flight");
                }
            });
            let headers = crabcloud_zip::zip_response_headers(&archive_basename);
            let stream = ReceiverStream::new(rx);
            let body = Body::from_stream(stream);
            (StatusCode::OK, headers, body).into_response()
        }
        Err(WalkError::TooLarge { count, bytes }) => {
            let body = OverCapBody::for_too_large(count, bytes, caps);
            (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response()
        }
        Err(WalkError::View(_)) => (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }
}

/// Derive the archive basename from the resolved user path. Root (`/`)
/// falls back to `fallback` (the caller passes the uid, matching the
/// Nextcloud convention `<uid>.zip` for whole-home archives).
fn basename_for_zip(user_path: &UserPath, fallback: &str) -> String {
    let trimmed = user_path
        .as_str()
        .trim_start_matches('/')
        .trim_end_matches('/');
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}
