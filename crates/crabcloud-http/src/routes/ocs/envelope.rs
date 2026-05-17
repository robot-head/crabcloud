//! Shared OCS envelope helpers.
//!
//! The OCS surface (`/ocs/v2.php/...`) wraps every payload in
//! `{ ocs: { meta, data } }` (or the XML equivalent) with both an HTTP
//! status and an OCS `statuscode` on the `meta`. These three helpers are
//! used verbatim by every OCS handler module — `files_sharing`,
//! `files_trashbin`, etc. — so they live here to keep the wire shape
//! single-sourced and prevent the modules from drifting.
//!
//! * [`http_status_from`] — `u16` → `StatusCode` with a 500 fallback.
//! * [`ocs_status_for_http`] — HTTP code → Nextcloud `OcsStatus`. Codes
//!   without a dedicated `OcsStatus` (501, 5xx, anything custom) fall
//!   through to `UnknownError` (999); the HTTP code itself remains
//!   distinct on the response line so clients can still branch on it.
//! * [`ocs_envelope`] — build a fully-encoded response (status + headers
//!   + body) for the requester's negotiated [`Format`].

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_ocs::{render, Format, OcsResponse, OcsStatus, OcsVersion};
use serde_json::Value;

pub fn http_status_from(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

pub fn ocs_status_for_http(code: u16) -> OcsStatus {
    match code {
        200 => OcsStatus::Ok,
        201 => OcsStatus::Created,
        400 => OcsStatus::BadRequest,
        401 => OcsStatus::Unauthorized,
        403 => OcsStatus::Forbidden,
        // 404 → Nextcloud's NotFound (998). Everything else we don't have
        // a dedicated OcsStatus for (501, 5xx, anything custom) falls
        // through to UnknownError (999); the HTTP code itself remains
        // distinct on the response line so clients can branch on it.
        404 => OcsStatus::NotFound,
        _ => OcsStatus::UnknownError,
    }
}

/// Wrap `data` in `{ ocs: { meta, data } }` (or XML equivalent). HTTP status
/// is `code`; OCS-envelope `statuscode` mirrors it via `OcsStatus`.
pub fn ocs_envelope(code: u16, message: &str, data: Value, fmt: Format) -> Response {
    let status = ocs_status_for_http(code);
    let envelope = OcsResponse {
        status,
        message: message.to_string(),
        data,
        version: OcsVersion::V2,
    };
    let (body, ct) = render(&envelope, fmt);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    (http_status_from(code), headers, body).into_response()
}
