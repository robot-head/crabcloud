//! `#[server]` function for the Dioxus top-bar search UI. Wraps the
//! `crabcloud-search` service with a thinner DTO shape than the OCS
//! `/search/providers/files/search` envelope: the UI doesn't need the
//! Nextcloud unified-search wrapper, just `(hits, cursor)`.
//!
//! Auth: runs through the production `AuthLayer`. The
//! [`super::require_user`] helper hands the body a `(AppState, UserId)`
//! pair and short-circuits anonymous callers with `unauthorized`.
//!
//! Cursor shape: opaque to clients, internally `base64(rank|fileid)`.
//! Matches the OCS surface's encoding so future polish can share helpers.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One search hit, returned by [`search_files`]. Trimmed-down vs.
/// `crabcloud_search::SearchHit`: the UI never needs `storage_id` or
/// the raw `rank` (only the server-side cursor encoder cares about
/// rank).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHitDto {
    pub fileid: i64,
    pub basename: String,
    pub path: String,
    pub mime: String,
    pub mtime: i64,
    pub size: i64,
}

/// Response payload for [`search_files`]. `cursor` is `None` when the
/// returned page filled completely below the limit (i.e. last page);
/// callers pass it back as `cursor` on the next call to advance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponseDto {
    pub hits: Vec<SearchHitDto>,
    pub cursor: Option<String>,
}

/// Default page size for the top-bar dropdown. Spec §5.3 caps the
/// dropdown at up to 10 hits; the server fn enforces it server-side so
/// a curious client can't request a larger window.
#[cfg(feature = "server")]
const DEFAULT_LIMIT: i64 = 10;

/// `POST /api/files/search` — return up to 10 hits matching `query`
/// for the authed user. Empty / whitespace-only `query` short-circuits
/// to an empty response without touching the DB (matches the OCS
/// surface's "type to search" empty state). `cursor`, when provided,
/// is the opaque token returned by a prior call's `cursor` field.
#[server(endpoint = "api/files/search", prefix = "")]
pub async fn search_files(
    query: String,
    cursor: Option<String>,
) -> Result<SearchResponseDto, ServerFnError> {
    use crabcloud_search::parse_query;
    let (state, uid) = super::require_user().await?;
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(SearchResponseDto {
            hits: Vec::new(),
            cursor: None,
        });
    }
    let parsed = parse_query(trimmed);
    let cursor_tuple = cursor.as_deref().and_then(decode_cursor);
    let hits = state
        .search
        .query(uid.as_str(), &parsed, DEFAULT_LIMIT, cursor_tuple)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "search server fn failed");
            ServerFnError::new(format!("search: {e}"))
        })?;
    // Only emit a next-cursor when the page filled — otherwise the
    // caller is on the last page and a fresh cursor would just return
    // an empty next page. Mirrors the OCS `isLast` semantics.
    let next_cursor = if hits.len() as i64 == DEFAULT_LIMIT {
        hits.last().map(|h| encode_cursor(h.rank, h.fileid))
    } else {
        None
    };
    let hits = hits
        .into_iter()
        .map(|h| SearchHitDto {
            fileid: h.fileid,
            basename: h.basename,
            path: h.path,
            mime: h.mime,
            mtime: h.mtime,
            size: h.size,
        })
        .collect();
    Ok(SearchResponseDto {
        hits,
        cursor: next_cursor,
    })
}

/// Encode `(rank, fileid)` as `base64_nopad("rank|fileid")`. The format
/// is opaque to clients; only `decode_cursor` reads it back.
#[cfg(feature = "server")]
fn encode_cursor(rank: f64, fileid: i64) -> String {
    use base64::Engine as _;
    let payload = format!("{rank}|{fileid}");
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(payload)
}

/// Decode a `base64_nopad("rank|fileid")` cursor. Any malformed input
/// returns `None`, which the query path treats as "start from the
/// top" — safe because the caller passes the cursor server-to-server
/// via the previous response.
#[cfg(feature = "server")]
fn decode_cursor(s: &str) -> Option<(f64, i64)> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s)
        .ok()?;
    let s = std::str::from_utf8(&raw).ok()?;
    let (a, b) = s.split_once('|')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_preserves_rank_and_fileid() {
        let enc = encode_cursor(1.5, 42);
        let (rank, fileid) = decode_cursor(&enc).expect("decode");
        assert!((rank - 1.5).abs() < f64::EPSILON);
        assert_eq!(fileid, 42);
    }

    #[test]
    fn decode_cursor_rejects_malformed_input() {
        assert!(decode_cursor("not base64!").is_none());
        // base64-valid but no `|` separator
        use base64::Engine as _;
        let bad = base64::engine::general_purpose::STANDARD_NO_PAD.encode("no separator here");
        assert!(decode_cursor(&bad).is_none());
        // Wrong number of parts after split
        let bad2 = base64::engine::general_purpose::STANDARD_NO_PAD.encode("abc|def");
        assert!(decode_cursor(&bad2).is_none());
    }
}
