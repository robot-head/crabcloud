//! MOVE + COPY handlers. Both honor `Destination:` and `Overwrite:` headers.
//! If `Overwrite: T` (default) and destination exists, the handler DELETEs
//! it first before calling `View::rename`/`copy` (which error on existing
//! destination in 4a's Storage trait).

use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_destination_files, parse_overwrite};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn move_(
    state: AppState,
    uid: &UserId,
    from: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let (to_user, to_path_raw) = parse_destination_files(headers)?;
    if to_user != uid.as_str() {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&to_path_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let to = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?;
    let overwrite = parse_overwrite(headers)?;
    let view = state.view_for(uid).await?;

    let dest_existed = view.stat(&to).await.is_ok();
    if dest_existed && !overwrite {
        return Err(DavError::PreconditionFailed);
    }
    if dest_existed {
        view.delete(&to).await?;
    }
    view.rename(from, &to).await?;
    // Keep custom-prop rows synchronized with the file tree. PropertyStore
    // owns its own per-userid key space, so the rewrite is scoped to the
    // moved subtree.
    let store = crabcloud_filecache::PropertyStore::new(state.filecache.pool().clone());
    let from_sp = from.as_str().trim_start_matches('/');
    let to_sp = to.as_str().trim_start_matches('/');
    store.rename_path(uid, from_sp, to_sp).await?;
    Ok((
        if dest_existed {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        },
        "",
    )
        .into_response())
}

pub async fn copy(
    state: AppState,
    uid: &UserId,
    from: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let (to_user, to_path_raw) = parse_destination_files(headers)?;
    if to_user != uid.as_str() {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&to_path_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let to = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?;
    let overwrite = parse_overwrite(headers)?;
    let view = state.view_for(uid).await?;

    let dest_existed = view.stat(&to).await.is_ok();
    if dest_existed && !overwrite {
        return Err(DavError::PreconditionFailed);
    }
    if dest_existed {
        view.delete(&to).await?;
    }
    view.copy(from, &to).await?;
    // Duplicate the source's property subtree under the new location.
    let store = crabcloud_filecache::PropertyStore::new(state.filecache.pool().clone());
    let from_sp = from.as_str().trim_start_matches('/');
    let to_sp = to.as_str().trim_start_matches('/');
    store.copy_path(uid, from_sp, to_sp).await?;
    Ok((
        if dest_existed {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        },
        "",
    )
        .into_response())
}
