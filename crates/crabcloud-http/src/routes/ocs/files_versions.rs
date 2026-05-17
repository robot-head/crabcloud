//! OCS endpoints for file versions.
//!
//! Nextcloud spelling: `/ocs/v2.php/apps/files_versions/api/v1/...`.
//! All endpoints require the authed user; the row filter is always the
//! authed uid (no `{uid}` segment — third-party OCS clients don't carry
//! one on this surface).
//!
//! * `GET    /versions/{fileid}`        — list versions of `fileid`
//! * `POST   /restore/{version_id}`     — restore one version (snapshot-then-replace)
//! * `DELETE /version/{version_id}`     — hard-delete one version row + file
//!
//! Envelope helpers live in [`super::envelope`] and are shared with
//! the other OCS modules so the wire shape stays single-sourced.

use super::envelope::ocs_envelope;
use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_ocs::Format;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use crabcloud_versions::{VersionEntry, VersionsError};
use serde::Serialize;
use serde_json::Value;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/versions/{fileid}", get(list_handler))
        .route("/restore/{version_id}", post(restore_handler))
        .route("/version/{version_id}", delete(purge_handler))
}

// --- error mapping ---------------------------------------------------------

/// Map `VersionsError` → OCS envelope (HTTP code, message). Mirrors the
/// trash module's policy: NotFound→404, WrongUser→403, others log
/// fail-fast at `tracing::error!` and surface 500.
fn from_versions_error(err: VersionsError, fmt: Format) -> Response {
    let (code, msg) = match err {
        VersionsError::NotFound | VersionsError::SourceMissing => (404, "not found".to_string()),
        VersionsError::WrongUser => (403, "forbidden".to_string()),
        other => {
            tracing::error!(error = %other, "versions OCS handler: unhandled VersionsError");
            (500, other.to_string())
        }
    };
    ocs_envelope(code, &msg, Value::Null, fmt)
}

// --- wire DTO --------------------------------------------------------------

/// Per-entry shape returned in the OCS `data` array. Mirrors the user-
/// facing fields of `crabcloud_versions::VersionEntry`; `storage_id`,
/// `user`, and `path` stay server-internal.
#[derive(Serialize)]
struct VersionDto {
    id: i64,
    fileid: i64,
    version_mtime: i64,
    size: i64,
}

impl From<VersionEntry> for VersionDto {
    fn from(e: VersionEntry) -> Self {
        Self {
            id: e.id,
            fileid: e.fileid,
            version_mtime: e.version_mtime,
            size: e.size,
        }
    }
}

// --- handlers --------------------------------------------------------------

async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(fileid): Path<i64>,
) -> Response {
    match state.versions.list_for(ctx.user_id.as_str(), fileid).await {
        Ok(rows) => {
            let dtos: Vec<Value> = rows
                .into_iter()
                .map(VersionDto::from)
                .map(|d| serde_json::to_value(d).expect("VersionDto serialises"))
                .collect();
            ocs_envelope(200, "OK", Value::Array(dtos), fmt.0)
        }
        Err(e) => from_versions_error(e, fmt.0),
    }
}

async fn restore_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(version_id): Path<i64>,
) -> Response {
    // Resolve the current file size from the filecache so
    // `Versions::restore` can decide whether to pre-snapshot the
    // current bytes. Best-effort: missing row → 0 (skip snapshot),
    // mirroring `routes::versions::copy::restore`.
    let entry = match state.versions.get_by_id(version_id).await {
        Ok(e) => e,
        Err(e) => return from_versions_error(e, fmt.0),
    };
    if entry.user != ctx.user_id.as_str() {
        return from_versions_error(VersionsError::WrongUser, fmt.0);
    }
    let current_size = current_filecache_size(&state, ctx.user_id.as_str(), &entry.path).await;
    let now = chrono::Utc::now().timestamp();
    let cfg = &state.config;
    match state
        .versions
        .restore(
            ctx.user_id.as_str(),
            version_id,
            current_size,
            now,
            cfg.versions_min_interval_secs as i64,
            cfg.versions_max_bytes,
        )
        .await
    {
        Ok(()) => ocs_envelope(200, "OK", Value::Null, fmt.0),
        Err(e) => from_versions_error(e, fmt.0),
    }
}

async fn purge_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(version_id): Path<i64>,
) -> Response {
    match state
        .versions
        .delete(ctx.user_id.as_str(), version_id)
        .await
    {
        Ok(()) => ocs_envelope(200, "OK", Value::Null, fmt.0),
        Err(e) => from_versions_error(e, fmt.0),
    }
}

/// Look up the current size of the owner-relative `path` under `uid`'s
/// home storage. Returns 0 on any lookup failure (missing row, error)
/// — `Versions::restore` interprets 0 as "skip the pre-snapshot",
/// matching the policy applied by the DAV COPY restore in
/// `routes::versions::copy::current_filecache_size`.
async fn current_filecache_size(state: &AppState, uid: &str, path: &str) -> i64 {
    let uid_obj = match UserId::new(uid) {
        Ok(u) => u,
        Err(_) => return 0,
    };
    let storage = match state.storage_factory.home_storage(&uid_obj).await {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let storage_id = storage.id().to_string();
    let rel = path.trim_start_matches('/');
    let sp = match StoragePath::new(rel.to_string()) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    match state.filecache.lookup(&storage_id, &sp).await {
        Ok(Some(row)) => row.size as i64,
        _ => 0,
    }
}
