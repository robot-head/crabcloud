//! OCS endpoints for the trash bin.
//!
//! Nextcloud spelling: `/ocs/v2.php/apps/files_trashbin/api/v1/...`.
//! All endpoints require the authed user; the row filter is always the
//! authed uid (no `{uid}` segment — third-party OCS clients don't carry
//! one on this surface).
//!
//! * `GET    /trashbin`              — list entries
//! * `POST   /restore/{id}`          — restore one entry
//! * `DELETE /trash/{id}`            — purge one entry
//! * `DELETE /trash`                 — empty the bin
//!
//! Envelope helpers mirror `files_sharing.rs` exactly (`ocs_envelope` ->
//! `{ ocs: { meta, data } }` via `crabcloud_ocs::render`).

use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_ocs::{render, Format, OcsResponse, OcsStatus, OcsVersion};
use crabcloud_trash::{TrashEntry, TrashError};
use serde::Serialize;
use serde_json::Value;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/trashbin", get(list_handler))
        .route("/restore/{id}", post(restore_handler))
        .route("/trash/{id}", delete(purge_handler))
        .route("/trash", delete(empty_handler))
}

// --- envelope helpers (verbatim from files_sharing.rs) ---------------------

fn http_status_from(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn ocs_status_for_http(code: u16) -> OcsStatus {
    match code {
        200 => OcsStatus::Ok,
        201 => OcsStatus::Created,
        400 => OcsStatus::BadRequest,
        401 => OcsStatus::Unauthorized,
        403 => OcsStatus::Forbidden,
        404 => OcsStatus::NotFound,
        409 => OcsStatus::UnknownError,
        _ => OcsStatus::UnknownError,
    }
}

fn ocs_envelope(code: u16, message: &str, data: Value, fmt: Format) -> Response {
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

fn from_trash_error(err: TrashError, fmt: Format) -> Response {
    let (code, msg) = match err {
        TrashError::NotFound => (404, "not found".to_string()),
        TrashError::WrongUser => (403, "forbidden".to_string()),
        TrashError::RestoreCollision => (409, "restore collision".to_string()),
        TrashError::SourceMissing => (404, "source missing".to_string()),
        other => (500, other.to_string()),
    };
    ocs_envelope(code, &msg, Value::Null, fmt)
}

// --- wire DTO --------------------------------------------------------------

/// Per-entry shape returned in the OCS `data` array. Mirrors the
/// fields exposed by `crabcloud_trash::TrashEntry` minus the
/// `user` column (the authed uid is implicit) and `fileid_legacy`
/// (an internal pointer the wire surface has no use for).
#[derive(Serialize)]
struct TrashEntryDto {
    id: i64,
    basename: String,
    suffix: String,
    location: String,
    deleted_at: i64,
    #[serde(rename = "type")]
    kind: String,
}

impl From<TrashEntry> for TrashEntryDto {
    fn from(e: TrashEntry) -> Self {
        Self {
            id: e.id,
            basename: e.basename,
            suffix: e.suffix,
            location: e.location,
            deleted_at: e.deleted_at,
            kind: e.r#type.as_str().to_string(),
        }
    }
}

// --- handlers --------------------------------------------------------------

async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Response {
    match state.trash.list(ctx.user_id.as_str()).await {
        Ok(rows) => {
            let dtos: Vec<Value> = rows
                .into_iter()
                .map(TrashEntryDto::from)
                .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                .collect();
            ocs_envelope(200, "OK", Value::Array(dtos), fmt.0)
        }
        Err(e) => from_trash_error(e, fmt.0),
    }
}

async fn restore_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(id): Path<i64>,
) -> Response {
    match state.trash.restore(ctx.user_id.as_str(), id, None).await {
        Ok(r) => ocs_envelope(
            200,
            "OK",
            serde_json::json!({ "path": r.path }),
            fmt.0,
        ),
        Err(e) => from_trash_error(e, fmt.0),
    }
}

async fn purge_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(id): Path<i64>,
) -> Response {
    match state.trash.purge(ctx.user_id.as_str(), id).await {
        Ok(()) => ocs_envelope(200, "OK", Value::Null, fmt.0),
        Err(e) => from_trash_error(e, fmt.0),
    }
}

async fn empty_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Response {
    match state.trash.purge_all(ctx.user_id.as_str()).await {
        Ok(n) => ocs_envelope(
            200,
            "OK",
            serde_json::json!({ "purged": n }),
            fmt.0,
        ),
        Err(e) => from_trash_error(e, fmt.0),
    }
}
