//! `GET /s/{token}/download/{*path}` handler — stream a file body. See
//! `super` for the surface-level docs.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_publiclinks::PublicLinkAuthContext;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::FileKind;
use tokio_util::io::ReaderStream;

use crate::routes::dav::headers::parse_range;

use super::{build_view, decoded_user_path, fs_err_to_response};

/// `GET /s/{token}/download/{*path}` — stream a file body. Refuses when the
/// password gate is still required or the link lacks the read bit (file-drop
/// links are upload-only).
pub(super) async fn download_handler(
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
