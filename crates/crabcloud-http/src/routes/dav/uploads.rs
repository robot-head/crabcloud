//! Chunked upload route handlers per spec §11.
//!
//! Nextcloud's desktop/mobile clients upload large files in chunks via a
//! distinct route family under `/dav/uploads/{user}/{upload_id}/...`:
//!
//! - `MKCOL /dav/uploads/{user}/{upload_id}` with `Destination:` header —
//!   begins the upload. Returns `201 Created`.
//! - `PUT /dav/uploads/{user}/{upload_id}/{part_n}` — appends part `part_n`.
//!   Returns `201 Created` + the per-part `ETag`.
//! - `MOVE /dav/uploads/{user}/{upload_id}/.file` with `Destination:` header
//!   and `X-Crabcloud-Part-Tags:` JSON header — commits the upload to the
//!   destination path. Returns `201 Created` + the final ETag.
//! - `DELETE /dav/uploads/{user}/{upload_id}` — aborts the upload. Returns
//!   `204 No Content` (idempotent: also `204` on unknown id).
//!
//! The client-chosen `{upload_id}` URL segment is mapped via the in-process
//! `AppState::upload_id_map` to the server-encoded upload id returned by
//! `Uploads::begin`. Restarting the server invalidates in-flight uploads
//! (clients retry from scratch — same as Nextcloud).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use futures::StreamExt as _;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::parse_destination_files;

/// `MKCOL /dav/uploads/{user}/{upload_id}` — begin a chunked upload. The
/// final destination path is supplied via the `Destination:` header (which
/// must point under `/dav/files/{user}/...`).
pub async fn mkcol_begin(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid =
        UserId::new(&url_user).map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let (dest_user, dest_path) = parse_destination_files(&headers)?;
    if dest_user != url_user {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&dest_path)
        .map_err(|e| DavError::BadRequest(format!("invalid encoding: {e}")))?;
    let destination = UserPath::new(format!("/{decoded}"))
        .map_err(|e| DavError::BadRequest(format!("invalid dest path: {e}")))?;

    let uploads = state.uploads_for(&uid).await?;
    let handle = uploads.begin(&destination).await?;
    state.upload_id_map.insert(upload_id, handle.upload_id);
    Ok((StatusCode::CREATED, "").into_response())
}

/// `PUT /dav/uploads/{user}/{upload_id}/{part_n}` — receive one chunk.
pub async fn put_chunk(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((url_user, upload_id, part_n)): Path<(String, String, u32)>,
    body: Body,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid =
        UserId::new(&url_user).map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = state
        .upload_id_map
        .get(&upload_id)
        .ok_or(DavError::NotFound)?
        .clone();

    let uploads = state.uploads_for(&uid).await?;
    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let reader = tokio_util::io::StreamReader::new(stream);
    let pinned: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = Box::pin(reader);
    let tag = uploads.put_part(&server_id, part_n, pinned).await?;

    let etag = HeaderValue::from_str(&tag.etag)
        .map_err(|e| DavError::Internal(format!("invalid etag header: {e}")))?;
    Ok((StatusCode::CREATED, [(header::ETAG, etag)], "").into_response())
}

/// `MOVE /dav/uploads/{user}/{upload_id}/.file` — commit the chunked upload.
///
/// Requires:
/// - `Destination:` header pointing at `/dav/files/{user}/{path}` (must
///   match the destination passed to MKCOL).
/// - `X-Crabcloud-Part-Tags:` header with a JSON array of
///   `{"part_number": <u32>, "etag": "<str>"}` objects matching the
///   per-part ETags returned by PUT.
pub async fn move_commit(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid =
        UserId::new(&url_user).map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = state
        .upload_id_map
        .get(&upload_id)
        .ok_or(DavError::NotFound)?
        .clone();

    let (dest_user, dest_path) = parse_destination_files(&headers)?;
    if dest_user != url_user {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&dest_path)
        .map_err(|e| DavError::BadRequest(format!("invalid encoding: {e}")))?;
    let destination = UserPath::new(format!("/{decoded}"))
        .map_err(|e| DavError::BadRequest(format!("invalid dest path: {e}")))?;

    let tags_raw = headers
        .get("x-crabcloud-part-tags")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| DavError::BadRequest("missing X-Crabcloud-Part-Tags".into()))?;
    let tags: Vec<crabcloud_storage::PartTag> = serde_json::from_str(tags_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid part tags json: {e}")))?;

    let uploads = state.uploads_for(&uid).await?;
    let meta = uploads.commit(&server_id, &destination, tags).await?;
    state.upload_id_map.remove(&upload_id);

    let etag = HeaderValue::from_str(&format!("\"{}\"", meta.etag.as_str()))
        .map_err(|e| DavError::Internal(format!("invalid etag header: {e}")))?;
    Ok((StatusCode::CREATED, [(header::ETAG, etag)], "").into_response())
}

/// `DELETE /dav/uploads/{user}/{upload_id}` — abort an in-flight upload.
/// Idempotent: returns `204` even on unknown id.
pub async fn delete_abort(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid =
        UserId::new(&url_user).map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = match state.upload_id_map.remove(&upload_id) {
        Some((_, v)) => v,
        None => return Ok((StatusCode::NO_CONTENT, "").into_response()),
    };
    let uploads = state.uploads_for(&uid).await?;
    uploads.abort(&server_id).await?;
    Ok((StatusCode::NO_CONTENT, "").into_response())
}
