//! MOVE handler — restores a trash entry.
//!
//! Two restore modes:
//! - Without a `Destination:` header, the entry is restored to its
//!   original `location/basename` (auto-creating parent dirs if needed).
//! - With a `Destination: /dav/files/{uid}/<path>` (or
//!   `/remote.php/dav/files/{uid}/<path>`) header, the entry is
//!   restored to the supplied path. Cross-user destinations and
//!   destinations outside `/dav/files/{uid}/` are rejected with 400.
//!
//! On collision the trash service appends ` (restored)`, then
//! ` (restored 2)`, etc. (capped at 99) so users don't silently
//! overwrite work they did since the delete.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use axum::http::HeaderMap;

use crate::routes::dav::error::{DavError, DavResult};

pub async fn restore(
    state: &AppState,
    uid: &str,
    name: &str,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let (basename, suffix) = super::split_basename_and_suffix(name).ok_or(DavError::NotFound)?;
    let entry = match state.trash.get_by_name(uid, &basename, &suffix).await {
        Ok(e) => e,
        Err(crabcloud_trash::TrashError::NotFound) => return Err(DavError::NotFound),
        Err(other) => return Err(trash_err(other)),
    };

    let dest_override = match headers.get("destination").and_then(|v| v.to_str().ok()) {
        Some(raw) => Some(parse_destination(raw, uid)?),
        None => None,
    };

    match state
        .trash
        .restore(uid, entry.id, dest_override.as_deref())
        .await
    {
        Ok(_restored) => Ok((StatusCode::CREATED, "").into_response()),
        Err(e) => Err(trash_err(e)),
    }
}

/// Strip the surface prefix and the `/files/<uid>/` segment from a
/// `Destination` header; return the user-relative path (with a leading
/// `/`). Rejects destinations outside the authed user's files namespace.
fn parse_destination(dest: &str, uid: &str) -> DavResult<String> {
    // Strip absolute URL prefix if present (some clients send full URLs).
    let path = match dest.find("://") {
        Some(_) => {
            let (_scheme, after) = dest
                .split_once("://")
                .ok_or_else(|| DavError::BadRequest("malformed Destination URL".into()))?;
            match after.find('/') {
                Some(i) => after[i..].to_string(),
                None => return Err(DavError::BadRequest("Destination missing path".into())),
            }
        }
        None => dest.to_string(),
    };
    // URL-decode in case the client percent-encoded path segments.
    let decoded = urlencoding::decode(&path)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let decoded = decoded.to_string();
    let prefixes = [
        format!("/remote.php/dav/files/{uid}/"),
        format!("/dav/files/{uid}/"),
    ];
    for p in &prefixes {
        if let Some(rest) = decoded.strip_prefix(p.as_str()) {
            return Ok(format!("/{}", rest.trim_start_matches('/')));
        }
    }
    Err(DavError::BadRequest(format!(
        "Destination not under /dav/files/{uid}/: {dest}"
    )))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_destination_strips_legacy_prefix() {
        let p = parse_destination("/remote.php/dav/files/alice/foo/bar.txt", "alice").unwrap();
        assert_eq!(p, "/foo/bar.txt");
    }

    #[test]
    fn parse_destination_strips_modern_prefix() {
        let p = parse_destination("/dav/files/alice/x.txt", "alice").unwrap();
        assert_eq!(p, "/x.txt");
    }

    #[test]
    fn parse_destination_strips_absolute_url() {
        let p =
            parse_destination("https://example.com/dav/files/alice/x.txt", "alice").unwrap();
        assert_eq!(p, "/x.txt");
    }

    #[test]
    fn parse_destination_decodes_percent_escapes() {
        let p =
            parse_destination("/dav/files/alice/hello%20world.txt", "alice").unwrap();
        assert_eq!(p, "/hello world.txt");
    }

    #[test]
    fn parse_destination_rejects_wrong_user() {
        let r = parse_destination("/dav/files/bob/x.txt", "alice");
        assert!(matches!(r, Err(DavError::BadRequest(_))));
    }

    #[test]
    fn parse_destination_rejects_outside_files() {
        let r = parse_destination("/dav/trashbin/alice/x.txt.d1", "alice");
        assert!(matches!(r, Err(DavError::BadRequest(_))));
    }
}
