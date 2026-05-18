//! Public-facing value types for the search service.

use serde::{Deserialize, Serialize};

/// Parsed user query. The text part feeds the FTS match; the filters
/// become AND clauses on the SQL side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Bare tokens joined by space (FTS match input).
    pub text: String,
    /// Quoted phrase, if any (at most one in MVP).
    pub phrase: Option<String>,
    /// Mime glob (`image/*` or `application/pdf`).
    pub mime: Option<String>,
    /// Unix seconds, inclusive.
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
    pub size_min: Option<i64>,
    pub size_max: Option<i64>,
}

impl SearchQuery {
    /// True iff the parsed query has no actionable matchable input —
    /// no text, no phrase, no filters. Used to short-circuit empty
    /// searches to empty results.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.phrase.is_none()
            && self.mime.is_none()
            && self.modified_after.is_none()
            && self.modified_before.is_none()
            && self.size_min.is_none()
            && self.size_max.is_none()
    }

    /// True iff the query has a text/phrase component the FTS engine
    /// can match against. Filters-only queries return false (and the
    /// service short-circuits to empty per spec §2 decision #7).
    pub fn has_text_match(&self) -> bool {
        !self.text.is_empty() || self.phrase.is_some()
    }
}

/// A single row in a batched upsert. Same shape as the args to
/// [`crate::Search::upsert_for_file`], in struct form so
/// [`crate::Search::upsert_many`] can take a slice.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchUpsertRow {
    pub viewer_uid: String,
    pub fileid: i64,
    pub storage_id: String,
    pub basename: String,
    pub path: String,
    pub mime: String,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub fileid: i64,
    pub storage_id: String,
    pub basename: String,
    pub path: String,
    pub mime: String,
    pub mtime: i64,
    pub size: i64,
    /// FTS rank (BM25-flavored; lower = more relevant on sqlite/mysql,
    /// higher = more relevant on postgres `ts_rank_cd`). Used for
    /// ordering + cursor pagination; opaque to clients.
    pub rank: f64,
}
