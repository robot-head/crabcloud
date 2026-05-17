//! `#[server]` functions for the Dioxus file-versions UI. Mirrors the
//! OCS surface (`/ocs/v2.php/apps/files_versions/api/v1/...`) but with
//! typed inputs / outputs the UI can call directly without round-
//! tripping through the OCS JSON envelope.
//!
//! Auth: the request runs through the production `AuthLayer`, so the
//! `AuthContext` extension is always present for authenticated callers
//! and the [`super::require_user`] helper hands the body a
//! `(AppState, UserId)` pair. Unauthenticated callers fall through
//! anonymous and the helper short-circuits with `unauthorized`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One version row, returned by [`list_versions`]. Subset of the OCS
/// DTO — `fileid` / `storage_id` / `user` / `path` stay server-internal
/// because the UI already knows which file's versions it asked for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionDto {
    pub id: i64,
    pub version_mtime: i64,
    pub size: i64,
}

/// `POST /api/files/versions/list` — list every version row owned by
/// the calling user for `fileid`. Empty list when no versions exist
/// (also when the fileid doesn't belong to the caller — `list_for`
/// filters by uid).
#[server(endpoint = "api/files/versions/list", prefix = "")]
pub async fn list_versions(fileid: i64) -> Result<Vec<VersionDto>, ServerFnError> {
    let (state, uid) = super::require_user().await?;
    let rows = state
        .versions
        .list_for(uid.as_str(), fileid)
        .await
        .map_err(map_versions_err)?;
    Ok(rows
        .into_iter()
        .map(|e| VersionDto {
            id: e.id,
            version_mtime: e.version_mtime,
            size: e.size,
        })
        .collect())
}

/// `POST /api/files/versions/restore` — restore the version identified
/// by `version_id` (snapshot-then-replace). The current bytes are
/// snapshotted to a NEW version row first so the restore is lossless.
#[server(endpoint = "api/files/versions/restore", prefix = "")]
pub async fn restore_version(version_id: i64) -> Result<(), ServerFnError> {
    let (state, uid) = super::require_user().await?;
    // Resolve owner + current size before the restore call, same shape
    // as the OCS POST and DAV COPY paths. Reject other-user rows here
    // so the error surface stays distinguishable from a NotFound.
    let entry = state
        .versions
        .get_by_id(version_id)
        .await
        .map_err(map_versions_err)?;
    if entry.user != uid.as_str() {
        return Err(map_versions_err(
            crabcloud_versions::VersionsError::WrongUser,
        ));
    }
    let current_size = current_filecache_size(&state, uid.as_str(), &entry.path).await;
    let now = chrono::Utc::now().timestamp();
    let cfg = &state.config;
    state
        .versions
        .restore(
            uid.as_str(),
            version_id,
            current_size,
            now,
            cfg.versions_min_interval_secs as i64,
            cfg.versions_max_bytes,
        )
        .await
        .map_err(map_versions_err)
}

/// `POST /api/files/versions/delete` — hard-delete the version row +
/// its on-disk file. Validates ownership via the underlying service.
#[server(endpoint = "api/files/versions/delete", prefix = "")]
pub async fn delete_version(version_id: i64) -> Result<(), ServerFnError> {
    let (state, uid) = super::require_user().await?;
    state
        .versions
        .delete(uid.as_str(), version_id)
        .await
        .map_err(map_versions_err)
}

/// Look up the current size of the owner-relative `path` under `uid`'s
/// home storage. Returns 0 on any lookup failure (missing row, error)
/// — `Versions::restore` interprets 0 as "skip the pre-snapshot",
/// matching the OCS POST and DAV COPY policy.
#[cfg(feature = "server")]
async fn current_filecache_size(state: &crabcloud_core::AppState, uid: &str, path: &str) -> i64 {
    use crabcloud_storage::StoragePath;
    use crabcloud_users::UserId;
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

/// Map the versions service's typed errors to the string-bodied
/// `ServerFnError` the dx client surface understands. Distinct strings
/// so the UI / tests can pattern-match without parsing `Display`
/// output of an opaque variant. Mirrors the trash module's policy:
/// NotFound→404, WrongUser→forbidden, fail-fast on other variants
/// with `tracing::error!`.
#[cfg(feature = "server")]
fn map_versions_err(err: crabcloud_versions::VersionsError) -> ServerFnError {
    use crabcloud_versions::VersionsError;
    match err {
        VersionsError::NotFound | VersionsError::SourceMissing => ServerFnError::new("not_found"),
        VersionsError::WrongUser => ServerFnError::new("forbidden"),
        other => {
            tracing::error!(error = %other, "versions server fn: unhandled VersionsError");
            ServerFnError::new(format!("versions: {other}"))
        }
    }
}
