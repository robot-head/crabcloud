//! `GET /s/{token}/zip/` and `GET /s/{token}/zip/{*path}` handlers — stream
//! a deflate-where-helpful zip archive of a directory inside the linked
//! subtree. See `super` for the surface-level docs.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use bytes::Bytes;
use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_publiclinks::PublicLinkAuthContext;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::FileKind;
use crabcloud_zip::{stream_folder, MpscBytesWriter, OverCapBody, WalkError, ZipCaps};
use tokio_stream::wrappers::ReceiverStream;

use super::{build_view, fs_err_to_response};

/// `GET /s/{token}/zip/` — zip the entire linked subtree. Splits out so the
/// trailing-slash form maps to a single-capture route (axum's `{*path}` glob
/// requires at least one character).
pub(super) async fn zip_handler_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path(token): Path<String>,
) -> Response {
    handle_public_zip(state, ctx, token, String::new()).await
}

/// `GET /s/{token}/zip/{*path}` — zip a directory inside the linked subtree.
pub(super) async fn zip_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((token, path)): Path<(String, String)>,
) -> Response {
    handle_public_zip(state, ctx, token, path).await
}

/// Mirrors `routes::files_zip::handle_zip` (authed surface) with the public-
/// link surface's two extra gates up front: password-gate and the link's read
/// bit. The basename for `Content-Disposition` falls back to the linked
/// folder's name (via `owner_path`) when the request targets the link root,
/// and finally to the token if `owner_path` itself is the home root.
async fn handle_public_zip(
    state: AppState,
    ctx: PublicLinkAuthContext,
    token: String,
    raw_path: String,
) -> Response {
    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.contains_read() {
        return (StatusCode::FORBIDDEN, "read_not_permitted").into_response();
    }

    let user_path_str = if raw_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", raw_path.trim_start_matches('/'))
    };
    let user_path = match UserPath::new(user_path_str) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };

    let owner_path_for_fallback = ctx.owner_path.clone();
    let view = match build_view(&state, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // 400 if target is a regular file, 404 if it doesn't exist. Mirror the
    // authed handler's triplicate over the three sources of "missing".
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
        Err(e) => return fs_err_to_response(e),
    }

    let caps = ZipCaps {
        max_entries: state.config.folder_zip_max_entries,
        max_bytes: state.config.folder_zip_max_bytes,
    };

    // Pre-walk so the 413 branch never has to retract a 200 once any bytes
    // have shipped. On success we discard the plan and let `stream_folder`
    // re-walk inside the spawned task (cheap relative to the zip body).
    match crabcloud_zip::walk_for_caps(&view, &user_path, &caps).await {
        Ok(_) => {
            let basename = public_zip_basename(&user_path, &owner_path_for_fallback, &token);
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
            let writer = MpscBytesWriter::new(tx);
            let view_clone = view;
            let user_path_clone = user_path;
            let caps_clone = caps;
            tokio::spawn(async move {
                if let Err(e) =
                    stream_folder(&view_clone, &user_path_clone, caps_clone, writer).await
                {
                    tracing::warn!(error = %e, "public-link zip stream failed mid-flight");
                }
            });
            public_zip_response(basename, rx)
        }
        Err(WalkError::TooLarge { count, bytes }) => {
            let body = OverCapBody::for_too_large(count, bytes, caps);
            (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response()
        }
        Err(WalkError::View(_)) => (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }
}

/// Derive the archive basename for a public-link zip. Priority:
///
/// 1. `crabcloud_zip::root_basename(user_path)` — present whenever the
///    request targets a subdirectory inside the link (`/zip/Photos/2024`).
/// 2. Basename of `owner_path` — the linked folder's own name, used when
///    the request targets the link root (`/zip/` → user_path is `/`).
/// 3. The token — last-resort fallback when both are empty (the link itself
///    targets the owner's home root, which is the only way `owner_path` is
///    bare).
fn public_zip_basename(
    user_path: &UserPath,
    owner_path: &crabcloud_storage::StoragePath,
    token: &str,
) -> String {
    let from_user = crabcloud_zip::root_basename(user_path);
    if !from_user.is_empty() {
        return from_user;
    }
    let stripped = owner_path
        .as_str()
        .trim_start_matches('/')
        .trim_end_matches('/');
    if stripped.is_empty() {
        return token.to_string();
    }
    match stripped.rsplit_once('/') {
        Some((_, last)) if !last.is_empty() => last.to_string(),
        _ => stripped.to_string(),
    }
}

fn public_zip_response(
    basename: String,
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    // RFC 6266 dual-form, matching the authed surface in `files_zip`.
    let safe_ascii: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let percent = urlencoding::encode(&basename);
    let disp = format!("attachment; filename=\"{safe_ascii}.zip\"; filename*=UTF-8''{percent}.zip");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disp).unwrap_or(HeaderValue::from_static("attachment")),
    );
    let stream = ReceiverStream::new(rx);
    let body = Body::from_stream(stream);
    (StatusCode::OK, headers, body).into_response()
}
