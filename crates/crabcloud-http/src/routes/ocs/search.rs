//! OCS unified-search-provider endpoint for file metadata.
//!
//! Nextcloud spelling: `/ocs/v2.php/search/providers/files/search`.
//! `GET ?query=<q>&limit=<N>&cursor=<token>` returns JSON results in the
//! standard OCS envelope. Response shape matches Nextcloud's
//! unified-search provider format so existing third-party clients work
//! without translation.
//!
//! * `limit` defaults to 20, clamped to `[1, 50]` per spec §5.1.
//! * `cursor` is an opaque base64-encoded `"<rank>|<fileid>"` tuple from
//!   the prior page's last hit (no padding).
//! * An empty `query` short-circuits to `{ entries: [], cursor: null,
//!   isLast: true }` — the underlying [`Search::query`] also short-
//!   circuits filter-only / empty parses to an empty result set, so the
//!   OCS layer never needs to re-check.
//!
//! Envelope helpers live in [`super::envelope`] and are shared with
//! the other OCS modules so the wire shape stays single-sourced.

use super::envelope::ocs_envelope;
use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Extension;
use base64::Engine as _;
use crabcloud_core::AppState;
use crabcloud_ocs::Format;
use crabcloud_search::{parse_query, SearchError, SearchHit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/search", get(search_handler))
}

// --- error mapping ---------------------------------------------------------

/// Map `SearchError` → OCS envelope. The search surface has no
/// user-actionable error variants today (every error is either a DB
/// failure or a filecache failure); log fail-fast at `tracing::error!`
/// and surface 500. Mirrors the activity OCS module's policy.
fn from_search_error(err: SearchError, fmt: Format) -> Response {
    tracing::error!(error = %err, "search OCS handler: unhandled SearchError");
    ocs_envelope(500, &err.to_string(), Value::Null, fmt)
}

// --- wire DTOs -------------------------------------------------------------

#[derive(Serialize)]
struct EntryAttributes {
    fileid: String,
    mime: String,
    size: String,
    mtime: String,
}

/// One row in the response `entries` array. Field naming mirrors the
/// Nextcloud unified-search-provider shape so existing clients consume
/// it without translation. The numeric attributes are stringified to
/// match Nextcloud's wire (its PHP layer JSON-encodes everything as
/// strings).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EntryDto {
    thumbnail_url: String,
    title: String,
    subline: String,
    resource_url: String,
    icon: String,
    rounded: bool,
    attributes: EntryAttributes,
}

fn hit_to_entry(h: &SearchHit) -> EntryDto {
    EntryDto {
        thumbnail_url: String::new(),
        title: h.basename.clone(),
        subline: h.path.clone(),
        resource_url: format!("/files{}", h.path),
        icon: String::new(),
        rounded: false,
        attributes: EntryAttributes {
            fileid: h.fileid.to_string(),
            mime: h.mime.clone(),
            size: h.size.to_string(),
            mtime: h.mtime.to_string(),
        },
    }
}

// --- cursor codec ----------------------------------------------------------

/// Base64-encode `(rank, fileid)` as the opaque `cursor` token.
/// Format is `"<rank>|<fileid>"` so the codec is human-debuggable when
/// the bytes are decoded outside the server (e.g. in test failure
/// output). No padding, matching the SP8 / SP14 cursor pattern.
fn encode_cursor(rank: f64, fileid: i64) -> String {
    let payload = format!("{rank}|{fileid}");
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(payload)
}

/// Inverse of [`encode_cursor`]. Returns a generic error tag on any
/// failure; the caller maps that to a 400 envelope. Tags are short
/// strings so logs / error envelopes stay terse without leaking
/// internal codec implementation.
fn decode_cursor(s: &str) -> Result<(f64, i64), &'static str> {
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s)
        .map_err(|_| "b64")?;
    let s = std::str::from_utf8(&raw).map_err(|_| "utf8")?;
    let (a, b) = s.split_once('|').ok_or("split")?;
    let rank: f64 = a.parse().map_err(|_| "rank")?;
    let fileid: i64 = b.parse().map_err(|_| "fileid")?;
    Ok((rank, fileid))
}

// --- handler ---------------------------------------------------------------

#[derive(Deserialize, Default)]
struct SearchParams {
    #[serde(default)]
    query: String,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn search_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Query(p): Query<SearchParams>,
) -> Response {
    // Empty query short-circuits to an empty result envelope. Spec §5.1
    // mandates `cursor: null, isLast: true` in this case. `Search::query`
    // would also return `[]` for an empty parse, but doing it up front
    // skips the cursor / parser work and matches the spec literally.
    if p.query.is_empty() {
        return ocs_envelope(
            200,
            "OK",
            serde_json::json!({
                "name": "Files",
                "isPaginated": true,
                "entries": [],
                "cursor": Value::Null,
                "isLast": true,
            }),
            fmt.0,
        );
    }

    // Defaults per spec §5.1: limit defaults to 20, max 50. Negative
    // values clamp to 1 (mirrors the policy in the activity / versions
    // OCS modules — fast-fail-soft on garbage client input).
    let limit = p.limit.unwrap_or(20).clamp(1, 50);

    let cursor = match p.cursor.as_deref().map(decode_cursor) {
        Some(Ok(c)) => Some(c),
        Some(Err(_)) => {
            return ocs_envelope(400, "bad cursor", Value::Null, fmt.0);
        }
        None => None,
    };

    let parsed = parse_query(&p.query);
    let hits = match state
        .search
        .query(ctx.user_id.as_str(), &parsed, limit, cursor)
        .await
    {
        Ok(h) => h,
        Err(e) => return from_search_error(e, fmt.0),
    };

    // `next_cursor` is the (rank, fileid) of the last (lowest-ranked /
    // largest-fileid-tiebreak) hit in this page. The client passes it
    // back as `?cursor=` for the next page. None when the page is empty,
    // matching the activity feed's `next_since`.
    let next_cursor = hits.last().map(|h| encode_cursor(h.rank, h.fileid));
    let is_last = (hits.len() as i64) < limit;
    let entries: Vec<EntryDto> = hits.iter().map(hit_to_entry).collect();

    ocs_envelope(
        200,
        "OK",
        serde_json::json!({
            "name": "Files",
            "isPaginated": true,
            "entries": entries,
            "cursor": next_cursor,
            "isLast": is_last,
        }),
        fmt.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let s = encode_cursor(-1.234, 42);
        let (r, id) = decode_cursor(&s).unwrap();
        assert!((r - -1.234).abs() < f64::EPSILON);
        assert_eq!(id, 42);
    }

    #[test]
    fn decode_cursor_rejects_garbage() {
        // Not base64.
        assert!(decode_cursor("!!!not base64!!!").is_err());
        // Valid base64 but no pipe.
        let s = base64::engine::general_purpose::STANDARD_NO_PAD.encode("no-pipe-here");
        assert!(decode_cursor(&s).is_err());
        // Pipe but non-numeric rank.
        let s = base64::engine::general_purpose::STANDARD_NO_PAD.encode("notafloat|42");
        assert!(decode_cursor(&s).is_err());
        // Pipe but non-numeric fileid.
        let s = base64::engine::general_purpose::STANDARD_NO_PAD.encode("1.0|notanint");
        assert!(decode_cursor(&s).is_err());
    }
}
