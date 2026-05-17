//! `#[server]` functions for the Dioxus trash view. Mirrors the OCS
//! surface (`/ocs/v2.php/apps/files_trashbin/api/v1/...`) but with
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

/// One trashed file/folder, returned by [`list_trash`]. Mirrors the
/// OCS DTO field-for-field so the UI can render with the same shape
/// whichever surface (server-fn vs. raw OCS JSON) it ends up calling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrashEntryDto {
    pub id: i64,
    pub basename: String,
    pub suffix: String,
    pub location: String,
    pub deleted_at: i64,
    /// `"file"` or `"dir"` (mirror of `crabcloud_trash::TrashType::as_str`).
    pub r#type: String,
}

/// Return shape from [`restore_trash`]. `path` is the user-relative
/// destination the entry was actually restored to (may include
/// ` (restored)` on collision — see the trash service docs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoredDto {
    pub path: String,
}

/// `POST /api/files/trash/list` — list every trash entry owned by the
/// calling user. Empty list when the bin is empty.
#[server(endpoint = "api/files/trash/list", prefix = "")]
pub async fn list_trash() -> Result<Vec<TrashEntryDto>, ServerFnError> {
    let (state, uid) = super::require_user().await?;
    let rows = state
        .trash
        .list(uid.as_str())
        .await
        .map_err(map_trash_err)?;
    Ok(rows
        .into_iter()
        .map(|e| TrashEntryDto {
            id: e.id,
            basename: e.basename,
            suffix: e.suffix,
            location: e.location,
            deleted_at: e.deleted_at,
            r#type: e.r#type.as_str().to_string(),
        })
        .collect())
}

/// `POST /api/files/trash/restore` — restore the trash row identified
/// by `id` back to its original location (or to a name-suffixed
/// sibling on collision). Returns the final user-relative path.
#[server(endpoint = "api/files/trash/restore", prefix = "")]
pub async fn restore_trash(id: i64) -> Result<RestoredDto, ServerFnError> {
    let (state, uid) = super::require_user().await?;
    state
        .trash
        .restore(uid.as_str(), id, None)
        .await
        .map(|r| RestoredDto { path: r.path })
        .map_err(map_trash_err)
}

/// `POST /api/files/trash/purge` — permanently delete one trash row.
#[server(endpoint = "api/files/trash/purge", prefix = "")]
pub async fn purge_trash(id: i64) -> Result<(), ServerFnError> {
    let (state, uid) = super::require_user().await?;
    state
        .trash
        .purge(uid.as_str(), id)
        .await
        .map_err(map_trash_err)
}

/// `POST /api/files/trash/empty` — permanently delete every trash row
/// owned by the calling user. Returns the count purged so the UI can
/// surface a confirmation toast.
#[server(endpoint = "api/files/trash/empty", prefix = "")]
pub async fn empty_trash() -> Result<u64, ServerFnError> {
    let (state, uid) = super::require_user().await?;
    state
        .trash
        .purge_all(uid.as_str())
        .await
        .map_err(map_trash_err)
}

/// Map the trash service's typed errors to the string-bodied
/// `ServerFnError` the dx client surface understands. Distinct
/// strings so the UI / tests can pattern-match without parsing
/// `Display` output of an opaque variant.
#[cfg(feature = "server")]
fn map_trash_err(err: crabcloud_trash::TrashError) -> ServerFnError {
    use crabcloud_trash::TrashError;
    match err {
        TrashError::NotFound => ServerFnError::new("not_found"),
        TrashError::WrongUser => ServerFnError::new("forbidden"),
        TrashError::RestoreCollision => ServerFnError::new("restore_collision"),
        TrashError::SourceMissing => ServerFnError::new("source_missing"),
        other => {
            tracing::error!(error = %other, "trash server fn: unhandled TrashError");
            ServerFnError::new(format!("trash: {other}"))
        }
    }
}
