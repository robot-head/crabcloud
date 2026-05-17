//! DAV `/dav/trashbin/{uid}/...` surface.
//!
//! Mounted via `Router::nest("/dav/trashbin", ...)` and
//! `Router::nest("/remote.php/dav/trashbin", ...)` from `router::build_router`.
//!
//! Inside this namespace:
//!
//! ```text
//! PROPFIND /{uid}/                                 - list root
//! PROPFIND /{uid}/trash/{basename_dot_suffix}      - single entry
//! DELETE   /{uid}/trash/{basename_dot_suffix}      - purge
//! MOVE     /{uid}/trash/{basename_dot_suffix}      - restore
//! *        anything else                           - 405
//! ```
//!
//! The `{basename_dot_suffix}` segment is `<basename>.<suffix>` where
//! `suffix` looks like `d<unix_seconds>` (or `d<unix_seconds>_<n>` on a
//! sub-second collision). Matches Nextcloud's wire shape so desktop /
//! KIO clients work without translation; internally the handler splits
//! the segment via [`split_basename_and_suffix`] and looks the row up
//! via the `(user, basename, suffix)` unique index.

mod delete;
mod move_;
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

pub(super) use delete::purge;
pub(super) use move_::restore;
pub(super) use propfind::{entry as propfind_entry, root as propfind_root};

/// Allow header listing the trashbin surface methods. Trash entries
/// don't accept writes (PUT/MKCOL) or copies — those return 405 below.
const ALLOW_HEADER: &str = "OPTIONS, PROPFIND, DELETE, MOVE";

/// Build the trashbin router. All routes are auth-gated by the outer
/// `AuthLayer` (mounted alongside `dav_router` in `build_router`).
pub fn trashbin_router() -> Router<AppState> {
    Router::new()
        // /{uid}            — root, accept trailing-slash and bare forms
        .route("/{uid}", any(dispatch_root))
        .route("/{uid}/", any(dispatch_root))
        // /{uid}/trash      — collection containing entries
        .route("/{uid}/trash", any(dispatch_trash_root))
        .route("/{uid}/trash/", any(dispatch_trash_root))
        // /{uid}/trash/{name} — per-entry resource
        .route("/{uid}/trash/{name}", any(dispatch_entry))
}

async fn dispatch_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path(uid): Path<String>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    dispatch_collection(state, authed, uid, method, headers)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_trash_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path(uid): Path<String>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    dispatch_collection(state, authed, uid, method, headers)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_collection(
    state: AppState,
    authed: AuthenticatedUser,
    uid: String,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, DavError> {
    if uid != authed.user_id {
        return Err(DavError::Forbidden);
    }
    match method {
        Method::OPTIONS => Ok(capability_response()),
        m if m.as_str() == "PROPFIND" => propfind_root(&state, &uid, &headers).await,
        _ => Ok(method_not_allowed()),
    }
}

async fn dispatch_entry(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((uid, name)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    dispatch_entry_inner(state, authed, uid, name, method, headers)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_entry_inner(
    state: AppState,
    authed: AuthenticatedUser,
    uid: String,
    name: String,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, DavError> {
    if uid != authed.user_id {
        return Err(DavError::Forbidden);
    }
    match method {
        Method::OPTIONS => Ok(capability_response()),
        m if m.as_str() == "PROPFIND" => propfind_entry(&state, &uid, &name, &headers).await,
        Method::DELETE => purge(&state, &uid, &name).await,
        m if m.as_str() == "MOVE" => restore(&state, &uid, &name, &headers).await,
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

/// Split `<basename>.<suffix>` where `suffix` is `d<digits>` or
/// `d<digits>_<digits>`. Returns `None` if the input isn't a valid
/// trash-encoded filename. Walks right-to-left to find the last `.d`
/// boundary so `basename`s containing dots (e.g. `archive.tar.gz`)
/// still split correctly.
pub(super) fn split_basename_and_suffix(name: &str) -> Option<(String, String)> {
    let bytes = name.as_bytes();
    let mut last_d: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1] == b'd' {
            last_d = Some(i);
        }
        i += 1;
    }
    let dot = last_d?;
    let (basename, dot_suffix) = name.split_at(dot);
    if basename.is_empty() {
        return None;
    }
    let suffix = &dot_suffix[1..]; // strip the leading '.'
                                   // Validate: `d<digits>` or `d<digits>_<digits>`.
    if !suffix.starts_with('d') {
        return None;
    }
    let rest = &suffix[1..];
    if rest.is_empty() {
        return None;
    }
    let valid = match rest.split_once('_') {
        Some((a, b)) => {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        }
        None => rest.chars().all(|c| c.is_ascii_digit()),
    };
    if !valid {
        return None;
    }
    Some((basename.to_string(), suffix.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_typical() {
        let (b, s) = split_basename_and_suffix("report.pdf.d1716000000").unwrap();
        assert_eq!(b, "report.pdf");
        assert_eq!(s, "d1716000000");
    }

    #[test]
    fn split_with_collision_suffix() {
        let (b, s) = split_basename_and_suffix("a.txt.d1716000000_2").unwrap();
        assert_eq!(b, "a.txt");
        assert_eq!(s, "d1716000000_2");
    }

    #[test]
    fn split_picks_last_d_boundary() {
        // Filename happens to contain another `.d` segment earlier on.
        let (b, s) = split_basename_and_suffix("draft.docx.d1716000000").unwrap();
        assert_eq!(b, "draft.docx");
        assert_eq!(s, "d1716000000");
    }

    #[test]
    fn split_rejects_non_trash_name() {
        assert!(split_basename_and_suffix("notes.txt").is_none());
        // `dfoo` isn't `d<digits>`.
        assert!(split_basename_and_suffix("notes.dfoo").is_none());
        // `d` alone is not `d<digits>`.
        assert!(split_basename_and_suffix("notes.d").is_none());
        // Empty basename.
        assert!(split_basename_and_suffix(".d12345").is_none());
        // Empty collision tail.
        assert!(split_basename_and_suffix("a.d12345_").is_none());
    }
}
