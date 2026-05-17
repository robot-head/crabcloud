//! DAV `/dav/versions/{uid}/{fileid}/...` surface.
//!
//! Mounted via `Router::nest("/dav/versions", ...)` and
//! `Router::nest("/remote.php/dav/versions", ...)` from
//! `router::build_router`.
//!
//! Inside this namespace:
//!
//! ```text
//! PROPFIND /{uid}/{fileid}/                  - list versions of fileid
//! PROPFIND /{uid}/{fileid}/{version_mtime}   - single version detail
//! GET      /{uid}/{fileid}/{version_mtime}   - stream version bytes
//! COPY     /{uid}/{fileid}/{version_mtime}   - restore (Destination required)
//! *        anything else                     - 405 with Allow: header
//! ```
//!
//! The `{fileid}` segment is the same DB-allocated id used everywhere
//! else (filecache, OCS); the `{version_mtime}` segment is the unix-secs
//! mtime that suffixes the on-disk version file (`<path>.v<mtime>`).
//! Both are integers — the per-handler parsers reject non-numeric
//! segments with 404.
//!
//! Wire shape mirrors Nextcloud so desktop / KIO clients work without
//! translation.

mod copy;
mod get;
mod propfind;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use crabcloud_core::AppState;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::DavError;

pub(super) use copy::restore;
pub(super) use get::download;
pub(super) use propfind::{entry as propfind_entry, root as propfind_root};

/// Allow header listing the versions surface methods. Versions don't
/// accept writes (PUT/MKCOL/DELETE/MOVE) — those return 405 below.
/// DELETE on a version goes through the OCS surface in Batch C; the
/// DAV surface is read+restore-only to match Nextcloud's spelling.
const ALLOW_HEADER: &str = "OPTIONS, PROPFIND, GET, COPY";

/// Shared `VersionsError` → `DavError` mapping used by every handler in
/// this module. `DuplicateSnapshot` is technically unreachable from the
/// DAV surface (only `snapshot_if_needed` returns it and the surface
/// never calls that directly), but keeping it in the shared mapping is
/// harmless.
pub(super) fn versions_err(e: crabcloud_versions::VersionsError) -> DavError {
    use crabcloud_versions::VersionsError::*;
    match e {
        NotFound | SourceMissing => DavError::NotFound,
        WrongUser => DavError::Forbidden,
        DuplicateSnapshot => DavError::Conflict,
        Io(err) => DavError::Internal(format!("versions io: {err}")),
        Db(err) => DavError::Internal(format!("versions db: {err}")),
    }
}

/// Build the versions router. All routes are auth-gated by the outer
/// `AuthLayer` (mounted alongside `dav_router` in `build_router`).
pub fn versions_router() -> Router<AppState> {
    Router::new()
        // /{uid}/{fileid}                — root collection
        .route("/{uid}/{fileid}", any(dispatch_root))
        .route("/{uid}/{fileid}/", any(dispatch_root))
        // /{uid}/{fileid}/{version_mtime} — single version entry
        .route("/{uid}/{fileid}/{version_mtime}", any(dispatch_entry))
}

async fn dispatch_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((uid, fileid)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    dispatch_root_inner(state, authed, uid, fileid, method, headers)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_root_inner(
    state: AppState,
    authed: AuthenticatedUser,
    uid: String,
    fileid: String,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, DavError> {
    if uid != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let fileid = parse_fileid(&fileid)?;
    match method {
        Method::OPTIONS => Ok(capability_response()),
        m if m.as_str() == "PROPFIND" => propfind_root(&state, &uid, fileid, &headers).await,
        _ => Ok(method_not_allowed()),
    }
}

async fn dispatch_entry(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((uid, fileid, version_mtime)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    dispatch_entry_inner(state, authed, uid, fileid, version_mtime, method, headers)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_entry_inner(
    state: AppState,
    authed: AuthenticatedUser,
    uid: String,
    fileid: String,
    version_mtime: String,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, DavError> {
    if uid != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let fileid = parse_fileid(&fileid)?;
    let version_mtime = parse_version_mtime(&version_mtime)?;
    match method {
        Method::OPTIONS => Ok(capability_response()),
        m if m.as_str() == "PROPFIND" => {
            propfind_entry(&state, &uid, fileid, version_mtime, &headers).await
        }
        Method::GET => download(&state, &uid, fileid, version_mtime).await,
        m if m.as_str() == "COPY" => restore(&state, &uid, fileid, version_mtime, &headers).await,
        _ => Ok(method_not_allowed()),
    }
}

fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, HeaderValue::from_static(ALLOW_HEADER))],
        "",
    )
        .into_response()
}

fn capability_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::ALLOW, HeaderValue::from_static(ALLOW_HEADER)),
            (
                header::HeaderName::from_static("dav"),
                HeaderValue::from_static("1, 2, 3"),
            ),
            (
                header::HeaderName::from_static("ms-author-via"),
                HeaderValue::from_static("DAV"),
            ),
        ],
        "",
    )
        .into_response()
}

/// Parse the `{fileid}` path segment as a non-negative `i64`. Rejects
/// negative or non-numeric values with 404 (matches Nextcloud — bad
/// segments look "not found" to the client rather than "bad request"
/// because DAV path segments are part of the resource identity).
pub(super) fn parse_fileid(s: &str) -> Result<i64, DavError> {
    let n: i64 = s.parse().map_err(|_| DavError::NotFound)?;
    if n < 0 {
        return Err(DavError::NotFound);
    }
    Ok(n)
}

/// Parse the `{version_mtime}` path segment. Same shape as
/// `parse_fileid` — both are non-negative unix-second integers.
pub(super) fn parse_version_mtime(s: &str) -> Result<i64, DavError> {
    let n: i64 = s.parse().map_err(|_| DavError::NotFound)?;
    if n < 0 {
        return Err(DavError::NotFound);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fileid_accepts_positive_int() {
        assert_eq!(parse_fileid("42").unwrap(), 42);
        assert_eq!(parse_fileid("0").unwrap(), 0);
    }

    #[test]
    fn parse_fileid_rejects_non_numeric() {
        assert!(matches!(parse_fileid("abc"), Err(DavError::NotFound)));
        assert!(matches!(parse_fileid(""), Err(DavError::NotFound)));
    }

    #[test]
    fn parse_fileid_rejects_negative() {
        assert!(matches!(parse_fileid("-1"), Err(DavError::NotFound)));
    }

    #[test]
    fn parse_version_mtime_accepts_positive_int() {
        assert_eq!(parse_version_mtime("1716000000").unwrap(), 1_716_000_000);
    }
}
