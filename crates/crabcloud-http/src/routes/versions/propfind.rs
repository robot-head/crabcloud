//! PROPFIND handlers for the DAV versions surface.
//!
//! Two entry points:
//! - [`root`] — `PROPFIND /{uid}/{fileid}/`. Emits the per-file
//!   collection plus (on Depth: 1) one `<d:response>` per version
//!   returned by `Versions::list_for`.
//! - [`entry`] — `PROPFIND /{uid}/{fileid}/{version_mtime}`. Looks up
//!   the matching row by `(user, fileid, version_mtime)` and emits a
//!   single `<d:response>`.
//!
//! Wire shape mirrors Nextcloud's DAV versions endpoint: hrefs are
//! `/remote.php/dav/versions/{uid}/{fileid}/{version_mtime}` regardless
//! of which surface alias the request came in on, `displayname` is the
//! original basename (derived from `entry.path`), `getlastmodified` is
//! the snapshot mtime, and `getcontentlength` is the snapshot size.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_versions::VersionEntry;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use std::time::{Duration, UNIX_EPOCH};

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_depth, Depth};
use crate::routes::dav::xml::{
    multistatus, write_empty, write_leaf, write_propstat, write_response,
};

/// HREF prefix emitted in version responses. Matches the authed-files
/// surface which always emits `/remote.php/dav/...` hrefs regardless of
/// which alias the request came in on — Nextcloud-compatible.
const HREF_PREFIX: &str = "/remote.php/dav/versions";

/// Build the per-file root multistatus body. Depth: 0 returns just the
/// collection; Depth: 1 (or default-Infinity) walks `Versions::list_for`
/// and emits one `<d:response>` per version. Returns 207 Multi-Status.
pub async fn root(
    state: &AppState,
    uid: &str,
    fileid: i64,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let depth = parse_depth(headers, Depth::Infinity)?;
    let entries = if matches!(depth, Depth::One | Depth::Infinity) {
        state
            .versions
            .list_for(uid, fileid)
            .await
            .map_err(super::versions_err)?
    } else {
        Vec::new()
    };

    let body = multistatus(|w| {
        // Root collection response. Use the bare `/<uid>/<fileid>/`
        // href even though our router accepts both with and without a
        // trailing slash — the trailing slash is the canonical DAV
        // collection spelling.
        let root_href = format!("{HREF_PREFIX}/{uid}/{fileid}/");
        write_response(w, &root_href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| {
                w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
                write_empty(w, "d:collection")?;
                w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
                write_leaf(w, "d:displayname", &fileid.to_string())?;
                Ok(())
            })
        })?;

        for e in &entries {
            let href = format!("{HREF_PREFIX}/{uid}/{fileid}/{}", e.version_mtime);
            write_response(w, &href, |w| {
                write_propstat(w, "HTTP/1.1 200 OK", |w| write_entry_props(w, e))
            })?;
        }
        Ok(())
    });

    Ok(multistatus_response(body))
}

/// Per-entry PROPFIND. Looks up the row by `(uid, fileid, version_mtime)`
/// and emits a single-entry multistatus. 404 if no matching row.
pub async fn entry(
    state: &AppState,
    uid: &str,
    fileid: i64,
    version_mtime: i64,
    _headers: &HeaderMap,
) -> DavResult<Response> {
    let entries = state
        .versions
        .list_for(uid, fileid)
        .await
        .map_err(super::versions_err)?;
    let entry = entries
        .into_iter()
        .find(|e| e.version_mtime == version_mtime)
        .ok_or(DavError::NotFound)?;

    let body = multistatus(|w| {
        let href = format!("{HREF_PREFIX}/{uid}/{fileid}/{}", entry.version_mtime);
        write_response(w, &href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| write_entry_props(w, &entry))
        })?;
        Ok(())
    });
    Ok(multistatus_response(body))
}

/// Emit the per-entry `<d:prop>` body (caller wraps in `<d:propstat>`).
fn write_entry_props(
    w: &mut quick_xml::Writer<std::io::Cursor<Vec<u8>>>,
    e: &VersionEntry,
) -> Result<(), quick_xml::Error> {
    write_leaf(w, "d:getcontentlength", &e.size.to_string())?;
    let mtime = UNIX_EPOCH + Duration::from_secs(e.version_mtime.max(0) as u64);
    write_leaf(w, "d:getlastmodified", &httpdate::fmt_http_date(mtime))?;
    w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
    // Versions are always file resources, never collections.
    w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
    write_leaf(
        w,
        "d:displayname",
        std::path::Path::new(e.path.trim_start_matches('/'))
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&e.path),
    )?;
    // Content-Type best-effort: emit octet-stream for now. Filecache
    // mime lookup would be slightly nicer but adds a per-row DB hit for
    // marginal client value (clients already know the current mime).
    write_leaf(w, "d:getcontenttype", "application/octet-stream")?;
    Ok(())
}

/// Build the 207 Multi-Status response shell.
fn multistatus_response(body: Vec<u8>) -> Response {
    (
        StatusCode::from_u16(207).expect("207 is valid"),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        )],
        Body::from(body),
    )
        .into_response()
}
