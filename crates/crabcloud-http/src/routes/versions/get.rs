//! GET handler — streams the on-disk bytes of a single version.
//!
//! Resolves `(uid, fileid, version_mtime)` to a row via
//! `Versions::list_for`, computes the version file's absolute path under
//! `<datadir>/<uid>/files_versions/<rel>.v<mtime>`, and streams it back
//! with `Content-Length` from the recorded row size. `Content-Type` is
//! `application/octet-stream` in MVP (the current file's mime isn't on
//! the row; clients already know what they're listing).

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use std::path::Path;

use crate::routes::dav::error::{DavError, DavResult};

pub async fn download(
    state: &AppState,
    uid: &str,
    fileid: i64,
    version_mtime: i64,
) -> DavResult<Response> {
    let entries = state
        .versions
        .list_for(uid, fileid)
        .await
        .map_err(super::versions_err)?;
    let entry = entries
        .into_iter()
        .find(|e| e.version_mtime == version_mtime)
        .ok_or(DavError::NotFound)?;

    let rel = entry.path.trim_start_matches('/');
    let basename = Path::new(rel)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| DavError::Internal(format!("versions: malformed path {}", entry.path)))?;
    let parent = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
    let abs = state
        .versions
        .datadir()
        .join(uid)
        .join("files_versions")
        .join(parent)
        .join(format!("{basename}.v{version_mtime}"));

    let f = match tokio::fs::File::open(&abs).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(
                error = %e,
                path = %abs.display(),
                version_id = entry.id,
                "versions GET: on-disk file missing"
            );
            // Spec §6 "Version's on-disk file missing": GET returns
            // 500 so the operator notices the row/file desync. The DB
            // row stays in place — list/delete still surface it.
            return Err(DavError::Internal(format!(
                "versions: on-disk file missing for version {}",
                entry.id
            )));
        }
    };
    let stream = tokio_util::io::ReaderStream::new(f);
    let body = Body::from_stream(stream);
    let len_header = HeaderValue::from_str(&entry.size.to_string())
        .map_err(|e| DavError::Internal(format!("versions: bad content-length: {e}")))?;
    let mime_header = HeaderValue::from_static("application/octet-stream");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime_header),
            (header::CONTENT_LENGTH, len_header),
        ],
        body,
    )
        .into_response())
}
