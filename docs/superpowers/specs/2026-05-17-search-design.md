# Search — Design (Sub-project 15)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-17
**Sub-project:** 15. Picks up after SP14 activity feed shipped (PRs #181–#186). Touches `crabcloud-fs` (storage_sink consumer), `crabcloud-sharing` (fan-out hooks), `crabcloud-http::routes::ocs`, and the Dioxus chrome top-bar. Adds a new `crabcloud-search` crate plus an `oc_search` table (FTS5 virtual on sqlite, FULLTEXT-equipped table on mysql, tsvector + GIN table on postgres).

## 1. Goal

Ship file metadata search across the user's accessible files (home + accepted shares + group shares + share-mount paths). Bare-term match on basename + path, with inline filter operators (`mime:image/*`, `modified:>2024-01-01`, `size:>1MB`). Per-user materialized index for sub-linear read performance regardless of share-graph complexity. Async-maintained via the existing `storage_sink` event stream.

In MVP scope:

- New `crabcloud-search` crate: `Search::{query, upsert_for_file, delete_for_file, delete_for_viewer_file, fan_out_for_share, fan_out_for_unshare, query_parse}` over multidialect SQL with FTS-flavored matching.
- New migration `0012_search_index` (sqlite FTS5, mysql FULLTEXT, postgres tsvector+GIN).
- `SearchIndexer` background task subscribed to `storage_sink` — handles `Created/Modified/Deleted/Renamed` events, recomputes recipient set via `Shares::recipients_for_fileid`, UPSERTs/DELETEs one row per `(viewer_uid, fileid)`.
- Hooks in `Shares::{create, delete}` for bulk fan-out at share lifecycle events (one row per (recipient, fileid) for every file under the shared subroot).
- Query parser: bare tokens + `mime:<glob>` + `modified:>EPOCH|ISO` + `size:>N{B,KB,MB,GB}` + quoted phrases. Unknown `key:value` falls through to text terms.
- Per-user materialized rows: `PRIMARY KEY (viewer_uid, fileid)`. Path stored is the **viewer's** path (share-mount-translated).
- OCS REST: `/ocs/v2.php/search/providers/files/search` — Nextcloud "unified search" provider spelling.
- Server fn + UI top-bar `<SearchBar>` with debounced input + dropdown of hits.
- E2E + unit tests on every layer.

Explicitly out of scope (deferred):

- **Full-text content extraction** (PDF text, DOCX, ODF, plain text body content). Metadata only.
- **Tag / favorite / comment search** (no tag/favorite/comment systems yet).
- **Dedicated `/search` page** with filters (mime dropdown, owner picker, date pickers). Top-bar dropdown only in MVP.
- **DAV SEARCH (RFC 5323)**. Niche; Nextcloud clients don't use it.
- **Initial backfill of existing filecache rows**. Index starts empty post-migration; only future writes populate. Operator-driven rescan deferred to a future SP / xtask.
- **Group membership change retroactive backfill**. Adding a new group member doesn't add historic share files to their index; only files touched (and re-indexed) since they joined become searchable from their view. Matches SP14 activity semantics.
- **Search-time per-user opt-out** (`oc_search_settings` equivalent). No per-event-type or per-folder mute for search; search results are all-or-nothing per visibility.
- **Suggestions / autocomplete / typeahead** beyond the dropdown showing partial matches.
- **Cross-language stemming normalization**. Each dialect uses its default tokenizer; semantic divergence between sqlite/mysql/postgres is documented but not normalized.
- **Trash / version search**. Soft-deleted files vanish from the index per spec; trash bin has its own DAV/UI surface.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Metadata-only corpus (basename + path)**. No file content extraction in MVP. | Smallest impl with highest user value per dev-hour. Content extraction needs per-mime extractors (PDF, DOCX, ODF, …), storage cost grows materially, and the indexing latency goes from "milliseconds" to "seconds per large file." Defer to a follow-up SP that's its own design conversation. |
| 2 | **Per-dialect FTS (sqlite FTS5, mysql FULLTEXT, postgres tsvector + GIN)**. Each dialect uses its native full-text mechanism with `UNINDEXED`/regular columns for the filter fields. | Real BM25-style ranking + token matching on every dialect. Hybrid (LIKE on sqlite, FULLTEXT elsewhere) was rejected for asymmetric semantics. Plain LIKE was rejected for scale (full-scan on every query). |
| 3 | **Per-user materialized: `PRIMARY KEY (viewer_uid, fileid)`**. One row per (viewer, file) pair; fan-out at write/share time. | Query-time join against `oc_share` was the alternative — kills perf when a user has many shares. Materialization mirrors the SP14 activity fan-out pattern (workspace consistency). Trade-off: index size scales with `Σ recipients`. For an MVP this is acceptable; documented in the spec. |
| 4 | **`path` field stores the VIEWER's path**, not the owner's. For a share-mount where Bob sees `/from-alice/report.docx` (owner is Alice's `/docs/report.docx`), Bob's row has `path = "/from-alice/report.docx"`. | Hit results render the path the user actually sees; click-to-open works without per-result translation. Computed at fan-out time via `Mount::recipient_path_for(owner_path)` or equivalent. |
| 5 | **Async indexer via `storage_sink::ChannelEventSink`** — `SearchIndexer` task subscribed at startup. Inline write to the index from `View::write_file`/etc. was rejected: blocks the write hot path on index ops, plus duplicates work that the scanner already does via the sink. | Reuses the existing event stream (the scanner is already a subscriber). Indexer drops events with `tracing::warn!` if the channel buffer fills (rare; documented). Index is eventually consistent — "I just uploaded the file, where is it" works within sub-second under normal load. |
| 6 | **Query parser supports inline filters**: `mime:<glob>`, `modified:>EPOCH|ISO|YYYY-MM-DD`, `modified:<...`, `modified:YYYY-MM-DD..YYYY-MM-DD`, `size:>N{B,KB,MB,GB,TB}`, `size:<...`, quoted phrases (`"q3 report"`). Unknown `key:value` → text term (graceful degradation). | Matches Nextcloud's unified-search power-user surface. Small recursive-descent parser; no full grammar. Quoted phrases use the dialect's native phrase support (sqlite/mysql `"x y"`, postgres `<->`). |
| 7 | **Empty query returns empty result set**, not "all files". Likewise queries with no text terms but only filters return empty unless the dialect can short-circuit. | Surface a "type to search" empty state in the UI; avoid accidental full-table scans. Operators that want all-files-of-a-mime can craft `*:* mime:image/*` if we ever support a `*:*` token (out of MVP). |
| 8 | **No initial backfill**. The `0012` migration creates the empty table; existing files only appear after they're written/touched. | Operators with large existing corpora need a manual `xtask` rescan (out of MVP scope, flagged as a follow-up). The trade-off is honest and documented; building the backfill plumbing right (chunked, resumable) is a separate concern. |
| 9 | **Indexer survives panics per event**. Each event handler is wrapped in a `tokio::task::spawn_blocking` panic-catcher equivalent (`std::panic::catch_unwind` inside an async block — or the standard `tokio::task::spawn(...)` + `JoinError::is_panic()` recovery), so a bad row decode doesn't kill the indexer task. | Otherwise a single corrupt event would silently disable indexing for the rest of the process lifetime. |
| 10 | **No search-time access control checks**. The materialized index IS the access control: a row only exists in the index for users who can already see the file. No share-graph lookup at query time. | Trade-off accepted — share lifecycle events must keep the index in sync (covered by Tasks A6–A7 hooks in `Shares::{create, delete}`). A bug there would leak file metadata, so test coverage is mandatory. |
| 11 | **Move-out-of-shared-subroot detection**: when a rename moves a file from inside a shared subroot to outside, the recipient rows must be deleted. `SearchIndexer` handles this by re-resolving recipients post-rename and computing the delta vs pre-rename. | Cleanest correctness model. Slightly more work per rename event; acceptable. |
| 12 | **Trash-related events**: when `View::delete` soft-deletes (moves to `files_trashbin`), the `storage_sink` `Deleted` event fires for the original location. `SearchIndexer` deletes all viewer rows for that fileid. Trash entries are not searchable in MVP (matches Nextcloud). | The trashbin storage's own writes also fire events, but `SearchIndexer` skips storage_id rows that belong to a `trash::<uid>` storage (filter at the indexer). |
| 13 | **OCS surface** at `/ocs/v2.php/search/providers/files/search` (Nextcloud's "unified search provider" spelling). `GET ?query=…&limit=…&cursor=…` returns JSON. Cursor pagination via descending `rank` then `mtime` (FTS-native ranking). | Standard apps-API namespacing; Nextcloud mobile clients consume this URL. |
| 14 | **Server fn + UI**: top-bar `<SearchBar>` component in the existing `pages/files/chrome.rs::TopBar`. Debounced input (300ms) → `search_files(q)` server fn → dropdown of up to 10 hits with basename + path + mime icon. Click navigates to the file's containing folder. | Mirrors Nextcloud's unified-search dropdown. No dedicated page in MVP. Dropdown closes on `Escape` / click-outside / blur. |

## 3. Architecture

```
[write path]
  View::{write_file, delete, hard_delete, rename, rename_force_overwrite}
        │
        ▼
  LocalStorage.{put_file, delete, rename, copy}
        │
        ▼
  storage_sink.publish(StorageEvent { kind: Created|Modified|Deleted|Renamed, storage_id, path, ... })

[indexer path]
  SearchIndexer (subscriber)
        │ recv()
        ▼
  classify event:
    Created/Modified → resolve_recipients_for_fileid(fileid) → UPSERT one row per recipient
    Deleted          → DELETE WHERE fileid = ? (all viewers)
    Renamed          → re-resolve recipients (path-prefix may have crossed a share boundary)
                       → compute delta vs old recipient set
                       → UPSERT for additions, DELETE for removals
    [if storage is trash::<uid>] → skip (trash isn't searchable)

[share lifecycle path]
  Shares::create → for_each fileid under shared subroot:
                     → resolve_recipients_for_share(share_row) (target user OR group members)
                     → UPSERT one row per (recipient, fileid)
  Shares::delete → for_each fileid under the (former) shared subroot:
                     → DELETE one row per (former_recipient, fileid)

[query path]
  OCS GET /ocs/v2.php/search/providers/files/search?query=...
  Server fn search_files(query)
        │
        ▼
  Search::query_parse(query) → SearchQuery { text, mime, modified_range, size_range, phrase }
        │
        ▼
  Search::query(viewer_uid, parsed, limit, cursor) → Vec<SearchHit>

crabcloud-search  (NEW crate)
 ├─ SearchHit { fileid, storage_id, basename, path, mime, mtime, size, rank }
 ├─ SearchQuery { text, mime: Option<String>, modified_after: Option<i64>, modified_before: Option<i64>,
 │                size_min: Option<i64>, size_max: Option<i64>, phrase: Option<String> }
 ├─ Search { pool: Arc<DbPool> }
 ├─ Search::{query_parse, query, upsert_for_file, delete_for_file, delete_for_viewer_file,
 │           fan_out_for_share, fan_out_for_unshare}
 ├─ SearchIndexerEmit trait + impl on Search (mirrors SP14 ActivityEmitter pattern — used so
 │   Shares::{create, delete} can call into the indexer without a cycle)
 └─ Multidialect SQL via match self.pool.as_ref()

SearchIndexer  (crates/crabcloud-core/src/search_indexer.rs)
 ├─ new(rx: broadcast::Receiver<StorageEvent>, search: Arc<Search>, shares: Arc<Shares>, filecache: Arc<FileCache>)
 ├─ run(): loop { recv() → handle_event() }
 └─ Spawned in AppStateBuilder::build() unconditionally

AppState additions
 ├─ search: Arc<crabcloud_search::Search>
 └─ search_indexer_shutdown: Arc<tokio::sync::Notify>
```

## 4. Schema

### sqlite (FTS5 virtual table)

```sql
CREATE VIRTUAL TABLE oc_search USING fts5 (
    viewer_uid UNINDEXED,
    fileid     UNINDEXED,
    storage_id UNINDEXED,
    basename,                              -- tokenized
    path,                                  -- tokenized
    mime       UNINDEXED,
    mtime      UNINDEXED,
    size       UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- FTS5 supports WHERE viewer_uid = ? filtering via the UNINDEXED column; the
-- planner uses the fts5 BM25 ranking when MATCH is present and falls back to
-- a linear scan otherwise (acceptable; empty queries return empty results).
```

### mysql

```sql
CREATE TABLE oc_search (
    viewer_uid  VARCHAR(64)  NOT NULL,
    fileid      BIGINT       NOT NULL,
    storage_id  BIGINT       NOT NULL,
    basename    VARCHAR(255) NOT NULL,
    path        VARCHAR(512) NOT NULL,
    mime        VARCHAR(255) NOT NULL,
    mtime       BIGINT       NOT NULL,
    size        BIGINT       NOT NULL,
    PRIMARY KEY (viewer_uid, fileid),
    INDEX idx_search_viewer (viewer_uid),
    FULLTEXT INDEX ftx_search_text (basename, path)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
```

### postgres

```sql
CREATE TABLE oc_search (
    viewer_uid  VARCHAR(64)  NOT NULL,
    fileid      BIGINT       NOT NULL,
    storage_id  BIGINT       NOT NULL,
    basename    VARCHAR(255) NOT NULL,
    path        VARCHAR(512) NOT NULL,
    mime        VARCHAR(255) NOT NULL,
    mtime       BIGINT       NOT NULL,
    size        BIGINT       NOT NULL,
    tsv         tsvector     GENERATED ALWAYS AS (
                  to_tsvector('simple', basename || ' ' || path)
                ) STORED,
    PRIMARY KEY (viewer_uid, fileid)
);
CREATE INDEX idx_search_viewer    ON oc_search (viewer_uid);
CREATE INDEX idx_search_tsv       ON oc_search USING GIN (tsv);
```

`path` stores the **viewer-relative** path (share-mount-translated). The `(viewer_uid, fileid)` composite primary key uniquely identifies a row.

## 5. Surface contracts

### 5.1 OCS — `/ocs/v2.php/search/providers/files/search`

| Method | Path | Behavior |
|---|---|---|
| GET | `/search/providers/files/search?query=<q>&limit=<N>&cursor=<token>` | Returns JSON results in the OCS envelope. `limit` defaults 20, max 50. `cursor` opaque token (base64-encoded `(rank, fileid)` tuple from the prior page's last hit). |

Response envelope (Nextcloud-shaped):

```json
{
  "ocs": {
    "meta": { "status": "ok", "statuscode": 200 },
    "data": {
      "name": "Files",
      "isPaginated": true,
      "entries": [
        {
          "thumbnailUrl": "",
          "title": "report.docx",
          "subline": "/docs/report.docx",
          "resourceUrl": "/files/docs/report.docx",
          "icon": "",
          "rounded": false,
          "attributes": { "fileid": "123", "mime": "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "size": "12345", "mtime": "1716000000" }
        }
      ],
      "cursor": "MS4yLDEyMw==",
      "isLast": false
    }
  }
}
```

Empty query → `{ entries: [], cursor: null, isLast: true }`.

### 5.2 Server-fn API (Dioxus)

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct SearchHitDto {
    pub fileid: i64,
    pub basename: String,
    pub path: String,
    pub mime: String,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct SearchResponseDto {
    pub hits: Vec<SearchHitDto>,
    pub cursor: Option<String>,
}

#[server(endpoint = "/api/files/search")]
pub async fn search_files(query: String, cursor: Option<String>) -> Result<SearchResponseDto, ServerFnError>;
```

Auth-gated by `require_user()`. Empty query short-circuits to `{ hits: [], cursor: None }`.

### 5.3 UI

New `<SearchBar>` component embedded in `pages/files/chrome.rs::TopBar`:
- Text input with placeholder "Search files…"
- Debounced 300ms input → `search_files(q, None)` server-fn call
- Dropdown panel below the input renders up to 10 hits as rows: basename (bold) + path (muted) + mime-derived icon
- Click on a hit navigates to the containing folder via the existing files-router and scrolls/highlights the file
- `Escape` / blur / click-outside closes the dropdown
- Empty state in the dropdown ("No matches.") when query is non-empty but returned 0 hits
- "Type to search" copy when the dropdown is open but input is empty

## 6. Edge cases

| Case | Behavior |
|---|---|
| **Shared file write** | `View::write_file` fires `Modified`; `SearchIndexer` resolves recipients = owner + share recipients (group expanded), UPSERTs one row per recipient. |
| **Share created** | `Shares::create` walks `oc_filecache` under the shared subroot and `Search::fan_out_for_share` UPSERTs one row per `(recipient, fileid)`. For group shares, recipient = each current group member. |
| **Share deleted** | `Shares::delete` walks the same subroot and `Search::fan_out_for_unshare` DELETEs one row per `(former_recipient, fileid)`. The owner's row stays. |
| **Group member added** | No backfill in MVP. New member sees files only after they're re-touched. Documented limitation. |
| **File renamed within owner home** | `Renamed(old_path, new_path)` event; `SearchIndexer` updates the row's `path` + `basename` for every viewer. Recipient `path` fields are re-translated through the share-mount path mapping. |
| **File renamed out of shared subroot** | Owner's path updates; recipient rows DELETEd because the file is no longer visible via the share. |
| **File renamed into a shared subroot** | Owner's path updates; recipient rows UPSERTed because the file now is visible via the share. |
| **File deleted (soft, to trash)** | `Deleted` event fires; `SearchIndexer` DELETEs all viewer rows for that fileid. The trash mounting's `Created` event for the new trash location is filtered (storage_id is the trash storage, indexer skips). |
| **File restored from trash** | The trash sweeper / `Trash::restore` triggers a new write event; SearchIndexer indexes it under the original owner. Recipients re-fanned out if the share still exists. |
| **Public-link write** | `View.uid` is the owner; the write looks normal to `SearchIndexer`. Recipients = owner + share recipients (same as authed write). |
| **Indexer channel full** | `RecvError::Lagged` from the broadcast channel → indexer logs a `tracing::warn!` with the count of dropped events; processing continues with the next received event. Documented; rare under normal load. |
| **Indexer panic** | Per-event work wrapped in `tokio::spawn(async { /* handle */ })` + `JoinError::is_panic()` check, so the supervisor loop in `run()` survives. |
| **Query with no text terms but filters** | Returns empty in MVP (no `WHERE viewer_uid = ? AND mime = ?` path is exposed — only the FTS-match path is). A future iteration can add the filters-only path; documented limitation. |
| **Query parser sees unknown `key:value`** | Token is treated as a bare text term. Logged at `tracing::debug!`. |
| **Per-dialect tokenization divergence** | sqlite uses `unicode61 remove_diacritics 2` (case-insensitive, diacritic-folding, Unicode-aware). mysql FULLTEXT uses InnoDB's default (case-insensitive, configurable stopwords). postgres uses `to_tsvector('simple')` (case-insensitive, no stemming, no stopwords). Documented: identical queries may match slightly different result sets across dialects. |
| **Index size** | One row per (viewer, file). For an instance with 10k users × 1k shared files each = 10M rows; mysql/postgres FULLTEXT/GIN scales. sqlite FTS5 also scales but a single-file deployment with millions of rows starts to feel size pressure. Acceptable for MVP. |

## 7. Testing

- **Unit (`crabcloud-search`)**: query parser splits bare terms + filters + phrases; round-trips edge cases (`mime:image/*`, `modified:>2024-01-01`, quoted `"q3 report"`, unknown `foo:bar` → text term, empty query). Path translation for share-mount viewer paths.
- **E2E sqlite** (FTS5 path): write file → query returns it; multi-token AND match; mime filter narrows; modified filter narrows; size filter narrows; phrase match; empty query returns empty.
- **E2E (`crabcloud-fs::view`)**: write triggers a `Modified` event → indexer eventually indexes; `View::delete` triggers `Deleted` → vanishes from all viewers; `View::rename` updates path; rename out of a shared subroot DELETEs recipient rows; rename in UPSERTs recipient rows.
- **E2E (`crabcloud-sharing`)**: share creation fans out per (recipient, fileid); share deletion reverses; group share fans out per member; group share deletion reverses.
- **E2E share-mount path translation**: Alice shares `/docs` with Bob as `/from-alice`; Bob's row for Alice's `/docs/report.docx` stores `path = "/from-alice/report.docx"`.
- **`SearchIndexer` integration**: end-to-end — build AppState, publish a `Modified` event via `storage_sink`, poll until `state.search.query("alice", parsed, 10, None)` returns the hit (with bounded retry; failure within N seconds = test failure).
- **Indexer panic survival**: inject a bad event; assert indexer doesn't die.
- **OCS e2e**: full query end-to-end via the OCS endpoint; cursor pagination round-trip.
- **Server-fn integration**: round-trip search through the dx fullstack server.
- **UI**: SSR snapshot of search dropdown with seeded hits + empty state + "type to search" state.

## 8. Batches (implementation order)

A. **Core: crate + indexer + share fan-out + query parser** — new `crabcloud-search` crate, migration `0012_search_index` triplet, `Search::{query_parse, query, upsert_for_file, delete_for_file, delete_for_viewer_file, fan_out_for_share, fan_out_for_unshare}`, query parser with bare terms + filter operators + phrases, `SearchIndexer` background task subscribed to `storage_sink` with event-classification dispatch + per-event panic survival, hooks in `Shares::{create, delete}` for bulk fan-out, AppState wiring. Comprehensive unit + e2e + cross-crate tests. **Largest batch — most of the SP weight is here.**
B. **OCS REST** — `/ocs/v2.php/search/providers/files/search` endpoint with cursor pagination. Reuses shared OCS envelope helpers. Tests cover query, filters, pagination.
C. **Server fn + UI top-bar search** — `search_files(query, cursor)` server fn; new `<SearchBar>` component in `pages/files/chrome.rs::TopBar` with debounced input, dropdown panel, click-to-navigate, keyboard handling. SSR snapshot tests + WASM build check.

Each batch ships as one PR through subagent-driven-development with the standard two-stage review (spec compliance → code quality).
