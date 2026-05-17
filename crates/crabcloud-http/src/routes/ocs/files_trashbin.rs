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
//! Envelope helpers live in [`super::envelope`] and are shared with
//! `files_sharing.rs` so the OCS wire shape stays single-sourced.

use super::envelope::ocs_envelope;
use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_ocs::Format;
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

// --- error mapping ---------------------------------------------------------

fn from_trash_error(err: TrashError, fmt: Format) -> Response {
    let (code, msg) = match err {
        TrashError::NotFound => (404, "not found".to_string()),
        TrashError::WrongUser => (403, "forbidden".to_string()),
        TrashError::RestoreCollision => (409, "restore collision".to_string()),
        TrashError::SourceMissing => (404, "source missing".to_string()),
        other => {
            tracing::error!(error = %other, "trash OCS handler: unhandled TrashError");
            (500, other.to_string())
        }
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
                .map(|d| serde_json::to_value(d).expect("TrashEntryDto serialises"))
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
        Ok(r) => ocs_envelope(200, "OK", serde_json::json!({ "path": r.path }), fmt.0),
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
        Ok(n) => ocs_envelope(200, "OK", serde_json::json!({ "purged": n }), fmt.0),
        Err(e) => from_trash_error(e, fmt.0),
    }
}
