//! `POST /s/{token}/upload/{filename}` handler — file-drop upload, plus
//! filename sanitization and collision suffixing. See `super` for the
//! surface-level docs.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_fs::{UserPath, View};
use crabcloud_publiclinks::{PublicLinkAuthContext, RateLimitDecision};
use crabcloud_sharing::SharePermissions;
use futures::StreamExt as _;

use super::{build_view, client_ip, fs_err_to_response, is_safe_filename};

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
pub(super) async fn upload_handler(
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
    let (stem, ext) = super::split_ext(name);
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
