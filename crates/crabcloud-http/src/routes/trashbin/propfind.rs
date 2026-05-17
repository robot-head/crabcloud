//! PROPFIND handlers for the DAV trashbin surface.
//!
//! Two entry points:
//! - [`root`] — `PROPFIND /{uid}/` and `PROPFIND /{uid}/trash/`. Emits
//!   the trash-root collection plus (on Depth: 1) one `<d:response>`
//!   per entry returned by `Trash::list`.
//! - [`entry`] — `PROPFIND /{uid}/trash/{basename}.{suffix}`. Looks up
//!   the row by `(user, basename, suffix)` and emits a single
//!   `<d:response>`.
//!
//! The wire shape mirrors Nextcloud's DAV trashbin: hrefs are
//! `/remote.php/dav/trashbin/{uid}/trash/{basename}.{suffix}`, the
//! basename appears in `<d:displayname>`, `getlastmodified` reflects
//! `deleted_at`, and the original parent + basename are surfaced as the
//! custom `{http://nextcloud.org/ns}trashbin-original-location` property.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_trash::{TrashEntry, TrashType};
use quick_xml::events::{BytesEnd, BytesStart, Event};
use std::time::{Duration, UNIX_EPOCH};

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_depth, Depth};
use crate::routes::dav::xml::{multistatus, write_empty, write_leaf, write_propstat, write_response};

/// HREF prefix used in trashbin responses. The handler emits this prefix
/// verbatim regardless of which surface alias (`/dav/trashbin/...` vs
/// `/remote.php/dav/trashbin/...`) the request came in on — matches the
/// authed-files surface, which always emits `/remote.php/dav/files/...`
/// hrefs (matches Nextcloud's wire shape and desktop-client convention).
const HREF_PREFIX: &str = "/remote.php/dav/trashbin";

/// Build the trash-root multistatus body. Depth: 0 returns just the
/// collection; Depth: 1 (or default-Infinity) walks `Trash::list` and
/// emits one `<d:response>` per entry. Returns 207 Multi-Status.
pub async fn root(state: &AppState, uid: &str, headers: &HeaderMap) -> DavResult<Response> {
    let depth = parse_depth(headers, Depth::Infinity)?;
    let entries = if matches!(depth, Depth::One | Depth::Infinity) {
        state.trash.list(uid).await.map_err(trash_err)?
    } else {
        Vec::new()
    };
    // Resolve file sizes BEFORE entering the writer closure — the
    // closure is sync, so any per-entry IO must be done up-front.
    let mut sized: Vec<(TrashEntry, Option<u64>)> = Vec::with_capacity(entries.len());
    for e in entries {
        let size = file_size_for_entry(state, &e).await;
        sized.push((e, size));
    }

    let body = multistatus(|w| {
        // Root collection response.
        let root_href = format!("{HREF_PREFIX}/{uid}/trash/");
        write_response(w, &root_href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| {
                w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
                write_empty(w, "d:collection")?;
                w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
                write_leaf(w, "d:displayname", "trash")?;
                Ok(())
            })
        })?;

        for (e, size) in &sized {
            let href = format!(
                "{HREF_PREFIX}/{uid}/trash/{}",
                encode_segment(&format!("{}.{}", e.basename, e.suffix))
            );
            write_response(w, &href, |w| {
                write_propstat(w, "HTTP/1.1 200 OK", |w| write_entry_props(w, e, *size))
            })?;
        }
        Ok(())
    });

    Ok(multistatus_response(body))
}

/// Per-entry PROPFIND. Looks up the row and emits a single-entry
/// multistatus. 404 if the row doesn't exist.
pub async fn entry(
    state: &AppState,
    uid: &str,
    name: &str,
    _headers: &HeaderMap,
) -> DavResult<Response> {
    let (basename, suffix) = super::split_basename_and_suffix(name).ok_or(DavError::NotFound)?;
    let entry = match state.trash.get_by_name(uid, &basename, &suffix).await {
        Ok(e) => e,
        Err(crabcloud_trash::TrashError::NotFound) => return Err(DavError::NotFound),
        Err(other) => return Err(trash_err(other)),
    };
    let size = file_size_for_entry(state, &entry).await;

    let body = multistatus(|w| {
        let href = format!(
            "{HREF_PREFIX}/{uid}/trash/{}",
            encode_segment(&format!("{}.{}", entry.basename, entry.suffix))
        );
        write_response(w, &href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| {
                write_entry_props(w, &entry, size)
            })
        })?;
        Ok(())
    });
    Ok(multistatus_response(body))
}

/// Emit the per-entry `<d:prop>` body (caller wraps in `<d:propstat>`).
fn write_entry_props(
    w: &mut quick_xml::Writer<std::io::Cursor<Vec<u8>>>,
    e: &TrashEntry,
    size: Option<u64>,
) -> Result<(), quick_xml::Error> {
    if matches!(e.r#type, TrashType::File) {
        if let Some(sz) = size {
            write_leaf(w, "d:getcontentlength", &sz.to_string())?;
        }
    }
    let mtime = UNIX_EPOCH + Duration::from_secs(e.deleted_at.max(0) as u64);
    write_leaf(w, "d:getlastmodified", &httpdate::fmt_http_date(mtime))?;
    w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
    if matches!(e.r#type, TrashType::Dir) {
        write_empty(w, "d:collection")?;
    }
    w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
    write_leaf(w, "d:displayname", &e.basename)?;
    // The original-location property is `<location>/<basename>` joined
    // without doubling the root slash. `location` is "/" for items
    // deleted at the user root.
    let original_location = if e.location == "/" {
        format!("/{}", e.basename)
    } else {
        format!("{}/{}", e.location.trim_end_matches('/'), e.basename)
    };
    write_leaf(w, "nc:trashbin-original-location", &original_location)?;
    write_leaf(w, "nc:trashbin-deletion-time", &e.deleted_at.to_string())?;
    Ok(())
}

/// Stat the on-disk trash file (best effort) so PROPFIND can surface a
/// size. Returns `None` on any failure — the file may be missing if the
/// trash row got out of sync (logged at warn). Directories return
/// `None` since we don't recursively size them in MVP.
async fn file_size_for_entry(state: &AppState, e: &TrashEntry) -> Option<u64> {
    if matches!(e.r#type, TrashType::Dir) {
        return None;
    }
    let path = state
        .trash
        .datadir()
        .join(&e.user)
        .join("files_trashbin")
        .join("files")
        .join(format!("{}.{}", e.basename, e.suffix));
    match tokio::fs::metadata(&path).await {
        Ok(m) => Some(m.len()),
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "trashbin propfind: stat failed");
            None
        }
    }
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

/// Percent-encode a path segment for href emission. Leaves
/// alphanumerics and `-._~` alone; encodes everything else as `%HH`
/// per RFC 3986.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
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
    fn encode_segment_preserves_safe_chars() {
        assert_eq!(encode_segment("report.pdf.d1716000000"), "report.pdf.d1716000000");
        assert_eq!(encode_segment("a-b_c~d"), "a-b_c~d");
    }

    #[test]
    fn encode_segment_escapes_space_and_slash() {
        assert_eq!(encode_segment("hi there.txt.d1"), "hi%20there.txt.d1");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
    }
}
