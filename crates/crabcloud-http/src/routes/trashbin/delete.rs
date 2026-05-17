//! DELETE handler — purges a trash entry permanently (removes both the
//! `oc_files_trash` row and the on-disk bytes under
//! `<datadir>/<uid>/files_trashbin/files/`).
//!
//! Wire shape matches Nextcloud: the URL segment after `/trash/` is the
//! suffix-encoded filename, decoded back to `(basename, suffix)` via
//! [`super::split_basename_and_suffix`] and resolved to a row through
//! `Trash::get_by_name`. Returns 204 on success, 404 if the row doesn't
//! exist, 403 if the resolved row's `user` doesn't match the authed
//! uid (the parent dispatcher already 403s on uid-mismatch in the URL,
//! so this is defense-in-depth).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;

use crate::routes::dav::error::{DavError, DavResult};

pub async fn purge(state: &AppState, uid: &str, name: &str) -> DavResult<Response> {
    let (basename, suffix) = super::split_basename_and_suffix(name).ok_or(DavError::NotFound)?;
    let entry = match state.trash.get_by_name(uid, &basename, &suffix).await {
        Ok(e) => e,
        Err(crabcloud_trash::TrashError::NotFound) => return Err(DavError::NotFound),
        Err(other) => return Err(trash_err(other)),
    };
    match state.trash.purge(uid, entry.id).await {
        Ok(()) => Ok((StatusCode::NO_CONTENT, "").into_response()),
        Err(e) => Err(trash_err(e)),
    }
}

fn trash_err(e: crabcloud_trash::TrashError) -> DavError {
    use crabcloud_trash::TrashError::*;
    match e {
        NotFound | SourceMissing => DavError::NotFound,
        WrongUser => DavError::Forbidden,
        RestoreCollision => DavError::Conflict,
        other => DavError::Internal(format!("trash: {other}")),
    }
}
