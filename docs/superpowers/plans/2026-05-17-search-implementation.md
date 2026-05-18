# Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship file-metadata search (basename + path + mime + mtime + size filters) across each user's accessible files (home + accepted shares + share-mount paths). Per-user materialized index in `oc_search` (sqlite FTS5 / mysql FULLTEXT / postgres tsvector+GIN), maintained async via the existing `storage_sink` event stream + bulk fan-out hooks in `Shares::{create, delete}`. Query parser supports bare terms + inline filter operators (`mime:image/*`, `modified:>2024-01-01`, `size:>1MB`, quoted phrases). Surfaces: Nextcloud-style OCS unified-search provider endpoint + Dioxus top-bar `<SearchBar>` dropdown.

**Architecture:** New `crabcloud-search` crate owns `Search::{query, query_parse, upsert_for_file, delete_for_file, delete_for_viewer_file, fan_out_for_share, fan_out_for_unshare}` with multidialect SQL dispatched on `DbPool` (sqlite uses FTS5 virtual-table syntax; mysql uses `MATCH() AGAINST() IN NATURAL LANGUAGE MODE`; postgres uses `tsv @@ plainto_tsquery`). `SearchIndexer` background task subscribes to the existing `storage_sink` broadcast channel, resolves recipients via `Shares::recipients_for_fileid` (new helper), and UPSERTs/DELETEs per-viewer rows. Each event handler is wrapped so a single bad event doesn't kill the indexer. Empty queries short-circuit to empty results — no full-table scans.

**Tech Stack:** Rust 1.95, sqlx 0.8 with FTS5 enabled in the sqlite build, axum 0.8, Dioxus 0.7 fullstack. No new external dependencies.

**Spec:** `docs/superpowers/specs/2026-05-17-search-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is its own PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp15/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-search-crate`, `b-ocs`, `c-ui`.

- **Commit cadence:** Commit at every "Commit" step. Each batch lands as one squash-merged PR.

- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- **Established workaround for AppState tests:** Tests building `AppState` set `cfg.filecache.enabled = false`, `cfg.mail.transport = "disabled"`, `cfg.trash_retention_days = 30`, `cfg.versions_retention_disabled = false`, `cfg.activity_retention_days = 365`. SP15 adds no new mandatory config knobs (the search index is unconditional), so no test-fixture additions are required.

- **Pre-existing patterns to mirror:**
  - **Crate shape:** `crates/crabcloud-activity/` (SP14) — focused service crate, multidialect SQL via `match self.pool.as_ref()`, per-dialect inline row decode, error type in `error.rs`, types in `types.rs`.
  - **Background task subscribed to `storage_sink`:** `crates/crabcloud-filecache/src/scanner.rs` is the precedent. It subscribes via `storage_sink.subscribe()`, runs a `loop` over `recv()`, handles `RecvError::Lagged` with a `tracing::warn!` + continue, and survives panics via the task-supervisor pattern. Read it first.
  - **`MailEnqueuer`/`ActivityEmitter` precedent:** `crates/crabcloud-sharing/src/mail.rs` (SP11) and `crates/crabcloud-activity/src/emitter.rs` (SP14) — trait in the implementer crate, emitter crates take `Arc<dyn ...>`. SP15 doesn't strictly need this pattern (the indexer is the consumer, not an emitter from share lifecycle code — see the `SearchFanout` trait note in Task A6 below).
  - **OCS module shape:** `crates/crabcloud-http/src/routes/ocs/activity.rs` (SP14 Batch B) — uses shared `super::envelope::*` helpers + `Extension<AuthContext>`.
  - **Server fn shape:** `crates/crabcloud-app/src/server_fns/activity.rs` (SP14) — `require_user()` extractor, centralized `map_*_err` helper.
  - **Migration triplet:** `migrations/core/0011_activity_and_settings/{sqlite,mysql,postgres}.sql`. Next migration number is `0012`.
  - **UI component embedded in TopBar:** the trash sidebar entry in `pages/files/chrome.rs` is the closest precedent for adding to chrome. The dropdown overlay shape is new — the SP14 banner overlay in `pages/activity.rs` is the nearest visual analog for an absolutely-positioned overlay panel.

---

## File-by-file map

### New crate: `crabcloud-search`

```
crates/crabcloud-search/
├── Cargo.toml
├── src/
│   ├── lib.rs       — re-exports + crate doc
│   ├── error.rs     — SearchError
│   ├── parse.rs     — query parser (bare terms + filter operators + phrases) → SearchQuery
│   ├── service.rs   — Search struct + query / upsert / delete / fan_out methods
│   ├── sql.rs       — multidialect SQL constants
│   └── types.rs     — SearchQuery, SearchHit
└── tests/
    └── search_e2e.rs   — sqlite e2e (parser + write→query + share fan-out + delete + rename + phrase + filter)
```

### New migration

```
migrations/core/0012_search_index/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-search` member.
- `crates/crabcloud-core/Cargo.toml` — adds `crabcloud-search` workspace dep.
- `crates/crabcloud-core/src/search_indexer.rs` (new) — `SearchIndexer::{new, run}`.
- `crates/crabcloud-core/src/lib.rs` — `mod search_indexer;` + re-export.
- `crates/crabcloud-core/src/state.rs` — construct `Search` + spawn `SearchIndexer`; expose `AppState.search`, `AppState.search_indexer_shutdown`. Pass into `SharesConfig`.
- `crates/crabcloud-sharing/Cargo.toml` — adds `crabcloud-search` workspace dep.
- `crates/crabcloud-sharing/src/service.rs` (or wherever `Shares::{create, delete}` live) — call `search.fan_out_for_share(...)` / `fan_out_for_unshare(...)` after the share row is committed. Add `SharesConfig.search: Arc<dyn SearchFanout>` field.
- `crates/crabcloud-sharing/src/service.rs` — new helper `Shares::recipients_for_fileid(fileid) -> Vec<UserId>` (for the indexer's per-event resolve). Returns owner + every direct share recipient + every group-share member; de-dupes.
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-search` workspace dep.
- `crates/crabcloud-http/src/routes/ocs/search.rs` (new) — OCS endpoint.
- `crates/crabcloud-http/src/routes/ocs/mod.rs` — mount `/v2.php/search/providers/files`.
- `crates/crabcloud-app/Cargo.toml` — adds `crabcloud-search` workspace dep.
- `crates/crabcloud-app/src/server_fns/search.rs` (new) — `search_files` server fn.
- `crates/crabcloud-app/src/server_fns/mod.rs` — `pub mod search;` + re-export of DTOs.
- `crates/crabcloud-app/src/lib.rs` — re-export DTOs for UI consumption.
- `crates/crabcloud-app/src/pages/files/chrome.rs` — add `<SearchBar>` into the `TopBar` component.
- `crates/crabcloud-app/src/pages/files/search_bar.rs` (new) — the component itself + dropdown overlay.
- `crates/crabcloud-app/assets/app.css` — `.search-bar*` and `.search-dropdown*` styles (~80 lines).

---

# Batch A — `crabcloud-search` core + indexer + share fan-out + query parser

**Branch:** `sp15/a-search-crate`

**Goal:** Stand up the search crate, the 0012 migration, the `SearchIndexer` background task subscribed to `storage_sink`, fan-out hooks in `Shares::{create, delete}`, and the query parser. AppState wires everything together.

After this batch:
- New file writes (via `View::write_file`/etc.) eventually appear in the per-user `oc_search` index.
- Share lifecycle events (create / delete / group fan-out) populate / remove per-viewer rows.
- File rename / delete propagate through the indexer.
- `Search::query("alice", parse("q3 mime:image/*"), 10, None)` returns the right hits.
- No surface yet — OCS + UI land in B + C.

### Task A1: Migration `0012_search_index`

**Files:**
- Create: `migrations/core/0012_search_index/sqlite.sql`
- Create: `migrations/core/0012_search_index/mysql.sql`
- Create: `migrations/core/0012_search_index/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs` (or wherever `core_set()` lives — grep)

- [ ] **Step 1: Confirm migration registration pattern**

  Read the existing `0011_activity_and_settings` registration in the core migrations file. New entry registers identically (next sequence number = 12).

- [ ] **Step 2: Write `sqlite.sql`**

  ```sql
  CREATE VIRTUAL TABLE oc_search USING fts5 (
      viewer_uid UNINDEXED,
      fileid     UNINDEXED,
      storage_id UNINDEXED,
      basename,
      path,
      mime       UNINDEXED,
      mtime      UNINDEXED,
      size       UNINDEXED,
      tokenize = 'unicode61 remove_diacritics 2'
  );
  ```

  Note: FTS5 doesn't accept a separate `CREATE INDEX`. The `UNINDEXED` columns are still stored and `WHERE viewer_uid = ?` works (linear over the small viewer-partition for that uid via the rowid). The composite `(viewer_uid, fileid)` uniqueness is enforced by the indexer at write time (DELETE-then-INSERT pattern), not by a SQL constraint.

- [ ] **Step 3: Write `mysql.sql`**

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

- [ ] **Step 4: Write `postgres.sql`**

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

  CREATE INDEX idx_search_viewer ON oc_search (viewer_uid);
  CREATE INDEX idx_search_tsv    ON oc_search USING GIN (tsv);
  ```

- [ ] **Step 5: Register in core migrations**

  Add the new directory entry to `core_set()` mirroring the 0011 registration.

- [ ] **Step 6: Verify migration runs**

  ```bash
  cargo test -p crabcloud-db
  ```

  Expected: all migration tests pass; the new 0012 directory is registered.

- [ ] **Step 7: Commit**

  ```bash
  git add migrations/core/0012_search_index crates/crabcloud-db/src/core_migrations.rs
  git commit -m "search: 0012_search_index migration triplet (sqlite FTS5 / mysql FULLTEXT / pg tsvector+GIN)"
  ```

### Task A2: Crate skeleton

**Files:**
- Create: `crates/crabcloud-search/Cargo.toml`
- Create: `crates/crabcloud-search/src/lib.rs`
- Create: `crates/crabcloud-search/src/error.rs`
- Create: `crates/crabcloud-search/src/types.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Register the crate in the workspace**

  In root `Cargo.toml`:
  - Add `"crates/crabcloud-search",` to `members`.
  - Add to `[workspace.dependencies]`:
    ```toml
    crabcloud-search = { path = "crates/crabcloud-search" }
    ```

- [ ] **Step 2: Write `Cargo.toml`**

  Mirror `crates/crabcloud-activity/Cargo.toml`. Same dep set.

  ```toml
  [package]
  name = "crabcloud-search"
  version.workspace = true
  edition.workspace = true
  license.workspace = true

  [dependencies]
  async-trait = { workspace = true }
  chrono = { workspace = true }
  crabcloud-db = { workspace = true }
  crabcloud-users = { workspace = true }
  serde = { workspace = true }
  serde_json = { workspace = true }
  sqlx = { workspace = true }
  thiserror = { workspace = true }
  tokio = { workspace = true, features = ["sync"] }
  tracing = { workspace = true }

  [dev-dependencies]
  crabcloud-config = { workspace = true }
  tempfile = { workspace = true }
  tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
  ```

- [ ] **Step 3: Write `src/lib.rs`**

  ```rust
  //! File-metadata search service for Crabcloud.
  //!
  //! Spec: `docs/superpowers/specs/2026-05-17-search-design.md`.
  //!
  //! Public entry points: [`Search`] (query + write API), [`SearchFanout`]
  //! (trait used by `crabcloud-sharing` to drive share-lifecycle fan-out),
  //! and the value types in [`types`]. SQL dispatch mirrors the
  //! `crabcloud-activity` / `crabcloud-versions` pattern.

  mod error;
  mod parse;
  mod service;
  mod sql;
  mod types;

  pub use error::SearchError;
  pub use parse::parse_query;
  pub use service::{Search, SearchFanout};
  pub use types::{SearchHit, SearchQuery};
  ```

- [ ] **Step 4: Write `src/error.rs`**

  ```rust
  use thiserror::Error;

  #[derive(Debug, Error)]
  pub enum SearchError {
      #[error("db: {0}")]
      Db(#[from] sqlx::Error),
  }
  ```

- [ ] **Step 5: Write `src/types.rs`**

  ```rust
  //! Public-facing value types for the search service.

  use serde::{Deserialize, Serialize};

  /// Parsed user query. The text part feeds the FTS match; the filters
  /// become AND clauses on the SQL side.
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub struct SearchQuery {
      pub text: String,                // bare-tokens joined by space (FTS match input)
      pub phrase: Option<String>,      // quoted phrase, if any
      pub mime: Option<String>,        // mime glob ("image/*" or "application/pdf")
      pub modified_after: Option<i64>, // unix seconds
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

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
  pub struct SearchHit {
      pub fileid: i64,
      pub storage_id: i64,
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
  ```

- [ ] **Step 6: Stub `src/service.rs`, `src/sql.rs`, `src/parse.rs`**

  Minimal stubs so the crate compiles in this step:

  ```rust
  // src/sql.rs
  //! Multidialect SQL constants. Filled in Task A3.
  ```

  ```rust
  // src/parse.rs
  //! Query parser. Filled in Task A4.

  use crate::types::SearchQuery;

  pub fn parse_query(_input: &str) -> SearchQuery {
      SearchQuery::default()
  }
  ```

  ```rust
  // src/service.rs
  //! Search service. Filled in Task A5.

  use crate::error::SearchError;
  use crate::types::{SearchHit, SearchQuery};
  use async_trait::async_trait;
  use crabcloud_db::DbPool;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Search {
      #[allow(dead_code)]
      pool: Arc<DbPool>,
  }

  impl Search {
      pub fn new(pool: Arc<DbPool>) -> Self {
          Self { pool }
      }

      pub async fn query(
          &self,
          _viewer_uid: &str,
          _q: &SearchQuery,
          _limit: i64,
          _cursor: Option<(f64, i64)>,
      ) -> Result<Vec<SearchHit>, SearchError> {
          Ok(Vec::new())
      }
  }

  /// Trait that `crabcloud-sharing` depends on so it can drive bulk
  /// fan-out at share lifecycle events without taking a hard dep on
  /// `crabcloud-search`. `Search` itself impls this.
  #[async_trait]
  pub trait SearchFanout: Send + Sync {
      async fn fan_out_for_share(
          &self,
          recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
          recipient_path_prefix: &str,
      ) -> Result<(), SearchError>;

      async fn fan_out_for_unshare(
          &self,
          former_recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
      ) -> Result<(), SearchError>;
  }

  #[async_trait]
  impl SearchFanout for Search {
      async fn fan_out_for_share(
          &self,
          _recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
          _recipient_path_prefix: &str,
      ) -> Result<(), SearchError> {
          Ok(())
      }

      async fn fan_out_for_unshare(
          &self,
          _former_recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
      ) -> Result<(), SearchError> {
          Ok(())
      }
  }
  ```

- [ ] **Step 7: Build**

  ```bash
  cargo build -p crabcloud-search
  ```

  Expected: clean.

- [ ] **Step 8: Commit**

  ```bash
  git add Cargo.toml crates/crabcloud-search/
  git commit -m "search: crate skeleton (types + error + lib facade + stub service)"
  ```

### Task A3: Multidialect SQL constants

**Files:**
- Modify: `crates/crabcloud-search/src/sql.rs`

- [ ] **Step 1: Write the constants**

  ```rust
  //! Multidialect SQL constants for the search service.
  //!
  //! Per-dialect because the full-text mechanism differs substantially:
  //!   - sqlite: FTS5 virtual table; `MATCH ?` syntax with FTS5 query string
  //!   - mysql: `MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE)`
  //!   - postgres: `tsv @@ plainto_tsquery('simple', ?)` with `ts_rank_cd`

  // -- UPSERT a row for one (viewer, file). sqlite FTS5 has no UPSERT;
  //    indexer does DELETE-then-INSERT inside a transaction. mysql + pg
  //    use their native ON DUPLICATE / ON CONFLICT.
  //    The sqlite "DELETE then INSERT" lives in service.rs (not here)
  //    because it needs to dispatch on the pool dialect.

  pub const DELETE_VIEWER_FILE_QM: &str =
      "DELETE FROM oc_search WHERE viewer_uid = ? AND fileid = ?";
  pub const DELETE_VIEWER_FILE_PG: &str =
      "DELETE FROM oc_search WHERE viewer_uid = $1 AND fileid = $2";

  pub const INSERT_QM: &str =
      "INSERT INTO oc_search \
       (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

  pub const INSERT_MYSQL_UPSERT: &str =
      "INSERT INTO oc_search \
       (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
       VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
       ON DUPLICATE KEY UPDATE \
         storage_id = VALUES(storage_id), basename = VALUES(basename), \
         path = VALUES(path), mime = VALUES(mime), \
         mtime = VALUES(mtime), size = VALUES(size)";

  pub const INSERT_PG_UPSERT: &str =
      "INSERT INTO oc_search \
       (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
       ON CONFLICT (viewer_uid, fileid) DO UPDATE SET \
         storage_id = EXCLUDED.storage_id, basename = EXCLUDED.basename, \
         path = EXCLUDED.path, mime = EXCLUDED.mime, \
         mtime = EXCLUDED.mtime, size = EXCLUDED.size";

  // -- DELETE all rows for one fileid (every viewer). Used when the file
  //    is hard-deleted or moved to trash.
  pub const DELETE_FILEID_QM: &str =
      "DELETE FROM oc_search WHERE fileid = ?";
  pub const DELETE_FILEID_PG: &str =
      "DELETE FROM oc_search WHERE fileid = $1";

  // -- DELETE rows for one (viewer, fileid). Used by fan_out_for_unshare
  //    per file.
  // (Same as DELETE_VIEWER_FILE_*.)

  // -- QUERY: per-dialect; substituted by service.rs based on whether
  //    the parsed query has filters (we splice WHERE clauses dynamically).
  //    These are the BASE query templates; the indexer builds the final
  //    SQL by appending AND clauses.
  //
  // sqlite (FTS5): the `MATCH ?` predicate goes on oc_search and the
  // UNINDEXED columns are part of the same virtual table. Rank via the
  // `bm25(oc_search)` function (lower = better; we ORDER BY rank ASC).
  pub const QUERY_BASE_SQLITE: &str =
      "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
              bm25(oc_search) AS rank \
       FROM oc_search \
       WHERE viewer_uid = ? AND oc_search MATCH ?";

  // mysql: NATURAL LANGUAGE MODE. Rank via the MATCH score (higher = better);
  // we ORDER BY rank DESC.
  pub const QUERY_BASE_MYSQL: &str =
      "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
              MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) AS rank \
       FROM oc_search \
       WHERE viewer_uid = ? \
         AND MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE)";

  // postgres: @@ plainto_tsquery + ts_rank_cd.
  pub const QUERY_BASE_PG: &str =
      "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
              ts_rank_cd(tsv, plainto_tsquery('simple', $1)) AS rank \
       FROM oc_search \
       WHERE viewer_uid = $2 \
         AND tsv @@ plainto_tsquery('simple', $1)";

  // -- FAN-OUT: insert one (viewer, fileid, ...) row per recipient per
  //    file under the shared subroot. The indexer SELECTs from oc_filecache
  //    + oc_storages for the owner, then INSERTs one row per recipient.
  //    The actual queries live in service.rs because they're parameterized
  //    on both DbPool dialect and the recipient list.
  ```

- [ ] **Step 2: Build**

  ```bash
  cargo build -p crabcloud-search
  ```

  Expected: clean.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-search/src/sql.rs
  git commit -m "search: multidialect SQL constants (UPSERT/DELETE/QUERY base templates)"
  ```

### Task A4: Query parser — TDD

**Files:**
- Modify: `crates/crabcloud-search/src/parse.rs`

- [ ] **Step 1: Write the parser tests (RED)**

  Replace the stub with:

  ```rust
  //! Query parser.
  //!
  //! Splits a free-text query into:
  //!  - `text`: bare tokens (joined by space) for FTS match
  //!  - `phrase`: quoted phrase, if any (at most one in MVP)
  //!  - filter operators: `mime:<glob>`, `modified:>EPOCH|ISO|YYYY-MM-DD`,
  //!    `modified:<...`, `modified:YYYY-MM-DD..YYYY-MM-DD`,
  //!    `size:>N{B,KB,MB,GB,TB}`, `size:<N...`
  //!  - Unknown `key:value` → bare text term (graceful degradation)

  use crate::types::SearchQuery;

  /// Parse the user-supplied query into a structured [`SearchQuery`].
  /// The grammar is forgiving: malformed filters degrade to text terms.
  pub fn parse_query(input: &str) -> SearchQuery {
      let mut q = SearchQuery::default();
      let mut text_parts: Vec<&str> = Vec::new();

      // Phrase extraction: a single "..."-quoted run.
      let (phrase, rest) = extract_phrase(input);
      q.phrase = phrase;

      for tok in tokenize(&rest) {
          if let Some((key, value)) = tok.split_once(':') {
              if !apply_filter(&mut q, key, value) {
                  tracing::debug!(unknown_filter = %tok, "search parser: unknown key:value, treating as text term");
                  text_parts.push(tok);
              }
          } else {
              text_parts.push(tok);
          }
      }
      q.text = text_parts.join(" ");
      q
  }

  fn extract_phrase(input: &str) -> (Option<String>, String) {
      // Find the first balanced "..." run; everything else is `rest`.
      let bytes = input.as_bytes();
      let mut start = None;
      for (i, &b) in bytes.iter().enumerate() {
          if b == b'"' {
              start = Some(i);
              break;
          }
      }
      let Some(s) = start else {
          return (None, input.to_string());
      };
      let after = &input[s + 1..];
      if let Some(end_rel) = after.find('"') {
          let phrase = after[..end_rel].to_string();
          let mut rest = String::with_capacity(input.len() - phrase.len() - 2);
          rest.push_str(&input[..s]);
          rest.push_str(&after[end_rel + 1..]);
          (Some(phrase), rest)
      } else {
          // Unterminated quote — treat the rest as text, no phrase.
          (None, input.to_string())
      }
  }

  fn tokenize(s: &str) -> impl Iterator<Item = &str> {
      s.split_whitespace().filter(|t| !t.is_empty())
  }

  fn apply_filter(q: &mut SearchQuery, key: &str, value: &str) -> bool {
      match key {
          "mime" => {
              q.mime = Some(value.to_string());
              true
          }
          "modified" => parse_modified_filter(q, value),
          "size" => parse_size_filter(q, value),
          _ => false,
      }
  }

  fn parse_modified_filter(q: &mut SearchQuery, value: &str) -> bool {
      // `>EPOCH`, `>YYYY-MM-DD`, `<EPOCH`, `<YYYY-MM-DD`, `YYYY-MM-DD..YYYY-MM-DD`
      if let Some(rest) = value.strip_prefix('>') {
          if let Some(ts) = parse_epoch_or_iso(rest) {
              q.modified_after = Some(ts);
              return true;
          }
          return false;
      }
      if let Some(rest) = value.strip_prefix('<') {
          if let Some(ts) = parse_epoch_or_iso(rest) {
              q.modified_before = Some(ts);
              return true;
          }
          return false;
      }
      if let Some((a, b)) = value.split_once("..") {
          let (Some(a_ts), Some(b_ts)) = (parse_epoch_or_iso(a), parse_epoch_or_iso(b)) else {
              return false;
          };
          q.modified_after = Some(a_ts);
          q.modified_before = Some(b_ts);
          return true;
      }
      false
  }

  fn parse_epoch_or_iso(s: &str) -> Option<i64> {
      if let Ok(n) = s.parse::<i64>() {
          return Some(n);
      }
      // YYYY-MM-DD only (no time-of-day in MVP).
      let dt = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
      dt.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc().timestamp())
  }

  fn parse_size_filter(q: &mut SearchQuery, value: &str) -> bool {
      if let Some(rest) = value.strip_prefix('>') {
          if let Some(n) = parse_size(rest) {
              q.size_min = Some(n);
              return true;
          }
          return false;
      }
      if let Some(rest) = value.strip_prefix('<') {
          if let Some(n) = parse_size(rest) {
              q.size_max = Some(n);
              return true;
          }
          return false;
      }
      false
  }

  fn parse_size(s: &str) -> Option<i64> {
      let bytes = s.as_bytes();
      let mut split = bytes.len();
      while split > 0 && !bytes[split - 1].is_ascii_digit() {
          split -= 1;
      }
      let (num_str, unit) = s.split_at(split);
      let n: i64 = num_str.parse().ok()?;
      let mult: i64 = match unit.to_ascii_uppercase().as_str() {
          "" | "B" => 1,
          "KB" => 1024,
          "MB" => 1024 * 1024,
          "GB" => 1024 * 1024 * 1024,
          "TB" => 1024_i64 * 1024 * 1024 * 1024,
          _ => return None,
      };
      Some(n * mult)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn bare_tokens_become_text() {
          let q = parse_query("q3 report");
          assert_eq!(q.text, "q3 report");
          assert!(q.phrase.is_none());
      }

      #[test]
      fn mime_filter_lifts_out() {
          let q = parse_query("q3 mime:image/*");
          assert_eq!(q.text, "q3");
          assert_eq!(q.mime.as_deref(), Some("image/*"));
      }

      #[test]
      fn modified_gt_iso_lifts_out() {
          let q = parse_query("modified:>2024-01-01 design");
          assert_eq!(q.text, "design");
          let want = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
              .unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
          assert_eq!(q.modified_after, Some(want));
      }

      #[test]
      fn modified_gt_epoch_lifts_out() {
          let q = parse_query("modified:>1700000000");
          assert_eq!(q.text, "");
          assert_eq!(q.modified_after, Some(1700000000));
      }

      #[test]
      fn modified_range_lifts_both() {
          let q = parse_query("modified:2024-01-01..2024-12-31");
          let lo = chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap().and_hms_opt(0,0,0).unwrap().and_utc().timestamp();
          let hi = chrono::NaiveDate::from_ymd_opt(2024, 12, 31).unwrap().and_hms_opt(0,0,0).unwrap().and_utc().timestamp();
          assert_eq!(q.modified_after, Some(lo));
          assert_eq!(q.modified_before, Some(hi));
      }

      #[test]
      fn size_gt_mb_lifts_out() {
          let q = parse_query("size:>1MB photo");
          assert_eq!(q.text, "photo");
          assert_eq!(q.size_min, Some(1024 * 1024));
      }

      #[test]
      fn size_lt_kb_lifts_out() {
          let q = parse_query("size:<10KB");
          assert_eq!(q.size_max, Some(10 * 1024));
      }

      #[test]
      fn phrase_extracts() {
          let q = parse_query("alice \"q3 report\" mime:application/pdf");
          assert_eq!(q.phrase.as_deref(), Some("q3 report"));
          assert_eq!(q.text, "alice");
          assert_eq!(q.mime.as_deref(), Some("application/pdf"));
      }

      #[test]
      fn unterminated_phrase_falls_through() {
          let q = parse_query("\"unterminated text");
          assert!(q.phrase.is_none());
          assert_eq!(q.text, "\"unterminated text");
      }

      #[test]
      fn unknown_key_falls_back_to_text() {
          let q = parse_query("foo:bar baz");
          assert_eq!(q.text, "foo:bar baz");
          assert!(q.mime.is_none());
      }

      #[test]
      fn empty_query_is_empty() {
          let q = parse_query("");
          assert!(q.is_empty());
          assert!(!q.has_text_match());
      }

      #[test]
      fn filters_only_is_not_empty_but_no_text_match() {
          let q = parse_query("mime:image/*");
          assert!(!q.is_empty());
          assert!(!q.has_text_match());
      }
  }
  ```

- [ ] **Step 2: Run tests**

  ```bash
  cargo test -p crabcloud-search parse
  ```

  Expected: 12 tests pass.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-search/src/parse.rs
  git commit -m "search: query parser (bare terms + mime/modified/size filters + phrases)"
  ```

### Task A5: `Search` service — TDD with sqlite e2e

**Files:**
- Modify: `crates/crabcloud-search/src/service.rs`
- Create: `crates/crabcloud-search/tests/search_e2e.rs`

This is the bulk of Batch A.

- [ ] **Step 1: Write the e2e test file (RED)**

  Create `crates/crabcloud-search/tests/search_e2e.rs`:

  ```rust
  //! sqlite e2e for the Search service.

  use crabcloud_config::test_support::minimal_sqlite_config;
  use crabcloud_db::{core_set, DbPool, MigrationRunner};
  use crabcloud_search::{parse_query, Search, SearchHit};
  use crabcloud_users::UserId;
  use std::sync::Arc;
  use tempfile::TempDir;

  async fn setup() -> (Arc<DbPool>, TempDir) {
      let db_dir = TempDir::new().unwrap();
      let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
      let pool = DbPool::connect(&cfg).await.unwrap();
      let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
      runner.register(core_set());
      runner.run().await.unwrap();
      (Arc::new(pool), db_dir)
  }

  fn uid(s: &str) -> UserId {
      UserId::new(s).unwrap()
  }

  #[tokio::test]
  async fn empty_query_returns_empty() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      let hits = search.query("alice", &parse_query(""), 10, None).await.unwrap();
      assert!(hits.is_empty());
  }

  #[tokio::test]
  async fn upsert_then_query_returns_hit() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file(
          "alice", /*fileid*/ 100, /*storage_id*/ 1,
          "report.docx", "/docs/report.docx",
          "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
          1_700_000_000, 12345,
      ).await.unwrap();

      let hits = search.query("alice", &parse_query("report"), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].fileid, 100);
      assert_eq!(hits[0].basename, "report.docx");
      assert_eq!(hits[0].path, "/docs/report.docx");
  }

  #[tokio::test]
  async fn query_filters_by_mime() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "photo.jpg", "/pics/photo.jpg", "image/jpeg", 1_700_000_000, 200_000).await.unwrap();
      search.upsert_for_file("alice", 101, 1, "report.docx", "/docs/report.docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", 1_700_000_000, 12345).await.unwrap();

      // Match by tokens but filter to images only.
      let hits = search.query("alice", &parse_query("photo mime:image/*"), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].fileid, 100);
  }

  #[tokio::test]
  async fn query_filters_by_modified_range() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "old.txt", "/o/old.txt", "text/plain", 1_500_000_000, 1).await.unwrap();
      search.upsert_for_file("alice", 101, 1, "new.txt", "/o/new.txt", "text/plain", 1_700_000_000, 1).await.unwrap();

      let hits = search.query("alice", &parse_query("txt modified:>1600000000"), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].fileid, 101);
  }

  #[tokio::test]
  async fn query_filters_by_size_min() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "small.bin", "/s/small.bin", "application/octet-stream", 1_700_000_000, 500).await.unwrap();
      search.upsert_for_file("alice", 101, 1, "big.bin",   "/s/big.bin",   "application/octet-stream", 1_700_000_000, 5_000_000).await.unwrap();

      let hits = search.query("alice", &parse_query("bin size:>1MB"), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].fileid, 101);
  }

  #[tokio::test]
  async fn query_isolates_per_viewer() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "report.docx", "/docs/report.docx", "application/octet-stream", 1_700_000_000, 1).await.unwrap();
      let alice_hits = search.query("alice", &parse_query("report"), 10, None).await.unwrap();
      let bob_hits   = search.query("bob",   &parse_query("report"), 10, None).await.unwrap();
      assert_eq!(alice_hits.len(), 1);
      assert!(bob_hits.is_empty());
  }

  #[tokio::test]
  async fn upsert_updates_existing_row() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "old.txt", "/x/old.txt", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.upsert_for_file("alice", 100, 1, "new.txt", "/x/new.txt", "text/plain", 1_700_000_100, 2).await.unwrap();
      let hits = search.query("alice", &parse_query("new"), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].basename, "new.txt");
      // Old-named hit no longer matches.
      let stale = search.query("alice", &parse_query("old"), 10, None).await.unwrap();
      assert!(stale.is_empty());
  }

  #[tokio::test]
  async fn delete_for_file_removes_all_viewers() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "x.txt", "/x.txt", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.upsert_for_file("bob",   100, 1, "x.txt", "/x.txt", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.delete_for_file(100).await.unwrap();
      assert!(search.query("alice", &parse_query("x"), 10, None).await.unwrap().is_empty());
      assert!(search.query("bob",   &parse_query("x"), 10, None).await.unwrap().is_empty());
  }

  #[tokio::test]
  async fn delete_for_viewer_file_targets_one_row() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "x.txt", "/x.txt", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.upsert_for_file("bob",   100, 1, "x.txt", "/x.txt", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.delete_for_viewer_file("bob", 100).await.unwrap();
      assert!(!search.query("alice", &parse_query("x"), 10, None).await.unwrap().is_empty());
      assert!(search.query("bob",    &parse_query("x"), 10, None).await.unwrap().is_empty());
  }

  #[tokio::test]
  async fn query_pagination_cursor() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      for i in 0..5 {
          search.upsert_for_file(
              "alice", 100 + i, 1,
              &format!("rpt-{i}.txt"), &format!("/r/rpt-{i}.txt"),
              "text/plain", 1_700_000_000 + i, 1,
          ).await.unwrap();
      }
      let page1 = search.query("alice", &parse_query("rpt"), 2, None).await.unwrap();
      assert_eq!(page1.len(), 2);
      let cursor = (page1.last().unwrap().rank, page1.last().unwrap().fileid);
      let page2 = search.query("alice", &parse_query("rpt"), 2, Some(cursor)).await.unwrap();
      assert_eq!(page2.len(), 2);
      // No overlap between page1 and page2.
      let p1_ids: std::collections::HashSet<_> = page1.iter().map(|h| h.fileid).collect();
      for h in &page2 {
          assert!(!p1_ids.contains(&h.fileid));
      }
  }

  #[tokio::test]
  async fn fan_out_for_share_inserts_recipient_rows() {
      use crabcloud_search::SearchFanout;
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      // Seed alice's row first.
      search.upsert_for_file("alice", 100, 1, "report.docx", "/docs/report.docx", "application/octet-stream", 1_700_000_000, 1).await.unwrap();
      // Now share /docs with bob; the fan-out helper needs to be told
      // which fileids fall under /docs and what bob's recipient prefix is.
      // For unit testing we exercise the lower-level upsert directly:
      search.upsert_for_file("bob", 100, 1, "report.docx", "/from-alice/report.docx", "application/octet-stream", 1_700_000_000, 1).await.unwrap();
      let alice_hits = search.query("alice", &parse_query("report"), 10, None).await.unwrap();
      let bob_hits   = search.query("bob",   &parse_query("report"), 10, None).await.unwrap();
      assert_eq!(alice_hits.len(), 1);
      assert_eq!(bob_hits.len(), 1);
      assert_eq!(bob_hits[0].path, "/from-alice/report.docx");
      let _ = search.fan_out_for_share(vec![uid("carol")], "alice", "/docs", "/from-alice").await; // Carol shares not implemented in unit test
  }

  #[tokio::test]
  async fn phrase_query_matches_adjacent_tokens() {
      let (pool, _d) = setup().await;
      let search = Search::new(pool);
      search.upsert_for_file("alice", 100, 1, "q3 report.docx", "/q3 report.docx", "text/plain", 1_700_000_000, 1).await.unwrap();
      search.upsert_for_file("alice", 101, 1, "report q3.docx", "/report q3.docx", "text/plain", 1_700_000_000, 1).await.unwrap();
      let hits = search.query("alice", &parse_query("\"q3 report\""), 10, None).await.unwrap();
      assert_eq!(hits.len(), 1);
      assert_eq!(hits[0].fileid, 100);
  }
  ```

- [ ] **Step 2: Run the test (RED)**

  ```bash
  cargo test -p crabcloud-search --test search_e2e
  ```

  Expected: compile failures on `Search::upsert_for_file`, `delete_for_file`, `delete_for_viewer_file`, and the rich `query` body.

- [ ] **Step 3: Implement `src/service.rs`**

  Replace the stub with the full service. The sqlite path uses DELETE-then-INSERT in a transaction for UPSERT (FTS5 doesn't support `ON CONFLICT`). The query path builds the SQL by appending filter AND clauses to the base template per dialect.

  ```rust
  //! `Search` — query / upsert / delete + SearchFanout impl.
  //!
  //! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
  //! Per-dialect dispatch via `match self.pool.as_ref()`. sqlite uses
  //! FTS5; mysql uses FULLTEXT NATURAL LANGUAGE MODE; postgres uses
  //! tsvector + plainto_tsquery + ts_rank_cd.

  use crate::error::SearchError;
  use crate::sql;
  use crate::types::{SearchHit, SearchQuery};
  use async_trait::async_trait;
  use crabcloud_db::DbPool;
  use sqlx::Row as _;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Search {
      pool: Arc<DbPool>,
  }

  impl Search {
      pub fn new(pool: Arc<DbPool>) -> Self {
          Self { pool }
      }

      pub async fn upsert_for_file(
          &self,
          viewer_uid: &str,
          fileid: i64,
          storage_id: i64,
          basename: &str,
          path: &str,
          mime: &str,
          mtime: i64,
          size: i64,
      ) -> Result<(), SearchError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => {
                  // FTS5 has no UPSERT; DELETE-then-INSERT inside a tx.
                  let mut tx = p.begin().await?;
                  sqlx::query(sql::DELETE_VIEWER_FILE_QM)
                      .bind(viewer_uid).bind(fileid)
                      .execute(&mut *tx).await?;
                  sqlx::query(sql::INSERT_QM)
                      .bind(viewer_uid).bind(fileid).bind(storage_id)
                      .bind(basename).bind(path).bind(mime)
                      .bind(mtime).bind(size)
                      .execute(&mut *tx).await?;
                  tx.commit().await?;
              }
              DbPool::MySql(p) => {
                  sqlx::query(sql::INSERT_MYSQL_UPSERT)
                      .bind(viewer_uid).bind(fileid).bind(storage_id)
                      .bind(basename).bind(path).bind(mime)
                      .bind(mtime).bind(size)
                      .execute(p).await?;
              }
              DbPool::Postgres(p) => {
                  sqlx::query(sql::INSERT_PG_UPSERT)
                      .bind(viewer_uid).bind(fileid).bind(storage_id)
                      .bind(basename).bind(path).bind(mime)
                      .bind(mtime).bind(size)
                      .execute(p).await?;
              }
          }
          Ok(())
      }

      pub async fn delete_for_file(&self, fileid: i64) -> Result<(), SearchError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => { sqlx::query(sql::DELETE_FILEID_QM).bind(fileid).execute(p).await?; }
              DbPool::MySql(p) => { sqlx::query(sql::DELETE_FILEID_QM).bind(fileid).execute(p).await?; }
              DbPool::Postgres(p) => { sqlx::query(sql::DELETE_FILEID_PG).bind(fileid).execute(p).await?; }
          }
          Ok(())
      }

      pub async fn delete_for_viewer_file(
          &self,
          viewer_uid: &str,
          fileid: i64,
      ) -> Result<(), SearchError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => { sqlx::query(sql::DELETE_VIEWER_FILE_QM).bind(viewer_uid).bind(fileid).execute(p).await?; }
              DbPool::MySql(p) => { sqlx::query(sql::DELETE_VIEWER_FILE_QM).bind(viewer_uid).bind(fileid).execute(p).await?; }
              DbPool::Postgres(p) => { sqlx::query(sql::DELETE_VIEWER_FILE_PG).bind(viewer_uid).bind(fileid).execute(p).await?; }
          }
          Ok(())
      }

      pub async fn query(
          &self,
          viewer_uid: &str,
          q: &SearchQuery,
          limit: i64,
          cursor: Option<(f64, i64)>,
      ) -> Result<Vec<SearchHit>, SearchError> {
          if q.is_empty() || !q.has_text_match() {
              return Ok(Vec::new());
          }
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => self.query_sqlite(p, viewer_uid, q, limit, cursor).await,
              DbPool::MySql(p)  => self.query_mysql(p,  viewer_uid, q, limit, cursor).await,
              DbPool::Postgres(p) => self.query_pg(p,   viewer_uid, q, limit, cursor).await,
          }
      }

      async fn query_sqlite(
          &self,
          pool: &sqlx::SqlitePool,
          viewer_uid: &str,
          q: &SearchQuery,
          limit: i64,
          cursor: Option<(f64, i64)>,
      ) -> Result<Vec<SearchHit>, SearchError> {
          // Build the FTS5 MATCH expression: phrase comes first as a quoted
          // string (FTS5 syntax: "x y"), then bare tokens AND'd by space.
          let mut match_expr = String::new();
          if let Some(ph) = &q.phrase {
              match_expr.push_str(&format!("\"{}\"", escape_fts5(ph)));
          }
          if !q.text.is_empty() {
              if !match_expr.is_empty() {
                  match_expr.push(' ');
              }
              match_expr.push_str(&q.text);
          }
          let mut sql = String::from(sql::QUERY_BASE_SQLITE);
          let mut bind_mime = None;
          let mut bind_after = None;
          let mut bind_before = None;
          let mut bind_size_min = None;
          let mut bind_size_max = None;
          if let Some(m) = &q.mime {
              sql.push_str(" AND mime GLOB ?");
              bind_mime = Some(m.clone());
          }
          if let Some(t) = q.modified_after {
              sql.push_str(" AND mtime >= ?");
              bind_after = Some(t);
          }
          if let Some(t) = q.modified_before {
              sql.push_str(" AND mtime <= ?");
              bind_before = Some(t);
          }
          if let Some(n) = q.size_min {
              sql.push_str(" AND size >= ?");
              bind_size_min = Some(n);
          }
          if let Some(n) = q.size_max {
              sql.push_str(" AND size <= ?");
              bind_size_max = Some(n);
          }
          let (cursor_rank, cursor_id) = match cursor {
              Some((r, id)) => (Some(r), Some(id)),
              None => (None, None),
          };
          if cursor_rank.is_some() {
              // sqlite bm25 lower = better; pagination = `(rank, fileid)` strictly after the cursor.
              sql.push_str(" AND (bm25(oc_search) > ? OR (bm25(oc_search) = ? AND fileid > ?))");
          }
          sql.push_str(" ORDER BY bm25(oc_search) ASC, fileid ASC LIMIT ?");

          let mut query = sqlx::query(&sql).bind(viewer_uid).bind(&match_expr);
          if let Some(m) = bind_mime { query = query.bind(m); }
          if let Some(t) = bind_after { query = query.bind(t); }
          if let Some(t) = bind_before { query = query.bind(t); }
          if let Some(n) = bind_size_min { query = query.bind(n); }
          if let Some(n) = bind_size_max { query = query.bind(n); }
          if let Some(r) = cursor_rank {
              query = query.bind(r).bind(r).bind(cursor_id.unwrap());
          }
          query = query.bind(limit);

          let rows = query.fetch_all(pool).await?;
          rows.iter().map(row_to_hit).collect()
      }

      async fn query_mysql(
          &self,
          pool: &sqlx::MySqlPool,
          viewer_uid: &str,
          q: &SearchQuery,
          limit: i64,
          cursor: Option<(f64, i64)>,
      ) -> Result<Vec<SearchHit>, SearchError> {
          // mysql NATURAL LANGUAGE MODE: no phrase syntax in NLM, so a
          // phrase falls back to bare-tokens AND (BOOLEAN MODE would let
          // us do quoted phrases, but its tokenizer rules differ enough
          // that we accept the soft-coalesce for MVP).
          let mut match_text = q.text.clone();
          if let Some(ph) = &q.phrase {
              if !match_text.is_empty() {
                  match_text.push(' ');
              }
              match_text.push_str(ph);
          }

          let mut sql = String::from(sql::QUERY_BASE_MYSQL);
          // base template binds: rank-input, viewer, match-text
          let mut bind_mime = None;
          let mut bind_after = None;
          let mut bind_before = None;
          let mut bind_size_min = None;
          let mut bind_size_max = None;
          if let Some(m) = &q.mime {
              sql.push_str(" AND mime LIKE ?");
              bind_mime = Some(m.replace('*', "%"));
          }
          if let Some(t) = q.modified_after {
              sql.push_str(" AND mtime >= ?");
              bind_after = Some(t);
          }
          if let Some(t) = q.modified_before {
              sql.push_str(" AND mtime <= ?");
              bind_before = Some(t);
          }
          if let Some(n) = q.size_min {
              sql.push_str(" AND size >= ?");
              bind_size_min = Some(n);
          }
          if let Some(n) = q.size_max {
              sql.push_str(" AND size <= ?");
              bind_size_max = Some(n);
          }
          let (cursor_rank, cursor_id) = match cursor {
              Some((r, id)) => (Some(r), Some(id)),
              None => (None, None),
          };
          if cursor_rank.is_some() {
              // mysql MATCH rank: higher = better. Strictly-after cursor.
              sql.push_str(" AND (MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) < ? OR (MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) = ? AND fileid > ?))");
          }
          sql.push_str(" ORDER BY rank DESC, fileid ASC LIMIT ?");

          let mut query = sqlx::query(&sql)
              .bind(&match_text)
              .bind(viewer_uid)
              .bind(&match_text);
          if let Some(m) = bind_mime { query = query.bind(m); }
          if let Some(t) = bind_after { query = query.bind(t); }
          if let Some(t) = bind_before { query = query.bind(t); }
          if let Some(n) = bind_size_min { query = query.bind(n); }
          if let Some(n) = bind_size_max { query = query.bind(n); }
          if let Some(r) = cursor_rank {
              query = query
                  .bind(&match_text).bind(r)
                  .bind(&match_text).bind(r)
                  .bind(cursor_id.unwrap());
          }
          query = query.bind(limit);

          let rows = query.fetch_all(pool).await?;
          rows.iter().map(row_to_hit).collect()
      }

      async fn query_pg(
          &self,
          pool: &sqlx::PgPool,
          viewer_uid: &str,
          q: &SearchQuery,
          limit: i64,
          cursor: Option<(f64, i64)>,
      ) -> Result<Vec<SearchHit>, SearchError> {
          let mut match_text = q.text.clone();
          if let Some(ph) = &q.phrase {
              if !match_text.is_empty() {
                  match_text.push(' ');
              }
              match_text.push_str(ph);
          }

          let mut sql = String::from(sql::QUERY_BASE_PG);
          let mut next_arg = 3; // $1 + $2 + base $1 reuse: base uses $1 for rank-input AND tsquery; $2 for viewer
          let mut bind_mime = None;
          let mut bind_after = None;
          let mut bind_before = None;
          let mut bind_size_min = None;
          let mut bind_size_max = None;
          if let Some(m) = &q.mime {
              sql.push_str(&format!(" AND mime LIKE ${next_arg}"));
              next_arg += 1;
              bind_mime = Some(m.replace('*', "%"));
          }
          if let Some(t) = q.modified_after {
              sql.push_str(&format!(" AND mtime >= ${next_arg}"));
              next_arg += 1;
              bind_after = Some(t);
          }
          if let Some(t) = q.modified_before {
              sql.push_str(&format!(" AND mtime <= ${next_arg}"));
              next_arg += 1;
              bind_before = Some(t);
          }
          if let Some(n) = q.size_min {
              sql.push_str(&format!(" AND size >= ${next_arg}"));
              next_arg += 1;
              bind_size_min = Some(n);
          }
          if let Some(n) = q.size_max {
              sql.push_str(&format!(" AND size <= ${next_arg}"));
              next_arg += 1;
              bind_size_max = Some(n);
          }
          let (cursor_rank, cursor_id) = match cursor {
              Some((r, id)) => (Some(r), Some(id)),
              None => (None, None),
          };
          if cursor_rank.is_some() {
              let r_pos = next_arg;
              let r_pos2 = next_arg + 1;
              let id_pos = next_arg + 2;
              sql.push_str(&format!(
                  " AND (ts_rank_cd(tsv, plainto_tsquery('simple', $1)) < ${r_pos} \
                   OR (ts_rank_cd(tsv, plainto_tsquery('simple', $1)) = ${r_pos2} AND fileid > ${id_pos}))"
              ));
              next_arg += 3;
          }
          let limit_pos = next_arg;
          sql.push_str(&format!(" ORDER BY rank DESC, fileid ASC LIMIT ${limit_pos}"));

          let mut query = sqlx::query(&sql).bind(&match_text).bind(viewer_uid);
          if let Some(m) = bind_mime { query = query.bind(m); }
          if let Some(t) = bind_after { query = query.bind(t); }
          if let Some(t) = bind_before { query = query.bind(t); }
          if let Some(n) = bind_size_min { query = query.bind(n); }
          if let Some(n) = bind_size_max { query = query.bind(n); }
          if let Some(r) = cursor_rank {
              query = query.bind(r).bind(r).bind(cursor_id.unwrap());
          }
          query = query.bind(limit);

          let rows = query.fetch_all(pool).await?;
          rows.iter().map(row_to_hit).collect()
      }
  }

  fn escape_fts5(s: &str) -> String {
      // Escape embedded quotes by doubling per FTS5 syntax.
      s.replace('"', "\"\"")
  }

  /// Decode any-dialect row into [`SearchHit`].
  fn row_to_hit<R>(r: &R) -> Result<SearchHit, SearchError>
  where
      R: sqlx::Row,
      i64: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      f64: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      String: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      for<'a> &'a str: sqlx::ColumnIndex<R>,
  {
      Ok(SearchHit {
          fileid: r.try_get("fileid")?,
          storage_id: r.try_get("storage_id")?,
          basename: r.try_get("basename")?,
          path: r.try_get("path")?,
          mime: r.try_get("mime")?,
          mtime: r.try_get("mtime")?,
          size: r.try_get("size")?,
          rank: r.try_get("rank")?,
      })
  }

  /// Trait that `crabcloud-sharing` depends on so it can drive bulk
  /// fan-out at share lifecycle events without taking a hard dep on
  /// `crabcloud-search`. `Search` itself impls this; tests can use a
  /// `Noop` impl.
  #[async_trait]
  pub trait SearchFanout: Send + Sync {
      /// Walk every fileid under `owner_subroot_path` (in the owner's
      /// storage) and UPSERT one row per recipient. `recipient_path_prefix`
      /// is the share-mount-translated path prefix that replaces
      /// `owner_subroot_path` in the recipient's view (e.g. owner's
      /// `/docs/report.docx` becomes recipient's
      /// `/from-alice/report.docx`).
      ///
      /// Returns Ok with zero rows on an empty share. Errors propagate
      /// to the caller (Shares::create) which logs + continues.
      async fn fan_out_for_share(
          &self,
          recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
          recipient_path_prefix: &str,
      ) -> Result<(), SearchError>;

      /// Walk the same subroot and DELETE one (recipient, fileid) row
      /// per recipient per fileid.
      async fn fan_out_for_unshare(
          &self,
          former_recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
      ) -> Result<(), SearchError>;
  }

  #[async_trait]
  impl SearchFanout for Search {
      async fn fan_out_for_share(
          &self,
          recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
          _recipient_path_prefix: &str,
      ) -> Result<(), SearchError> {
          // Fan-out body is implemented in Task A6 once we have access to
          // the filecache walk helper. Stub returns Ok so the share
          // service can wire the call without breaking.
          let _ = recipients;
          Ok(())
      }

      async fn fan_out_for_unshare(
          &self,
          former_recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
      ) -> Result<(), SearchError> {
          let _ = former_recipients;
          Ok(())
      }
  }
  ```

  **Note on `row_to_hit`:** if the generic trait bounds prove unwieldy for `f64` decode across the three dialects, fall back to per-dialect inline decode (mirror `crates/crabcloud-activity/src/service.rs::row_to_activity`).

- [ ] **Step 4: Iterate against the e2e until GREEN**

  ```bash
  cargo test -p crabcloud-search --test search_e2e
  ```

  All 12 tests must pass. Sticking points:
  - sqlite `bm25(oc_search)` rank is a `REAL`; `try_get::<f64, _>` should decode it.
  - `GLOB` for mime matching mirrors how Nextcloud users expect `image/*`. If GLOB has issues use `LIKE` with `replace('*', '%')` instead.
  - The mysql GLOB→LIKE conversion (`*` → `%`) is in the dialect-specific branch.
  - The pagination predicate uses strict-after semantics so an exact-tie row isn't returned twice across pages.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-search/src/service.rs crates/crabcloud-search/tests/search_e2e.rs
  git commit -m "search: Search service (query / upsert / delete / pagination + per-dialect FTS)"
  ```

### Task A6: Implement `SearchFanout` body + `recipients_for_fileid` helper

**Files:**
- Modify: `crates/crabcloud-search/src/service.rs` (replace stub fan-out body with real one)
- Modify: `crates/crabcloud-search/Cargo.toml` (add `crabcloud-filecache` dep — needed to walk owner subroots)
- Modify: `crates/crabcloud-sharing/src/service.rs` (add `recipients_for_fileid` helper)
- Modify: `crates/crabcloud-search/tests/search_e2e.rs` (add real fan-out test)

- [ ] **Step 1: Add `crabcloud-filecache` workspace dep**

  ```toml
  # crates/crabcloud-search/Cargo.toml
  crabcloud-filecache = { workspace = true }
  ```

- [ ] **Step 2: Implement `Search::fan_out_for_share`**

  Replace the stub. It needs a `&FileCache` to walk the owner's subroot.

  ```rust
  // crates/crabcloud-search/src/service.rs - replace impl SearchFanout block

  use crabcloud_filecache::FileCache;

  impl Search {
      /// Real fan-out: walk every fileid under `owner_subroot_path` in
      /// `owner_uid`'s home storage and UPSERT one row per recipient
      /// with the share-mount-translated path.
      ///
      /// Caller must supply the filecache reference; we don't store it
      /// on `Search` because the only caller (the share service) already
      /// holds it.
      pub async fn fan_out_for_share_with_filecache(
          &self,
          filecache: &FileCache,
          recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
          recipient_path_prefix: &str,
      ) -> Result<(), SearchError> {
          if recipients.is_empty() {
              return Ok(());
          }
          let rows = filecache
              .walk_under(owner_uid, owner_subroot_path)
              .await
              .map_err(|e| SearchError::Db(sqlx::Error::Protocol(format!("filecache walk: {e}"))))?;
          for row in rows {
              let viewer_path = translate_path(owner_subroot_path, recipient_path_prefix, &row.path);
              let basename = std::path::Path::new(&viewer_path)
                  .file_name()
                  .and_then(|s| s.to_str())
                  .unwrap_or(&row.path).to_string();
              for r in &recipients {
                  self.upsert_for_file(
                      r.as_str(),
                      row.fileid,
                      row.storage_id,
                      &basename,
                      &viewer_path,
                      &row.mime,
                      row.mtime,
                      row.size,
                  ).await?;
              }
          }
          let _ = owner_uid;
          Ok(())
      }

      /// Inverse of `fan_out_for_share_with_filecache`. Walks the same
      /// subroot and DELETEs the (recipient, fileid) rows.
      pub async fn fan_out_for_unshare_with_filecache(
          &self,
          filecache: &FileCache,
          former_recipients: Vec<crabcloud_users::UserId>,
          owner_uid: &str,
          owner_subroot_path: &str,
      ) -> Result<(), SearchError> {
          if former_recipients.is_empty() {
              return Ok(());
          }
          let rows = filecache
              .walk_under(owner_uid, owner_subroot_path)
              .await
              .map_err(|e| SearchError::Db(sqlx::Error::Protocol(format!("filecache walk: {e}"))))?;
          for row in rows {
              for r in &former_recipients {
                  self.delete_for_viewer_file(r.as_str(), row.fileid).await?;
              }
          }
          let _ = owner_uid;
          Ok(())
      }
  }

  /// Translate an owner-relative path to a viewer-relative path. Given
  /// owner_subroot=`/docs` and recipient_prefix=`/from-alice`,
  /// owner_path=`/docs/q1/r.docx` becomes `/from-alice/q1/r.docx`.
  fn translate_path(owner_subroot: &str, recipient_prefix: &str, owner_path: &str) -> String {
      let trimmed = owner_path.strip_prefix(owner_subroot).unwrap_or(owner_path);
      format!("{}{}", recipient_prefix.trim_end_matches('/'), trimmed)
  }

  #[async_trait]
  impl SearchFanout for Search {
      async fn fan_out_for_share(
          &self,
          _recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
          _recipient_path_prefix: &str,
      ) -> Result<(), SearchError> {
          // The trait method takes no filecache. The share service uses
          // `fan_out_for_share_with_filecache` directly via `Arc<Search>`
          // (the trait exists so consumers can substitute a Noop fan-out
          // in tests).
          tracing::warn!("SearchFanout::fan_out_for_share called via trait — use fan_out_for_share_with_filecache directly");
          Ok(())
      }

      async fn fan_out_for_unshare(
          &self,
          _former_recipients: Vec<crabcloud_users::UserId>,
          _owner_uid: &str,
          _owner_subroot_path: &str,
      ) -> Result<(), SearchError> {
          tracing::warn!("SearchFanout::fan_out_for_unshare called via trait — use fan_out_for_unshare_with_filecache directly");
          Ok(())
      }
  }
  ```

  **Note on `FileCache::walk_under`**: this helper may not exist yet. Grep for it; if missing, add a new method on `FileCache` that returns `Vec<FilecacheRow>` for all rows under a path prefix. Look at how the scanner walks (`crates/crabcloud-filecache/src/scanner.rs`) — there's likely a `list_under` or similar helper. If nothing exists, add a small `pub async fn walk_under(&self, owner_uid: &str, path_prefix: &str) -> Result<Vec<FilecacheRow>, FileCacheError>` that wraps a `SELECT fileid, storage_id, path, mime, mtime, size FROM oc_filecache WHERE storage_id = ? AND (path = ? OR path LIKE ? || '/%')`.

- [ ] **Step 3: Add `Shares::recipients_for_fileid` helper**

  In `crates/crabcloud-sharing/src/service.rs`, add a method:

  ```rust
  impl Shares {
      /// Returns the de-duped set of UserIds that can see `fileid` —
      /// owner + every user-share recipient + every group-share member.
      /// Used by the search indexer for the per-write fan-out.
      pub async fn recipients_for_fileid(
          &self,
          fileid: i64,
      ) -> Result<Vec<crabcloud_users::UserId>, ShareError> {
          // 1. Look up the owner via oc_filecache + oc_storages (the owner
          //    uid is encoded in the storage_id, e.g. "local::<datadir>/alice/files").
          //    Look at how `View::resolve_recipients` or similar resolves
          //    the owner; copy that pattern.
          // 2. Look up all shares (user + group) whose item_source matches
          //    the fileid OR whose item_source is an ancestor folder of
          //    the fileid (cascading shares).
          // 3. For group shares, expand via users.group_store().members_of().
          // 4. De-dupe via HashSet<String>, return Vec<UserId>.
          todo!("Task A6 step 3: implement recipients_for_fileid")
      }
  }
  ```

  The exact SQL depends on how shares are stored. Read `crates/crabcloud-sharing/src/sql.rs` (or wherever the share queries live) to understand `oc_share.item_source` semantics, then write a query that finds every share whose item_source covers the fileid (including ancestors — a share of `/docs` covers `/docs/q1/r.docx`).

  **Critical:** the cascading-share-ancestor query is the trickiest part. The brute-force approach is "walk from the fileid up to root, collecting every ancestor fileid, then SELECT FROM oc_share WHERE item_source IN (?,?,…)". Since filecache stores `parent` per row, this is N hops where N is the path depth — bounded and cheap.

- [ ] **Step 4: Add real fan-out e2e test**

  Append to `crates/crabcloud-search/tests/search_e2e.rs`:

  ```rust
  #[tokio::test]
  async fn fan_out_for_share_with_filecache_inserts_rows_for_recipients() {
      use crabcloud_filecache::FileCache;
      let (pool, _d) = setup().await;
      let filecache = FileCache::new(pool.clone());
      // Seed two files under alice's "/docs/...":
      // (Use whatever insert helpers exist on FileCache; if testing tooling
      //  is thin, INSERT directly into oc_filecache + oc_storages via
      //  sqlx::query in this test scope.)
      // Then call fan_out_for_share_with_filecache(&filecache, vec![uid("bob")], "alice", "/docs", "/from-alice").
      // Assert bob has 2 rows with /from-alice/... paths.
      let _ = filecache;
      let _ = pool;
      // (Full impl depends on the filecache test helpers; outline only here
      //  because the impl was already covered by upsert tests. Add concrete
      //  asserts following filecache test patterns in
      //  crates/crabcloud-filecache/tests/.)
  }
  ```

  This test is structurally correct but the body depends on filecache test helpers. If those helpers don't expose easy file-seeding from outside the crate, **defer this test to a cross-crate integration test in `crabcloud-core` or `crabcloud-fs`** where you have AppState and View available. The unit test for the `translate_path` helper is the load-bearing piece — add that inline:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn translate_path_replaces_subroot_prefix() {
          assert_eq!(
              translate_path("/docs", "/from-alice", "/docs/q1/report.docx"),
              "/from-alice/q1/report.docx",
          );
      }

      #[test]
      fn translate_path_handles_root_owner_subroot() {
          assert_eq!(
              translate_path("/", "/from-alice", "/q1/report.docx"),
              "/from-alice/q1/report.docx",
          );
      }

      #[test]
      fn translate_path_handles_trailing_slash_in_prefix() {
          assert_eq!(
              translate_path("/docs", "/from-alice/", "/docs/r.txt"),
              "/from-alice/r.txt",
          );
      }
  }
  ```

- [ ] **Step 5: Run + iterate**

  ```bash
  cargo test -p crabcloud-search
  cargo test -p crabcloud-sharing
  ```

  Expected: all passing. `Shares::recipients_for_fileid` may need a focused test in the sharing crate (assert recipients for a fileid under a user-share + a group-share return the right set).

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-search/ crates/crabcloud-sharing/
  git commit -m "search: real fan-out with FileCache::walk_under + Shares::recipients_for_fileid"
  ```

### Task A7: Wire `Shares::{create, delete}` to drive fan-out

**Files:**
- Modify: `crates/crabcloud-sharing/Cargo.toml` (already added in Task A6)
- Modify: `crates/crabcloud-sharing/src/service.rs` — add `search` field to `Shares` via `SharesConfig`; call `fan_out_for_share_with_filecache` after create + `fan_out_for_unshare_with_filecache` after delete.

- [ ] **Step 1: Add `search` field on `SharesConfig`**

  Find `SharesConfig` (introduced in SP12 polish C). Add:

  ```rust
  pub search: Arc<crabcloud_search::Search>,
  pub filecache: Arc<crabcloud_filecache::FileCache>,  // may already be present
  ```

  Pass through to the `Shares` struct.

- [ ] **Step 2: Call fan-out in `Shares::create`**

  After the share row is committed:

  ```rust
  // Resolve the recipient list (user share or group expansion) AND the
  // recipient_path_prefix from the share row's item_target / file_target.
  let recipients = match req.share_type {
      ShareType::User => vec![UserId::new(req.share_with.clone())?],
      ShareType::Group => {
          let gid = GroupId::new(req.share_with.clone())?;
          self.users.group_store().members_of(&gid).await
              .unwrap_or_default()
      }
      ShareType::Link | ShareType::Email => vec![],  // public links aren't a fan-out target
  };
  if !recipients.is_empty() {
      let recipient_prefix = format!("/{}", req.file_target.trim_start_matches('/'));
      if let Err(e) = self.search.fan_out_for_share_with_filecache(
          &self.filecache,
          recipients,
          &req.requester,
          &req.path,         // owner_subroot_path
          &recipient_prefix, // recipient_path_prefix
      ).await {
          tracing::warn!(error = %e, share_id, "search fan_out_for_share failed");
      }
  }
  ```

- [ ] **Step 3: Call fan-out in `Shares::delete`**

  Read the share row BEFORE deletion (the existing SP14 emit pattern already does this), then after the row delete completes:

  ```rust
  if !former_recipients.is_empty() {
      if let Err(e) = self.search.fan_out_for_unshare_with_filecache(
          &self.filecache,
          former_recipients,
          &row.uid_owner,
          &row.path,  // however the owner subroot is stored
      ).await {
          tracing::warn!(error = %e, share_id, "search fan_out_for_unshare failed");
      }
  }
  ```

  Adjust field names (`row.uid_owner`, `row.path`, etc.) to the actual share row shape.

- [ ] **Step 4: Update AppState wiring**

  In `crates/crabcloud-core/src/state.rs::AppStateBuilder::build`, construct `Search` and pass it into `SharesConfig`:

  ```rust
  let search = Arc::new(crabcloud_search::Search::new(Arc::new(pool.clone())));
  // ... existing SharesConfig construction ...
  let shares = Arc::new(crabcloud_sharing::Shares::new(crabcloud_sharing::SharesConfig {
      // ... existing fields ...
      search: search.clone(),
      filecache: filecache.clone(),
  }));
  ```

- [ ] **Step 5: Update test fixtures that build `Shares::new` / `SharesConfig` directly**

  Grep + add `search: Arc::new(Search::new(pool.clone()))` (and filecache if needed) to each fixture.

- [ ] **Step 6: Add an integration test in `crates/crabcloud-sharing/tests/`**

  ```rust
  #[tokio::test]
  async fn user_share_create_fans_out_to_search() {
      // Build AppState. Seed alice's /docs/report.docx via filecache helpers.
      // alice.shares.create(... share_type=User share_with=bob ...).await.unwrap();
      // assert state.search.query("bob", &parse_query("report"), 10, None).await.unwrap().len() == 1.
  }

  #[tokio::test]
  async fn user_share_delete_removes_from_search() {
      // Same setup + share creation, then delete the share.
      // Assert state.search.query("bob", ...).await.unwrap().is_empty().
  }
  ```

- [ ] **Step 7: Run + commit**

  ```bash
  cargo test -p crabcloud-sharing
  cargo test --workspace
  git add crates/crabcloud-sharing/ crates/crabcloud-core/src/state.rs
  git commit -m "search: Shares::{create, delete} drive search fan_out_{for_share, for_unshare}"
  ```

### Task A8: `SearchIndexer` background task

**Files:**
- Create: `crates/crabcloud-core/src/search_indexer.rs`
- Modify: `crates/crabcloud-core/src/lib.rs`
- Modify: `crates/crabcloud-core/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-search` dep**

  ```toml
  # crates/crabcloud-core/Cargo.toml
  crabcloud-search = { workspace = true }
  ```

- [ ] **Step 2: Read the existing scanner for the storage_sink subscriber pattern**

  Open `crates/crabcloud-filecache/src/scanner.rs`. Find the subscribe loop. Note:
  - It calls `storage_sink.subscribe()` to get a broadcast receiver.
  - It runs a `loop { match rx.recv().await { ... } }`.
  - It handles `RecvError::Lagged(n)` with a tracing warn + continue.
  - It handles `RecvError::Closed` by returning.

  Mirror this exactly.

- [ ] **Step 3: Write `src/search_indexer.rs`**

  ```rust
  //! Background task: subscribes to `storage_sink` events and maintains
  //! the `oc_search` index. Per-event panic-survival via per-event
  //! `tokio::spawn` + ignore-result.
  //!
  //! Recipients are resolved via `Shares::recipients_for_fileid` at the
  //! moment of the event (point-in-time). Owner-side rows are always
  //! UPSERTed; recipient rows reflect the share graph at event time.

  use crabcloud_filecache::FileCache;
  use crabcloud_search::Search;
  use crabcloud_sharing::Shares;
  use crabcloud_storage::{ChannelEventSink, StorageEvent};
  use std::sync::Arc;
  use tokio::sync::broadcast::error::RecvError;
  use tokio::sync::Notify;

  pub struct SearchIndexer {
      search: Arc<Search>,
      shares: Arc<Shares>,
      filecache: Arc<FileCache>,
      rx: tokio::sync::broadcast::Receiver<StorageEvent>,
      shutdown: Arc<Notify>,
  }

  impl SearchIndexer {
      pub fn new(
          search: Arc<Search>,
          shares: Arc<Shares>,
          filecache: Arc<FileCache>,
          storage_sink: &ChannelEventSink,
      ) -> (Self, Arc<Notify>) {
          let shutdown = Arc::new(Notify::new());
          let rx = storage_sink.subscribe();
          (
              Self { search, shares, filecache, rx, shutdown: shutdown.clone() },
              shutdown,
          )
      }

      pub async fn run(mut self) {
          loop {
              tokio::select! {
                  _ = self.shutdown.notified() => return,
                  ev = self.rx.recv() => match ev {
                      Ok(event) => {
                          // Per-event panic survival: run handle_event in a
                          // sub-task. If it panics, log + continue the loop.
                          let search = self.search.clone();
                          let shares = self.shares.clone();
                          let filecache = self.filecache.clone();
                          let event_for_log = format!("{:?}", event);
                          let handle = tokio::spawn(async move {
                              handle_event(&search, &shares, &filecache, event).await;
                          });
                          if let Err(e) = handle.await {
                              tracing::error!(error = %e, event = %event_for_log, "search indexer: handler panicked or was cancelled");
                          }
                      }
                      Err(RecvError::Lagged(n)) => {
                          tracing::warn!(dropped = n, "search indexer: lagged behind storage_sink; events dropped");
                      }
                      Err(RecvError::Closed) => {
                          tracing::info!("search indexer: storage_sink closed; exiting");
                          return;
                      }
                  }
              }
          }
      }
  }

  async fn handle_event(
      search: &Search,
      shares: &Shares,
      filecache: &FileCache,
      event: StorageEvent,
  ) {
      // Skip trash storage events (trash isn't searchable).
      if is_trash_storage(&event) {
          return;
      }
      match event {
          StorageEvent::Created { storage_id, path } | StorageEvent::Modified { storage_id, path } => {
              if let Err(e) = upsert_for_event(search, shares, filecache, &storage_id, &path).await {
                  tracing::warn!(error = %e, storage_id, path, "search indexer: upsert failed");
              }
          }
          StorageEvent::Deleted { storage_id: _, path: _, fileid } => {
              if let Some(fid) = fileid {
                  if let Err(e) = search.delete_for_file(fid).await {
                      tracing::warn!(error = %e, fileid = fid, "search indexer: delete failed");
                  }
              }
          }
          StorageEvent::Renamed { storage_id, from: _, to } => {
              // Re-resolve recipients (a rename may have moved the file
              // out of OR into a shared subroot).
              if let Err(e) = upsert_for_event(search, shares, filecache, &storage_id, &to).await {
                  tracing::warn!(error = %e, storage_id, to, "search indexer: rename-upsert failed");
              }
              // For OUT-OF-share renames the upsert path won't delete
              // stale viewer rows; explicit cleanup happens via the
              // resolved recipient list (a recipient who can no longer
              // see the file at all simply doesn't appear in the new
              // recipient set, and the indexer must remove them).
              // Implementation: read the OLD recipient set (cached via
              // pre-rename SELECT) and compute set-difference vs the
              // new set. For MVP, we'll handle the simpler case: rebuild
              // by deleting all viewer rows for the fileid then
              // re-upserting per the new recipient set. Trades some
              // write amplification for correctness.
              // The fileid resolution is done via the upsert_for_event
              // path (which looks up filecache by path).
          }
      }
  }

  /// Resolve the filecache row for (storage_id, path) and UPSERT one
  /// search row per current recipient. If the file isn't in filecache
  /// (race between event emit and scanner), skip with a debug log.
  async fn upsert_for_event(
      search: &Search,
      shares: &Shares,
      filecache: &FileCache,
      storage_id: &str,
      path: &str,
  ) -> Result<(), crabcloud_search::SearchError> {
      // Look up the filecache row. The exact API is filecache.lookup or
      // filecache.lookup_by_path — read crabcloud-filecache to find it.
      // If None, the row hasn't been indexed yet; skip.
      let storage_path = crabcloud_storage::StoragePath::new(path.trim_start_matches('/').to_string())
          .map_err(|e| crabcloud_search::SearchError::Db(sqlx::Error::Protocol(format!("storage path: {e}"))))?;
      let row = match filecache.lookup(storage_id, &storage_path).await {
          Ok(Some(r)) => r,
          Ok(None) => {
              tracing::debug!(storage_id, path, "search indexer: filecache miss; skipping upsert");
              return Ok(());
          }
          Err(e) => {
              return Err(crabcloud_search::SearchError::Db(sqlx::Error::Protocol(format!("filecache lookup: {e}"))));
          }
      };

      // Resolve recipients (owner + share recipients + group members).
      let recipients = shares.recipients_for_fileid(row.fileid).await
          .map_err(|e| crabcloud_search::SearchError::Db(sqlx::Error::Protocol(format!("recipients: {e}"))))?;

      // For each recipient, compute their viewer-path (owner path for
      // the owner; share-mount-translated for share recipients). For the
      // MVP, we use the OWNER's path for everyone — the recipient_path
      // translation requires knowing each recipient's mount structure,
      // which the indexer doesn't have readily. The path stored in the
      // recipient's row will be the owner's path; the UI handles
      // share-mount aware display when the user clicks through.
      // FUTURE: enrich this by joining oc_share to get each recipient's
      // file_target (their view's prefix).
      let basename = std::path::Path::new(path)
          .file_name().and_then(|s| s.to_str())
          .unwrap_or(path).to_string();
      for recipient in recipients {
          search.upsert_for_file(
              recipient.as_str(),
              row.fileid,
              0,  // TODO: surface storage_id_num from filecache row
              &basename,
              path,
              &row.mime,
              row.mtime,
              row.size,
          ).await?;
      }
      Ok(())
  }

  fn is_trash_storage(event: &StorageEvent) -> bool {
      let id = match event {
          StorageEvent::Created { storage_id, .. } => storage_id.as_str(),
          StorageEvent::Modified { storage_id, .. } => storage_id.as_str(),
          StorageEvent::Deleted { storage_id, .. } => storage_id.as_str(),
          StorageEvent::Renamed { storage_id, .. } => storage_id.as_str(),
      };
      id.contains("files_trashbin")
  }
  ```

  **Important caveats called out by this code:**
  - `StorageEvent::Deleted { fileid }` — confirm the event carries the fileid. If not, the indexer needs a pre-delete lookup; in that case rework the design so `View::delete` resolves the fileid first.
  - The fan-out path uses the OWNER's path for recipients in the indexer; spec §2 decision #4 says we want the viewer's path. To honor that fully, the indexer needs to know each recipient's mount prefix. For Batch A MVP we accept the simpler "owner path" model and document; Batch B/C UI tolerates either. Add a TODO comment + a follow-up task.
  - `crabcloud_filecache::FileCache::lookup` returns whatever the existing API uses — adjust the call site.
  - `is_trash_storage` is a quick heuristic; if the storage id format changes the indexer would mis-classify. Better: add a small `pub fn is_trash(storage_id: &str) -> bool` to `crabcloud-trash` and import it.

- [ ] **Step 4: Wire in `lib.rs`**

  ```rust
  // crates/crabcloud-core/src/lib.rs
  mod search_indexer;
  pub use search_indexer::SearchIndexer;
  ```

- [ ] **Step 5: Build**

  ```bash
  cargo build -p crabcloud-core
  ```

  Expected: clean (after resolving the filecache/storage type imports).

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-core/Cargo.toml crates/crabcloud-core/src/search_indexer.rs crates/crabcloud-core/src/lib.rs
  git commit -m "search: SearchIndexer subscriber for storage_sink with per-event panic survival"
  ```

### Task A9: Spawn `SearchIndexer` in `AppStateBuilder::build`

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Construct `Search` (already done in Task A7) and `SearchIndexer`**

  After `Search` is constructed and after `Shares` is wired:

  ```rust
  let (search_indexer, search_indexer_shutdown) = crate::search_indexer::SearchIndexer::new(
      search.clone(),
      shares.clone(),
      filecache.clone(),
      &storage_sink,
  );
  // Always spawn — search is unconditional. JoinHandle dropped intentionally.
  std::mem::drop(tokio::spawn(async move { search_indexer.run().await }));
  ```

- [ ] **Step 2: Expose `search` + `search_indexer_shutdown` on `AppState`**

  ```rust
  pub struct AppState {
      // ... existing fields ...
      pub search: Arc<crabcloud_search::Search>,
      pub search_indexer_shutdown: Arc<tokio::sync::Notify>,
  }
  ```

  Add to the constructor:
  ```rust
  search,
  search_indexer_shutdown,
  ```

- [ ] **Step 3: Build + state tests**

  ```bash
  cargo build --workspace
  cargo test -p crabcloud-core state
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add crates/crabcloud-core/src/state.rs
  git commit -m "search: wire Search + SearchIndexer into AppState; spawn unconditionally"
  ```

### Task A10: End-to-end integration test through `AppState`

**Files:**
- Create: `crates/crabcloud-fs/tests/search_through_appstate.rs` (or extend an existing wiring test)

- [ ] **Step 1: Write the test**

  ```rust
  //! End-to-end: AppState -> View::write_file -> storage_sink fires ->
  //! SearchIndexer processes -> Search::query returns the hit.

  #[tokio::test]
  async fn write_eventually_indexed_then_queryable() {
      // Build a full AppState (mirror the appstate_wiring.rs harness).
      // Create a user 'alice', open her View, write_file "/docs/report.docx".
      // Poll state.search.query("alice", &parse_query("report"), 10, None)
      // with a 5-second bounded retry (200ms interval) until we get a hit.
      // Assert exactly one hit with the expected fileid/basename/path.
  }

  #[tokio::test]
  async fn delete_eventually_removes_from_index() {
      // Same setup; after the hit appears, View::hard_delete the file.
      // Poll until search.query returns empty; assert it eventually does.
  }
  ```

- [ ] **Step 2: Run + iterate**

  ```bash
  cargo test -p crabcloud-fs --test search_through_appstate
  ```

  Expected: both tests pass within a few seconds.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-fs/tests/search_through_appstate.rs
  git commit -m "search: end-to-end test through AppState -> View -> storage_sink -> SearchIndexer -> Search"
  ```

### Task A11: Batch A pre-PR

- [ ] **Step 1: Pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push and open PR**

  ```bash
  git push -u origin sp15/a-search-crate
  gh pr create --title "sp15(a): crabcloud-search crate + indexer + share fan-out + query parser" \
    --body "Batch A of SP15. New crabcloud-search crate with per-dialect FTS query (sqlite FTS5 / mysql FULLTEXT / postgres tsvector+GIN), 0012_search_index migration triplet, query parser (bare terms + mime/modified/size filters + phrases), SearchIndexer background task subscribed to storage_sink with per-event panic survival, Shares::{create, delete} hooks driving bulk fan-out via SearchFanout. Spec: docs/superpowers/specs/2026-05-17-search-design.md."
  ```

---

# Batch B — OCS REST surface

**Branch:** `sp15/b-ocs` (off the merged Batch A master)

**Goal:** Add the Nextcloud-shape OCS unified-search-provider endpoint at `/ocs/v2.php/search/providers/files/search`.

### Task B1: OCS endpoint

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/search.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`
- Modify: `crates/crabcloud-http/Cargo.toml`
- Create: `crates/crabcloud-http/tests/ocs_search.rs`

- [ ] **Step 1: Add dep**

  ```toml
  # crates/crabcloud-http/Cargo.toml
  crabcloud-search = { workspace = true }
  ```

- [ ] **Step 2: Write `search.rs`**

  Mirror `routes/ocs/activity.rs` (SP14 Batch B). Use shared `super::envelope::*` helpers.

  ```rust
  //! OCS unified-search-provider endpoint for file metadata.
  //!
  //! /ocs/v2.php/search/providers/files/search?query=...&limit=...&cursor=...
  //!
  //! Response shape matches Nextcloud's unified-search provider format
  //! so existing third-party clients work without translation.

  use axum::extract::{Extension, Query, State};
  use axum::routing::get;
  use axum::Router;
  use base64::Engine as _;
  use crabcloud_core::AppState;
  use crabcloud_search::{parse_query, SearchError, SearchHit};
  use serde::{Deserialize, Serialize};

  pub fn router() -> Router<AppState> {
      Router::new().route("/search", get(search_handler))
  }

  #[derive(Deserialize, Default)]
  struct SearchParams {
      query: String,
      limit: Option<i64>,
      cursor: Option<String>,
  }

  #[derive(Serialize)]
  struct EntryAttributes {
      fileid: String,
      mime: String,
      size: String,
      mtime: String,
  }

  #[derive(Serialize)]
  struct EntryDto {
      thumbnail_url: String,
      title: String,
      subline: String,
      resource_url: String,
      icon: String,
      rounded: bool,
      attributes: EntryAttributes,
  }

  async fn search_handler(
      State(state): State<AppState>,
      Extension(ctx): Extension<crate::middleware::auth::AuthContext>,
      Query(p): Query<SearchParams>,
  ) -> impl axum::response::IntoResponse {
      let limit = p.limit.unwrap_or(20).clamp(1, 50);
      let parsed = parse_query(&p.query);
      let cursor = match p.cursor.as_deref().map(decode_cursor) {
          Some(Ok(c)) => Some(c),
          Some(Err(_)) => {
              return super::envelope::ocs_envelope(400, "bad cursor".into(), serde_json::json!({}));
          }
          None => None,
      };
      let hits = match state.search.query(&ctx.user_id, &parsed, limit, cursor).await {
          Ok(h) => h,
          Err(e) => return from_search_error(e),
      };
      let entries: Vec<EntryDto> = hits.iter().map(hit_to_entry).collect();
      let next_cursor = hits.last().map(|h| encode_cursor(h.rank, h.fileid));
      let is_last = (hits.len() as i64) < limit;
      super::envelope::ocs_envelope(
          200, "OK".into(),
          serde_json::json!({
              "name": "Files",
              "isPaginated": true,
              "entries": entries,
              "cursor": next_cursor,
              "isLast": is_last,
          }),
      )
  }

  fn hit_to_entry(h: &SearchHit) -> EntryDto {
      EntryDto {
          thumbnail_url: "".into(),
          title: h.basename.clone(),
          subline: h.path.clone(),
          resource_url: format!("/files{}", h.path),
          icon: "".into(),
          rounded: false,
          attributes: EntryAttributes {
              fileid: h.fileid.to_string(),
              mime: h.mime.clone(),
              size: h.size.to_string(),
              mtime: h.mtime.to_string(),
          },
      }
  }

  fn encode_cursor(rank: f64, fileid: i64) -> String {
      let payload = format!("{rank}|{fileid}");
      base64::engine::general_purpose::STANDARD_NO_PAD.encode(payload)
  }

  fn decode_cursor(s: &str) -> Result<(f64, i64), &'static str> {
      let raw = base64::engine::general_purpose::STANDARD_NO_PAD.decode(s).map_err(|_| "b64")?;
      let s = std::str::from_utf8(&raw).map_err(|_| "utf8")?;
      let (a, b) = s.split_once('|').ok_or("split")?;
      let rank: f64 = a.parse().map_err(|_| "rank")?;
      let fileid: i64 = b.parse().map_err(|_| "fileid")?;
      Ok((rank, fileid))
  }

  fn from_search_error(e: SearchError) -> impl axum::response::IntoResponse {
      tracing::error!(error = %e, "search OCS: unhandled error");
      super::envelope::ocs_envelope(500, e.to_string(), serde_json::json!({}))
  }
  ```

  Verify `base64` is already a workspace dep (it's used by SP8 public links and SP14 cursors). If not, add it.

- [ ] **Step 3: Mount in `routes/ocs/mod.rs`**

  ```rust
  pub mod search;
  // ... in the router assembly:
  .nest("/v2.php/search/providers/files", search::router().with_state(state.clone()))
  ```

- [ ] **Step 4: E2E test**

  Create `crates/crabcloud-http/tests/ocs_search.rs`. Cover:
  - GET with empty query → 200 with `entries: []`.
  - Seed 3 search rows via `state.search.upsert_for_file`; GET with matching query → 3 entries.
  - GET with `?cursor=<from prior response>` → page 2 starts after cursor.
  - GET with `?limit=1` → 1 entry, `isLast` false.
  - GET with `mime:image/*` filter narrows the result.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test ocs_search
  git add crates/crabcloud-http/Cargo.toml crates/crabcloud-http/src/routes/ocs/ crates/crabcloud-http/tests/ocs_search.rs
  git commit -m "search ocs: /search/providers/files/search endpoint with cursor pagination"
  ```

### Task B2: Batch B pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp15/b-ocs
  gh pr create --title "sp15(b): OCS /search/providers/files/search endpoint" \
    --body "Batch B of SP15. Nextcloud unified-search-provider endpoint."
  ```

---

# Batch C — Server fn + UI top-bar search

**Branch:** `sp15/c-ui` (off the merged Batch B master)

### Task C1: Server fn

**Files:**
- Create: `crates/crabcloud-app/src/server_fns/search.rs`
- Modify: `crates/crabcloud-app/src/server_fns/mod.rs`
- Modify: `crates/crabcloud-app/src/lib.rs`
- Modify: `crates/crabcloud-app/Cargo.toml`
- Create: `crates/crabcloud-app/tests/server_fns_search.rs`

- [ ] **Step 1: Add dep**

  ```toml
  # crates/crabcloud-app/Cargo.toml
  crabcloud-search = { workspace = true }
  ```

- [ ] **Step 2: Write `search.rs`**

  Mirror `server_fns/activity.rs`.

  ```rust
  //! Search server fn for the UI.

  use dioxus::prelude::*;
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
  pub struct SearchHitDto {
      pub fileid: i64,
      pub basename: String,
      pub path: String,
      pub mime: String,
      pub mtime: i64,
      pub size: i64,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
  pub struct SearchResponseDto {
      pub hits: Vec<SearchHitDto>,
      pub cursor: Option<String>,
  }

  #[server(endpoint = "/api/files/search")]
  pub async fn search_files(query: String, cursor: Option<String>) -> Result<SearchResponseDto, ServerFnError> {
      use crate::server_fns::require_user;
      use crabcloud_search::parse_query;
      let (state, ctx) = require_user()?;
      let parsed = parse_query(&query);
      let cursor_tuple = cursor.as_deref().and_then(decode_cursor);
      let hits = state.search.query(&ctx.user_id, &parsed, 10, cursor_tuple).await
          .map_err(|e| {
              tracing::error!(error = %e, "search server fn failed");
              ServerFnError::new(format!("search: {e}"))
          })?;
      let next_cursor = hits.last().map(|h| encode_cursor(h.rank, h.fileid));
      Ok(SearchResponseDto {
          hits: hits.into_iter().map(|h| SearchHitDto {
              fileid: h.fileid, basename: h.basename, path: h.path,
              mime: h.mime, mtime: h.mtime, size: h.size,
          }).collect(),
          cursor: next_cursor,
      })
  }

  #[cfg(feature = "server")]
  fn encode_cursor(rank: f64, fileid: i64) -> String {
      use base64::Engine as _;
      let payload = format!("{rank}|{fileid}");
      base64::engine::general_purpose::STANDARD_NO_PAD.encode(payload)
  }

  #[cfg(feature = "server")]
  fn decode_cursor(s: &str) -> Option<(f64, i64)> {
      use base64::Engine as _;
      let raw = base64::engine::general_purpose::STANDARD_NO_PAD.decode(s).ok()?;
      let s = std::str::from_utf8(&raw).ok()?;
      let (a, b) = s.split_once('|')?;
      Some((a.parse().ok()?, b.parse().ok()?))
  }
  ```

- [ ] **Step 3: Wire mod + lib re-exports**

  ```rust
  // crates/crabcloud-app/src/server_fns/mod.rs
  pub mod search;
  ```

  ```rust
  // crates/crabcloud-app/src/lib.rs (re-export for UI / tests):
  pub use server_fns::search::{search_files, SearchHitDto, SearchResponseDto};
  ```

- [ ] **Step 4: Integration test**

  Mirror `server_fns_activity.rs`. Cover empty query, seeded results, unauthenticated.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-app --test server_fns_search
  git add crates/crabcloud-app/
  git commit -m "search: search_files server fn"
  ```

### Task C2: `<SearchBar>` component + TopBar integration

**Files:**
- Create: `crates/crabcloud-app/src/pages/files/search_bar.rs`
- Modify: `crates/crabcloud-app/src/pages/files/chrome.rs` (add `<SearchBar>` into `TopBar`)
- Modify: `crates/crabcloud-app/src/pages/files/mod.rs` (or wherever the chrome module list lives)
- Modify: `crates/crabcloud-app/assets/app.css`

- [ ] **Step 1: Write `search_bar.rs`**

  ```rust
  //! Top-bar search input + dropdown overlay.
  //!
  //! Debounced 300ms input → server-fn search_files(q) → dropdown of
  //! up to 10 hits. Click navigates to the containing folder.
  //! Escape / blur / click-outside closes. Reuses the .files-modal-*
  //! palette + new .search-bar* / .search-dropdown* CSS.

  use crate::server_fns::search::{search_files, SearchHitDto, SearchResponseDto};
  use dioxus::prelude::*;
  use std::time::Duration;

  #[component]
  pub fn SearchBar() -> Element {
      let mut query = use_signal::<String>(String::new);
      let mut hits = use_signal::<Vec<SearchHitDto>>(Vec::new);
      let mut open = use_signal::<bool>(|| false);
      let mut loading = use_signal::<bool>(|| false);
      let mut last_error = use_signal::<Option<String>>(|| None);

      // Debounced effect: when query changes, wait 300ms then fire search.
      use_effect(move || {
          let q = query();
          spawn(async move {
              // Cheap debounce via tokio::time::sleep; on quick re-edit
              // the spawned task is just a stale closure that returns
              // before fetching (best-effort).
              gloo_timers::future::TimeoutFuture::new(300).await;
              if query() != q {
                  return; // user typed more after we started; let the newer effect run
              }
              if q.trim().is_empty() {
                  hits.set(Vec::new());
                  loading.set(false);
                  return;
              }
              loading.set(true);
              match search_files(q.clone(), None).await {
                  Ok(SearchResponseDto { hits: h, .. }) => {
                      hits.set(h);
                      last_error.set(None);
                  }
                  Err(e) => {
                      last_error.set(Some(format!("Search failed: {e}")));
                      hits.set(Vec::new());
                  }
              }
              loading.set(false);
          });
      });

      let on_input = move |e: FormEvent| {
          query.set(e.value());
          open.set(true);
      };
      let on_focus = move |_evt: FocusEvent| open.set(true);
      let on_blur = move |_evt: FocusEvent| {
          // Tiny delay so a click on a result registers before close.
          spawn(async move {
              gloo_timers::future::TimeoutFuture::new(150).await;
              open.set(false);
          });
      };
      let on_keydown = move |evt: KeyboardEvent| {
          if evt.key() == Key::Escape {
              open.set(false);
          }
      };

      rsx! {
          div { class: "search-bar",
              input {
                  r#type: "search",
                  class: "search-bar-input",
                  placeholder: "Search files…",
                  value: query(),
                  oninput: on_input,
                  onfocus: on_focus,
                  onblur: on_blur,
                  onkeydown: on_keydown,
                  aria_label: "Search files",
                  aria_autocomplete: "list",
              }
              if open() {
                  SearchDropdown {
                      query: query(),
                      hits: hits(),
                      loading: loading(),
                      error: last_error(),
                  }
              }
          }
      }
  }

  #[derive(Props, Clone, PartialEq)]
  struct DropdownProps {
      query: String,
      hits: Vec<SearchHitDto>,
      loading: bool,
      error: Option<String>,
  }

  #[component]
  fn SearchDropdown(props: DropdownProps) -> Element {
      rsx! {
          div { class: "search-dropdown", role: "listbox",
              if let Some(err) = &props.error {
                  p { class: "search-dropdown-error", role: "alert", "{err}" }
              } else if props.query.trim().is_empty() {
                  p { class: "search-dropdown-hint", "Type to search" }
              } else if props.loading {
                  p { class: "search-dropdown-loading", "Searching…" }
              } else if props.hits.is_empty() {
                  p { class: "search-dropdown-empty", "No matches." }
              } else {
                  ul { class: "search-dropdown-list",
                      for hit in props.hits.iter() {
                          SearchHitRow { key: "{hit.fileid}", hit: hit.clone() }
                      }
                  }
              }
          }
      }
  }

  #[derive(Props, Clone, PartialEq)]
  struct HitProps {
      hit: SearchHitDto,
  }

  #[component]
  fn SearchHitRow(props: HitProps) -> Element {
      let h = props.hit;
      let parent = std::path::Path::new(&h.path).parent()
          .and_then(|p| p.to_str()).unwrap_or("/").to_string();
      let nav_to = format!("/files{}", if parent.is_empty() { "/" } else { parent.as_str() });
      rsx! {
          li { class: "search-dropdown-row", role: "option",
              a { href: "{nav_to}",
                  span { class: "search-dropdown-icon", aria_hidden: "true", "{icon_for_mime(&h.mime)}" }
                  span { class: "search-dropdown-name", "{h.basename}" }
                  span { class: "search-dropdown-path", "{h.path}" }
              }
          }
      }
  }

  fn icon_for_mime(mime: &str) -> &'static str {
      if mime.starts_with("image/") { "🖼" }
      else if mime.starts_with("video/") { "🎬" }
      else if mime.starts_with("audio/") { "🎵" }
      else if mime == "application/pdf" { "📕" }
      else if mime.starts_with("text/") { "📄" }
      else { "📄" }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      // SSR snapshot tests: mirror the SP14 activity page Wrapper
      // pattern (defer Callback construction to render time).
      //
      // Test cases:
      //  1. Dropdown closed (open=false) renders only the input, no dropdown div.
      //  2. open=true, query="", hits=[] → "Type to search" hint visible.
      //  3. open=true, loading=true → "Searching…" visible.
      //  4. open=true, hits with 2 entries → both basenames + paths rendered.
      //  5. open=true, hits empty + query non-empty + not loading → "No matches.".
      //  6. error state → role=alert + error string.
      //
      // The Wrapper pattern needed because SearchBar's signals can't be
      // constructed outside a dioxus runtime; tests render the
      // SearchDropdown directly with hand-built Props.
  }
  ```

  **Note on `gloo_timers`**: this is a wasm-compatible setTimeout wrapper. If not already a workspace dep, add it (already commonly used in dioxus wasm projects). If a sibling page already uses a different debounce mechanism, use that pattern instead.

- [ ] **Step 2: Add `<SearchBar />` into `TopBar`**

  Open `crates/crabcloud-app/src/pages/files/chrome.rs`. Find `TopBar`. Add the `SearchBar` component into its layout:

  ```rust
  // Inside the TopBar rsx! body:
  div { class: "top-bar-search-slot",
      SearchBar {}
  }
  ```

  Position it between the existing left chrome (logo / breadcrumb) and the right chrome (user menu). Use whatever flexbox arrangement the file already has.

- [ ] **Step 3: Wire mod**

  In `crates/crabcloud-app/src/pages/files/mod.rs` (or wherever the `chrome` module is declared):

  ```rust
  pub mod search_bar;
  pub use search_bar::SearchBar;
  ```

- [ ] **Step 4: Add CSS**

  Append to `assets/app.css` (~80 lines):

  ```css
  /* === Search bar === */
  .search-bar {
      position: relative;
      flex: 1;
      max-width: 480px;
  }
  .search-bar-input {
      width: 100%;
      padding: 6px 12px;
      border: 1px solid #d6d6d6;
      border-radius: 18px;
      font-size: 14px;
      background: #fff;
  }
  .search-bar-input:focus {
      outline: none;
      border-color: #0082c9;
  }
  .search-dropdown {
      position: absolute;
      top: 100%;
      left: 0;
      right: 0;
      margin-top: 4px;
      background: #fff;
      border: 1px solid #d6d6d6;
      border-radius: 4px;
      box-shadow: 0 6px 18px rgba(0,0,0,0.08);
      max-height: 480px;
      overflow-y: auto;
      z-index: 50;
  }
  .search-dropdown-hint,
  .search-dropdown-loading,
  .search-dropdown-empty,
  .search-dropdown-error {
      padding: 16px;
      color: #888;
      text-align: center;
      margin: 0;
  }
  .search-dropdown-error {
      color: #d33;
      background: #fff5f5;
  }
  .search-dropdown-list {
      list-style: none;
      margin: 0;
      padding: 0;
  }
  .search-dropdown-row a {
      display: grid;
      grid-template-columns: 24px 1fr auto;
      gap: 8px;
      align-items: center;
      padding: 8px 12px;
      color: inherit;
      text-decoration: none;
  }
  .search-dropdown-row a:hover {
      background: #f5fafd;
  }
  .search-dropdown-icon {
      font-size: 18px;
      line-height: 1;
  }
  .search-dropdown-name {
      font-weight: 500;
  }
  .search-dropdown-path {
      color: #888;
      font-size: 12px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
  }
  ```

- [ ] **Step 5: Build + WASM + tests**

  ```bash
  cargo test -p crabcloud-app
  cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
  cargo test --workspace
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-app/src/pages/files/ crates/crabcloud-app/assets/app.css
  git commit -m "search ui: top-bar SearchBar component with debounced server-fn dropdown"
  ```

### Task C3: Batch C pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp15/c-ui
  gh pr create --title "sp15(c): search UI (top-bar SearchBar + dropdown)" \
    --body "Final batch of SP15. New <SearchBar> component embedded in the files-page TopBar with debounced (300ms) input, dropdown of up to 10 hits, click-to-navigate, keyboard handling."
  ```

---

## Self-review notes

- **Spec coverage:** §1 goal → all batches. §2 decisions → A1–A11 (1–12), B (13), C (14). §3 architecture → A2–A9. §4 schema → A1. §5 surfaces → B (5.1) + C (5.2, 5.3). §6 edge cases → A4 parser (unknown key:value, empty query, filters-only, phrases), A5 service (per-viewer isolation, upsert-updates-existing, delete cascades, pagination cursor), A7 share lifecycle (create-fans-out, delete-removes), A8 indexer (trash skip, rename re-resolve, panic survival, lagged-channel warn), A10 e2e (write-then-query latency, delete-then-vanish). §7 testing list → unit + e2e at every layer. §8 batches → 3 batches.

- **Placeholder scan:** A few "look at the sibling pattern in X" instructions — these point at established workspace conventions (`require_user`, OCS envelope helpers, the dispatch-by-method shape, the SSR-test Wrapper pattern). The `row_to_hit` generic-trait shape may need per-dialect inlining; called out with the SP14 precedent. The `Shares::recipients_for_fileid` SQL is irreducibly codebase-specific.

- **Type consistency:** `SearchHit` shape consistent A2→A5→B→C. `SearchQuery` consistent A2→A4→A5→B→C. `SearchHitDto` / `SearchResponseDto` consistent C1→C2. `SearchFanout` trait + `Search` impl consistent A2→A6→A7.

- **Known underspecified spots** the implementer must resolve from the codebase:
  - `crabcloud_filecache::FileCache::lookup` exact signature + whether a `walk_under(uid, path_prefix)` helper exists (Task A6) — read the crate, add if missing.
  - `Shares::recipients_for_fileid` ancestor-share lookup SQL (Task A6) — depends on `oc_share` schema + how filecache stores parent chain.
  - `StorageEvent::Deleted { fileid }` payload — confirm the event carries the fileid; if not, indexer needs a pre-delete lookup.
  - `SharesConfig` field set + Shares::new constructor signature — fields may have changed since SP12 polish C; grep + update.
  - `View::new` call sites are NOT changed by SP15 (the indexer is async, lives in core, gets a `storage_sink` subscription — no View signature change).
