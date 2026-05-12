//! LOCK + UNLOCK handlers + lock_check helper for use by mutation methods.
//!
//! SP5 ships exclusive write locks only. Acquisition and release route
//! through [`crabcloud_filecache::LockStore`]; lock state is keyed by
//! `"files/{uid}/{path}"` (root is `"files/{uid}"`). `lock_check` is the
//! shared gate every mutation handler (`PUT`, `MKCOL`, `DELETE`, `MOVE`,
//! `COPY`, `PROPPATCH`) calls before touching state: it 423s when the
//! resource itself or any depth-infinity ancestor is locked and none of
//! the submitted `If:` tokens match.

use crabcloud_core::AppState;
use crabcloud_filecache::LockStore;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{
    parse_depth, parse_if_tokens, parse_lock_token, parse_timeout, Depth,
};
use crate::routes::dav::xml::{multistatus, write_leaf, write_propstat, write_response};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Compose the `oc_filelocks.key` value for a (uid, user_path) pair.
/// For the user's root (`/`) this collapses to `files/{uid}` to keep the
/// row representable; otherwise it's `files/{uid}/<path-without-leading-slash>`.
fn lock_key(uid: &UserId, user_path: &UserPath) -> String {
    let p = user_path.as_str().trim_start_matches('/');
    if p.is_empty() {
        format!("files/{}", uid.as_str())
    } else {
        format!("files/{}/{}", uid.as_str(), p)
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Lock-aware mutation check. Errors with [`DavError::Locked`] if the
/// resource itself OR any ancestor with `depth = "infinity"` is locked
/// AND none of the `If:`-header-submitted tokens match the stored lock.
///
/// Ancestors locked with `depth = "0"` do NOT block descendants — depth-0
/// locks are direct-resource-only per RFC 4918 §6.2.
pub async fn lock_check(
    locks: &LockStore,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<()> {
    let submitted = parse_if_tokens(headers);
    // Self check.
    let self_key = lock_key(uid, user_path);
    if let Some(lock) = locks.current(&self_key).await? {
        if !submitted.iter().any(|t| t == &lock.token) {
            return Err(DavError::Locked);
        }
    }
    // Ancestor check (only depth=infinity blocks).
    let mut parent = user_path.parent();
    while let Some(p) = parent {
        let pkey = lock_key(uid, &p);
        if let Some(lock) = locks.current(&pkey).await? {
            if lock.depth == "infinity" && !submitted.iter().any(|t| t == &lock.token) {
                return Err(DavError::Locked);
            }
        }
        if p.is_root() {
            break;
        }
        parent = p.parent();
    }
    Ok(())
}

/// Handle a `LOCK` request (RFC 4918 §9.10). Acquires an exclusive lock on
/// the resource. If the resource is already locked and the request doesn't
/// supply a matching token in `If:`, responds with `423 Locked`.
///
/// Body XML (`<d:lockinfo>`) is captured opaquely as the `owner` field;
/// SP5 doesn't parse it (the client only needs the same blob echoed back
/// later in PROPFIND `<d:lockdiscovery>` — which is also out of scope here).
pub async fn acquire(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.stat(user_path).await?;
    let key = lock_key(uid, user_path);
    let locks = LockStore::new(state.filecache.pool().clone());

    // If already locked AND no matching token → 423.
    let submitted = parse_if_tokens(headers);
    if let Some(lock) = locks.current(&key).await? {
        if !submitted.iter().any(|t| t == &lock.token) {
            return Err(DavError::Locked);
        }
    }

    let depth = parse_depth(headers, Depth::Zero)?;
    let depth_str = match depth {
        Depth::Zero => "0",
        Depth::One => "0", // LOCK Depth: 1 is unusual; collapse to 0 per SP5 plan.
        Depth::Infinity => "infinity",
    };
    let ttl_secs = parse_timeout(headers);
    let ttl = now_unix() + ttl_secs;
    let token = format!("urn:uuid:{}", uuid::Uuid::new_v4());

    // Owner XML (best-effort: pass body through; not parsed).
    let owner = String::from_utf8(
        axum::body::to_bytes(body, 64 * 1024)
            .await
            .map_err(|e| DavError::BadRequest(format!("lock body: {e}")))?
            .to_vec(),
    )
    .ok();

    locks
        .acquire(&key, &token, "exclusive", depth_str, owner.as_deref(), ttl)
        .await?;

    // Compose response body (lockdiscovery).
    let prefix = "/remote.php/dav/files";
    let href = format!("{}/{}{}", prefix, uid.as_str(), user_path.as_str());
    let body = multistatus(|w| {
        write_response(w, &href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| {
                write_leaf(w, "d:locktype", "")?;
                write_leaf(w, "d:lockscope", "")?;
                write_leaf(w, "d:depth", depth_str)?;
                write_leaf(w, "d:timeout", &format!("Second-{}", ttl_secs))?;
                write_leaf(w, "d:locktoken", &token)?;
                write_leaf(w, "d:lockroot", &href)?;
                Ok(())
            })
        })
    });

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/xml; charset=utf-8"),
            ),
            (
                header::HeaderName::from_static("lock-token"),
                HeaderValue::from_str(&format!("<{}>", token)).unwrap(),
            ),
        ],
        Body::from(body),
    )
        .into_response())
}

/// Handle an `UNLOCK` request (RFC 4918 §9.11). Removes the lock keyed by
/// `(resource, Lock-Token)`. Returns `204 No Content` on success and
/// `409 Conflict` if the header is missing or no matching row exists.
pub async fn release(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let token = parse_lock_token(headers).ok_or(DavError::Conflict)?;
    let key = lock_key(uid, user_path);
    let locks = LockStore::new(state.filecache.pool().clone());
    if locks.release(&key, &token).await? {
        Ok((StatusCode::NO_CONTENT, "").into_response())
    } else {
        Err(DavError::Conflict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_key_root() {
        let uid = UserId::new("alice").unwrap();
        let p = UserPath::new("/").unwrap();
        assert_eq!(lock_key(&uid, &p), "files/alice");
    }

    #[test]
    fn lock_key_with_path() {
        let uid = UserId::new("alice").unwrap();
        let p = UserPath::new("/photos/cat.jpg").unwrap();
        assert_eq!(lock_key(&uid, &p), "files/alice/photos/cat.jpg");
    }
}
