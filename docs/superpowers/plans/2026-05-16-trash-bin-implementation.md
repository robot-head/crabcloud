# Trash Bin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Nextcloud-compatible trash bin. `DELETE` on authed surfaces (UI, DAV, OCS) moves files to `<datadir>/<uid>/files_trashbin/files/<basename>.<suffix>`, records metadata in `oc_files_trash`, and exposes restore/purge via DAV (`/dav/trashbin/{uid}/...`), OCS (`/ocs/v2.php/apps/files_trashbin/api/v1/trashbin`), and the Dioxus UI ("Deleted files" sidebar entry). Background sweeper purges entries older than `trash_retention_days` (default 30). Public-link DELETE bypasses trash and hard-deletes.

**Architecture:** New `crabcloud-trash` crate owns the `Trash` service (`soft_delete` / `list` / `restore` / `purge` / `sweep_expired`) backed by `oc_files_trash` + a per-user `trash::<uid>` storage mount. `crabcloud-fs::View::delete` is rerouted to `Trash::soft_delete`; a new `View::hard_delete` covers the public-link bypass. `crabcloud-core::TrashSweeper` background task spawned in `AppStateBuilder::build` runs the daily age-based purge. DAV + OCS + Dioxus UI surfaces all delegate to the same `Trash` handle on `AppState`.

**Tech Stack:** Rust 1.95, sqlx 0.8 (sqlite + mysql + postgres), axum 0.8, Dioxus 0.7 fullstack. No new external dependencies — everything reuses what's already in the workspace.

**Spec:** `docs/superpowers/specs/2026-05-16-trash-bin-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is its own PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp12/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-trash-crate`, `b-dav`, `c-ocs-and-server-fns`, `d-ui`.

- **Commit cadence:** Commit at every "Commit" step. Each batch lands as a single squash-merged PR; intermediate commits get squashed.

- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- **Merge:** After CI green and user merges via GitHub UI.

- **Established workaround for AppState tests:** Tests building `AppState` set `cfg.filecache.enabled = false` and `cfg.mail.transport = "disabled"`. Add `cfg.trash_retention_days = 30` defaulting (already the default; only override when a test specifically exercises sweeper behavior with retention `0` or short retention).

- **Pre-existing patterns to mirror:**
  - **Crate shape:** `crates/crabcloud-sharing` (SP7) — focused service crate, multidialect SQL via `match self.pool.as_ref()`, `_QM` (sqlite+mysql) vs `_PG` query constants in `sql.rs`, error type in `error.rs`.
  - **Background sweeper:** `crates/crabcloud-core/src/preview_cache_cleanup.rs` and `crates/crabcloud-core/src/mail_queue_cleanup.rs` (SP11 polish) — `pub fn new(...) -> (Self, Arc<Notify>)`, `pub async fn run(self)` with `tokio::select!` shutdown, `pub async fn cleanup_once()` for sync test drive, `CLEANUP_INTERVAL: Duration` const.
  - **Migration triplet:** `migrations/core/0008_share_last_warned/{sqlite,mysql,postgres}.sql`. Next migration number is `0009`.
  - **DAV handlers:** `crates/crabcloud-http/src/routes/dav/methods.rs` — `_with_view` surface-neutral helpers, `AuthenticatedUser` extractor.
  - **OCS shape:** `crates/crabcloud-http/src/routes/ocs/` (look at `files_sharing.rs`, `users.rs`) — `OcsEnvelope` wrapping, JSON via `axum::Json`.
  - **Server fns:** `crates/crabcloud-app/src/server_fns/mod.rs` — `FullstackContext::current()`, `fs.extension::<AppState>()`, `AuthenticatedUser` pattern from `server.rs`.
  - **UI page:** `crates/crabcloud-app/src/pages/files/` — `chrome.rs` for the sidebar, `mod.rs` for the view, `row.rs` for entries.

---

## File-by-file map

### New crate: `crabcloud-trash`

```
crates/crabcloud-trash/
├── Cargo.toml
├── src/
│   ├── lib.rs         — re-exports + crate doc
│   ├── error.rs       — TrashError
│   ├── service.rs     — Trash struct + soft_delete / list / restore / purge / sweep_expired / ensure_storage
│   ├── sql.rs         — multidialect SQL constants
│   └── types.rs       — TrashEntry, TrashType, RestoredTo
└── tests/
    └── trash_e2e.rs   — sqlite e2e (full round-trip + sweeper + collision + shared-with-me)
```

### New migration

```
migrations/core/0009_files_trash/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-trash` member.
- `crates/crabcloud-fs/Cargo.toml` — adds `crabcloud-trash` workspace dep.
- `crates/crabcloud-fs/src/view.rs` — `View::delete` reroutes to `Trash::soft_delete`; new `View::hard_delete` for public-link bypass.
- `crates/crabcloud-config/src/types.rs` — `trash_retention_days: u32` field + default fn.
- `crates/crabcloud-config/src/test_support.rs` — fills `trash_retention_days`.
- `crates/crabcloud-core/Cargo.toml` — adds `crabcloud-trash` workspace dep.
- `crates/crabcloud-core/src/trash_sweeper.rs` (new) — `TrashSweeper::{new, run, sweep_once}`.
- `crates/crabcloud-core/src/lib.rs` — `mod trash_sweeper;` + re-export.
- `crates/crabcloud-core/src/state.rs` — `AppState.trash`, `AppState.trash_sweeper_shutdown`; construct + spawn.
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-trash` dep.
- `crates/crabcloud-http/src/routes/trashbin/` (new) — `mod.rs`, `propfind.rs`, `delete.rs`, `move_.rs`.
- `crates/crabcloud-http/src/router.rs` — mount `/dav/trashbin/{uid}` and `/remote.php/dav/trashbin/{uid}`.
- `crates/crabcloud-http/src/routes/public_link/{download,upload,mod}.rs` — switch DELETE handlers to `View::hard_delete`.
- `crates/crabcloud-http/src/routes/public_dav.rs` — switch DELETE handler to `View::hard_delete`.
- `crates/crabcloud-http/src/routes/ocs/mod.rs` — mount `apps/files_trashbin/api/v1/trashbin`.
- `crates/crabcloud-http/src/routes/ocs/files_trashbin.rs` (new) — OCS endpoints.
- `crates/crabcloud-app/Cargo.toml` — adds `crabcloud-trash` workspace dep.
- `crates/crabcloud-app/src/server_fns/mod.rs` — `pub mod trash;` + re-export.
- `crates/crabcloud-app/src/server_fns/trash.rs` (new) — 4 server fns.
- `crates/crabcloud-app/src/pages/trash.rs` (new) — the view component.
- `crates/crabcloud-app/src/pages/files/chrome.rs` — add "Deleted files" sidebar entry.
- `crates/crabcloud-app/src/pages/mod.rs` — `pub mod trash;`.
- `crates/crabcloud-app/src/app.rs` — route `/trash`.

---

# Batch A — `crabcloud-trash` core + storage rerouting

**Branch:** `sp12/a-trash-crate`

**Goal:** Stand up the trash crate (service + types + multidialect SQL + migration), reroute `View::delete` through it, add the `TrashSweeper` background task, wire everything into `AppState`. Public-link DELETE handlers are NOT touched in this batch — that's Batch B (avoids touching `crates/crabcloud-http` from two batches concurrently).

After this batch, calling `View::delete(uid, path)` from any caller (UI server fns, DAV handlers, OCS handlers) creates a trash row + moves bytes on disk. The new `View::hard_delete` exists but no caller uses it yet (Batch B switches public-link to it).

### Task A1: Migration `0009_files_trash`

**Files:**
- Create: `migrations/core/0009_files_trash/sqlite.sql`
- Create: `migrations/core/0009_files_trash/mysql.sql`
- Create: `migrations/core/0009_files_trash/postgres.sql`

- [ ] **Step 1: Confirm migration registration pattern**

  Look at `crates/crabcloud-db/src/migrations.rs` (or whichever file holds `core_set()`). New migrations register sequentially. Find the existing 0008 registration and add the 0009 entry in the same style.

- [ ] **Step 2: Write `sqlite.sql`**

  ```sql
  CREATE TABLE oc_files_trash (
      id             INTEGER PRIMARY KEY AUTOINCREMENT,
      "user"         VARCHAR(64)  NOT NULL,
      basename       VARCHAR(255) NOT NULL,
      suffix         VARCHAR(32)  NOT NULL,
      location       VARCHAR(512) NOT NULL,
      deleted_at     BIGINT       NOT NULL,
      type           VARCHAR(16)  NOT NULL,
      fileid_legacy  BIGINT       NULL
  );

  CREATE INDEX        idx_trash_user_deleted ON oc_files_trash ("user", deleted_at);
  CREATE UNIQUE INDEX idx_trash_user_name    ON oc_files_trash ("user", basename, suffix);
  ```

- [ ] **Step 3: Write `mysql.sql`**

  ```sql
  CREATE TABLE oc_files_trash (
      id             BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
      `user`         VARCHAR(64)  NOT NULL,
      basename       VARCHAR(255) NOT NULL,
      suffix         VARCHAR(32)  NOT NULL,
      location       VARCHAR(512) NOT NULL,
      deleted_at     BIGINT       NOT NULL,
      type           VARCHAR(16)  NOT NULL,
      fileid_legacy  BIGINT       NULL,
      INDEX        idx_trash_user_deleted (`user`, deleted_at),
      UNIQUE INDEX idx_trash_user_name    (`user`, basename, suffix)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
  ```

- [ ] **Step 4: Write `postgres.sql`**

  ```sql
  CREATE TABLE oc_files_trash (
      id             BIGSERIAL    PRIMARY KEY,
      "user"         VARCHAR(64)  NOT NULL,
      basename       VARCHAR(255) NOT NULL,
      suffix         VARCHAR(32)  NOT NULL,
      location       VARCHAR(512) NOT NULL,
      deleted_at     BIGINT       NOT NULL,
      type           VARCHAR(16)  NOT NULL,
      fileid_legacy  BIGINT       NULL
  );

  CREATE        INDEX idx_trash_user_deleted ON oc_files_trash ("user", deleted_at);
  CREATE UNIQUE INDEX idx_trash_user_name    ON oc_files_trash ("user", basename, suffix);
  ```

- [ ] **Step 5: Register in `crabcloud-db`**

  Add the new directory to `core_set()` in `crates/crabcloud-db/src/migrations.rs` mirroring the 0008 registration. The migration loader picks files by directory name + dialect filename.

- [ ] **Step 6: Verify migration runs cleanly**

  ```bash
  cargo test -p crabcloud-db
  ```

  Expected: all migration tests pass; the new 0009 directory is included in the registered set.

- [ ] **Step 7: Commit**

  ```bash
  git add migrations/core/0009_files_trash crates/crabcloud-db/src/migrations.rs
  git commit -m "trash: 0009_files_trash migration triplet"
  ```

### Task A2: Crate skeleton

**Files:**
- Create: `crates/crabcloud-trash/Cargo.toml`
- Create: `crates/crabcloud-trash/src/lib.rs`
- Create: `crates/crabcloud-trash/src/error.rs`
- Create: `crates/crabcloud-trash/src/types.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Register the crate in the workspace**

  In root `Cargo.toml`:
  - Add `"crates/crabcloud-trash",` to `members`.
  - Add to `[workspace.dependencies]`:
    ```toml
    crabcloud-trash = { path = "crates/crabcloud-trash" }
    ```

- [ ] **Step 2: Write `Cargo.toml`**

  ```toml
  [package]
  name = "crabcloud-trash"
  version.workspace = true
  edition.workspace = true
  license.workspace = true

  [dependencies]
  async-trait = { workspace = true }
  chrono = { workspace = true }
  crabcloud-db = { workspace = true }
  crabcloud-filecache = { workspace = true }
  crabcloud-storage = { workspace = true }
  serde = { workspace = true }
  sqlx = { workspace = true }
  thiserror = { workspace = true }
  tokio = { workspace = true, features = ["fs", "io-util"] }
  tracing = { workspace = true }

  [dev-dependencies]
  crabcloud-config = { workspace = true }
  crabcloud-db = { workspace = true, features = ["test-support"] }
  tempfile = { workspace = true }
  tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
  ```

  (Drop `features = ["test-support"]` if `crabcloud-db` doesn't declare one — check the existing `crabcloud-sharing/Cargo.toml` dev-deps shape and mirror that exactly.)

- [ ] **Step 3: Write `src/lib.rs`**

  ```rust
  //! Trash bin service for Crabcloud.
  //!
  //! Spec: `docs/superpowers/specs/2026-05-16-trash-bin-design.md`.
  //!
  //! Public entry points are [`Trash`] (CRUD operations) and the value
  //! types in [`types`]. SQL dispatch is multidialect via
  //! `match self.pool.as_ref()` mirroring `crabcloud-sharing`.

  mod error;
  mod service;
  mod sql;
  mod types;

  pub use error::TrashError;
  pub use service::Trash;
  pub use types::{RestoredTo, TrashEntry, TrashType};
  ```

- [ ] **Step 4: Write `src/error.rs`**

  ```rust
  use thiserror::Error;

  #[derive(Debug, Error)]
  pub enum TrashError {
      #[error("trash entry not found")]
      NotFound,
      #[error("trash entry belongs to a different user")]
      WrongUser,
      #[error("restore destination collision could not be resolved")]
      RestoreCollision,
      #[error("source not found in user storage")]
      SourceMissing,
      #[error("cross-storage trash not supported in MVP")]
      CrossStorage,
      #[error("io: {0}")]
      Io(#[from] std::io::Error),
      #[error("db: {0}")]
      Db(#[from] sqlx::Error),
      #[error("filecache: {0}")]
      FileCache(String),
  }
  ```

- [ ] **Step 5: Write `src/types.rs`**

  ```rust
  //! Public-facing value types for the trash service.

  use serde::{Deserialize, Serialize};

  /// A single row in `oc_files_trash`. Returned from [`crate::Trash::list`].
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct TrashEntry {
      pub id: i64,
      pub user: String,
      /// Original basename without the suffix, e.g. "report.pdf".
      pub basename: String,
      /// On-disk suffix portion, e.g. "d1716000000" (or "d1716000000_2"
      /// on collision). Combined with `basename` gives the file's name
      /// inside the user's `files_trashbin/files/` directory.
      pub suffix: String,
      /// Original parent dir at delete time, e.g. "/projects/q1". "/" for root.
      pub location: String,
      /// Unix seconds at delete time.
      pub deleted_at: i64,
      pub r#type: TrashType,
      /// Best-effort: the `oc_filecache.fileid` of the file pre-delete.
      /// Populated when the source row was findable; `None` otherwise.
      pub fileid_legacy: Option<i64>,
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "lowercase")]
  pub enum TrashType {
      File,
      Dir,
  }

  impl TrashType {
      pub fn as_str(&self) -> &'static str {
          match self {
              TrashType::File => "file",
              TrashType::Dir => "dir",
          }
      }

      pub fn from_str(s: &str) -> Option<Self> {
          match s {
              "file" => Some(Self::File),
              "dir" => Some(Self::Dir),
              _ => None,
          }
      }
  }

  /// Returned from [`crate::Trash::restore`]. Holds the path the file was
  /// actually restored to (may differ from the original `location/basename`
  /// if the caller passed an explicit destination, or if the original name
  /// collided and the service appended ` (restored)`).
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct RestoredTo {
      pub path: String,
  }
  ```

- [ ] **Step 6: Build**

  ```bash
  cargo build -p crabcloud-trash
  ```

  Expected: clean.

- [ ] **Step 7: Commit**

  ```bash
  git add Cargo.toml crates/crabcloud-trash/
  git commit -m "trash: crate skeleton (error + types + lib facade)"
  ```

### Task A3: Multidialect SQL constants

**Files:**
- Create: `crates/crabcloud-trash/src/sql.rs`

- [ ] **Step 1: Write `src/sql.rs`**

  Two query families: `_QM` (sqlite + mysql `?` placeholders) and `_PG` (postgres `$N`). Mirrors `crates/crabcloud-sharing/src/sql.rs`.

  ```rust
  //! Multidialect SQL constants for the trash service.
  //!
  //! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres
  //! (`$N`). Dispatch in `service.rs` via `match self.pool.as_ref()`.

  // -- INSERT a new trash row. Returns id via RETURNING (pg) or
  //    last_insert_rowid/last_insert_id (sqlite/mysql).
  pub const INSERT_QM: &str = "\
      INSERT INTO oc_files_trash \
      (\"user\", basename, suffix, location, deleted_at, type, fileid_legacy) \
      VALUES (?, ?, ?, ?, ?, ?, ?)";

  pub const INSERT_PG: &str = "\
      INSERT INTO oc_files_trash \
      (\"user\", basename, suffix, location, deleted_at, type, fileid_legacy) \
      VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id";

  // -- LIST all entries for one user, most-recent-first.
  pub const LIST_QM: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE \"user\" = ? ORDER BY deleted_at DESC";

  pub const LIST_PG: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE \"user\" = $1 ORDER BY deleted_at DESC";

  // -- GET one entry by id (used by restore + purge by-id).
  pub const GET_BY_ID_QM: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE id = ?";

  pub const GET_BY_ID_PG: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE id = $1";

  // -- GET one entry by (user, basename, suffix) — used by DAV handlers
  //    which receive the suffix-encoded filename.
  pub const GET_BY_NAME_QM: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE \"user\" = ? AND basename = ? AND suffix = ?";

  pub const GET_BY_NAME_PG: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE \"user\" = $1 AND basename = $2 AND suffix = $3";

  // -- DELETE one row.
  pub const DELETE_QM: &str = "DELETE FROM oc_files_trash WHERE id = ?";
  pub const DELETE_PG: &str = "DELETE FROM oc_files_trash WHERE id = $1";

  // -- DELETE all rows for a user (empty-trash).
  pub const DELETE_ALL_QM: &str = "DELETE FROM oc_files_trash WHERE \"user\" = ?";
  pub const DELETE_ALL_PG: &str = "DELETE FROM oc_files_trash WHERE \"user\" = $1";

  // -- SELECT a batch of expired rows for sweeping.
  pub const SELECT_EXPIRED_QM: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE deleted_at < ? LIMIT ?";

  pub const SELECT_EXPIRED_PG: &str = "\
      SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
      FROM oc_files_trash WHERE deleted_at < $1 LIMIT $2";

  // -- Sub-second collision probe: count suffixes matching prefix for a user.
  //    Used when we need to bump `_2`, `_3`, ... on the same `dN` second.
  pub const COUNT_SUFFIX_PREFIX_QM: &str = "\
      SELECT COUNT(*) AS n FROM oc_files_trash \
      WHERE \"user\" = ? AND basename = ? AND suffix LIKE ?";

  pub const COUNT_SUFFIX_PREFIX_PG: &str = "\
      SELECT COUNT(*) AS n FROM oc_files_trash \
      WHERE \"user\" = $1 AND basename = $2 AND suffix LIKE $3";
  ```

- [ ] **Step 2: Build**

  ```bash
  cargo build -p crabcloud-trash
  ```

  Expected: clean (no usage warnings since this is `mod sql;`-internal).

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-trash/src/sql.rs
  git commit -m "trash: multidialect SQL constants"
  ```

### Task A4: `Trash` service — TDD with sqlite e2e

**Files:**
- Create: `crates/crabcloud-trash/src/service.rs`
- Create: `crates/crabcloud-trash/tests/trash_e2e.rs`

This is the meat of Batch A. Implement the service incrementally with the e2e test as the safety net.

- [ ] **Step 1: Write the e2e test file scaffold (RED)**

  Create `crates/crabcloud-trash/tests/trash_e2e.rs`:

  ```rust
  //! sqlite e2e for the Trash service. Round-trips every public method
  //! plus the edge cases the spec calls out (collision suffixing,
  //! shared-with-me cross-user, sweeper aging).

  use crabcloud_config::test_support::minimal_sqlite_config;
  use crabcloud_db::{core_set, DbPool, MigrationRunner};
  use crabcloud_trash::{Trash, TrashType};
  use std::path::PathBuf;
  use std::sync::Arc;
  use tempfile::TempDir;

  /// Spins a fresh sqlite pool + datadir tempdir and runs all migrations.
  /// Returns the pool, the datadir, and a held-onto `TempDir` so callers
  /// keep the tempdir alive for the test's lifetime.
  async fn setup() -> (Arc<DbPool>, PathBuf, TempDir, TempDir) {
      let db_dir = TempDir::new().unwrap();
      let data_dir = TempDir::new().unwrap();
      let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
      let pool = DbPool::connect(&cfg).await.unwrap();
      let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
      runner.register(core_set());
      runner.run().await.unwrap();
      let datadir = data_dir.path().to_path_buf();
      (Arc::new(pool), datadir, db_dir, data_dir)
  }

  /// Seed: write a file inside a user's "files" dir so we can soft-delete it.
  async fn write_user_file(datadir: &PathBuf, uid: &str, rel: &str, contents: &[u8]) {
      let p = datadir.join(uid).join("files").join(rel.trim_start_matches('/'));
      tokio::fs::create_dir_all(p.parent().unwrap()).await.unwrap();
      tokio::fs::write(&p, contents).await.unwrap();
  }

  #[tokio::test]
  async fn soft_delete_writes_row_and_moves_file() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "alice", "/notes/todo.txt", b"hello").await;
      let trash = Trash::new(pool.clone(), datadir.clone());

      let id = trash
          .soft_delete("alice", "/notes/todo.txt", TrashType::File, /*fileid_legacy*/ None)
          .await
          .unwrap();
      assert!(id > 0);

      // Original gone.
      let original = datadir.join("alice/files/notes/todo.txt");
      assert!(!original.exists(), "original should be removed after soft-delete");

      // Trashbin entry present on disk under the suffix-encoded name.
      let entries: Vec<_> = std::fs::read_dir(datadir.join("alice/files_trashbin/files"))
          .unwrap()
          .filter_map(|r| r.ok())
          .collect();
      assert_eq!(entries.len(), 1);
      let name = entries[0].file_name().into_string().unwrap();
      assert!(name.starts_with("todo.txt.d"), "got name {name}");

      // List returns it.
      let listed = trash.list("alice").await.unwrap();
      assert_eq!(listed.len(), 1);
      assert_eq!(listed[0].basename, "todo.txt");
      assert_eq!(listed[0].location, "/notes");
      assert_eq!(listed[0].r#type, TrashType::File);
  }

  #[tokio::test]
  async fn restore_moves_file_back_and_deletes_row() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "bob", "/photos/cat.jpg", b"jpeg-bytes").await;
      let trash = Trash::new(pool.clone(), datadir.clone());

      let id = trash
          .soft_delete("bob", "/photos/cat.jpg", TrashType::File, None)
          .await
          .unwrap();
      let restored = trash.restore("bob", id, None).await.unwrap();
      assert_eq!(restored.path, "/photos/cat.jpg");

      // File back at original location.
      let back = datadir.join("bob/files/photos/cat.jpg");
      assert!(back.exists());
      assert_eq!(tokio::fs::read(&back).await.unwrap(), b"jpeg-bytes");

      // Trash row gone.
      assert!(trash.list("bob").await.unwrap().is_empty());
  }

  #[tokio::test]
  async fn restore_auto_creates_missing_parents() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "carol", "/a/b/c/file.txt", b"x").await;
      let trash = Trash::new(pool.clone(), datadir.clone());
      let id = trash
          .soft_delete("carol", "/a/b/c/file.txt", TrashType::File, None)
          .await
          .unwrap();
      // Remove the parent chain so restore must recreate it.
      tokio::fs::remove_dir_all(datadir.join("carol/files/a")).await.unwrap();
      let restored = trash.restore("carol", id, None).await.unwrap();
      assert_eq!(restored.path, "/a/b/c/file.txt");
      assert!(datadir.join("carol/files/a/b/c/file.txt").exists());
  }

  #[tokio::test]
  async fn restore_collision_suffixes_with_restored() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "dave", "/doc.txt", b"v1").await;
      let trash = Trash::new(pool.clone(), datadir.clone());
      let id = trash
          .soft_delete("dave", "/doc.txt", TrashType::File, None)
          .await
          .unwrap();
      // User created a new file at the same path before restoring.
      write_user_file(&datadir, "dave", "/doc.txt", b"v2").await;

      let restored = trash.restore("dave", id, None).await.unwrap();
      assert_eq!(restored.path, "/doc.txt (restored)");
      assert!(datadir.join("dave/files/doc.txt").exists());
      assert!(datadir.join("dave/files/doc.txt (restored)").exists());
  }

  #[tokio::test]
  async fn purge_deletes_row_and_file() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "eve", "/x.txt", b"z").await;
      let trash = Trash::new(pool.clone(), datadir.clone());
      let id = trash.soft_delete("eve", "/x.txt", TrashType::File, None).await.unwrap();
      trash.purge("eve", id).await.unwrap();
      assert!(trash.list("eve").await.unwrap().is_empty());
      let entries: Vec<_> = std::fs::read_dir(datadir.join("eve/files_trashbin/files"))
          .map(|d| d.filter_map(|r| r.ok()).collect())
          .unwrap_or_default();
      assert!(entries.is_empty());
  }

  #[tokio::test]
  async fn sweep_expired_deletes_old_rows_only() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "fay", "/old.txt", b"o").await;
      write_user_file(&datadir, "fay", "/new.txt", b"n").await;
      let trash = Trash::new(pool.clone(), datadir.clone());
      let old_id = trash
          .soft_delete("fay", "/old.txt", TrashType::File, None)
          .await
          .unwrap();
      let new_id = trash
          .soft_delete("fay", "/new.txt", TrashType::File, None)
          .await
          .unwrap();
      // Backdate the "old" row by 31 days.
      let cutoff = chrono::Utc::now().timestamp() - 30 * 86400;
      sqlx::query("UPDATE oc_files_trash SET deleted_at = ? WHERE id = ?")
          .bind(cutoff - 86400)
          .bind(old_id)
          .execute(match pool.as_ref() {
              crabcloud_db::DbPool::Sqlite(p) => p,
              _ => unreachable!(),
          })
          .await
          .unwrap();

      let n = trash.sweep_expired(cutoff, /*batch*/ 100).await.unwrap();
      assert_eq!(n, 1);
      let rows = trash.list("fay").await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].id, new_id);
  }

  #[tokio::test]
  async fn sub_second_collision_suffix_increments() {
      let (pool, datadir, _d, _dd) = setup().await;
      write_user_file(&datadir, "gail", "/a.txt", b"1").await;
      let trash = Trash::new(pool.clone(), datadir.clone());
      // Two soft-deletes of the same basename within one second.
      let id1 = trash.soft_delete("gail", "/a.txt", TrashType::File, None).await.unwrap();
      // Recreate the source.
      write_user_file(&datadir, "gail", "/a.txt", b"2").await;
      let id2 = trash.soft_delete("gail", "/a.txt", TrashType::File, None).await.unwrap();

      let rows = trash.list("gail").await.unwrap();
      assert_eq!(rows.len(), 2);
      assert_ne!(rows[0].suffix, rows[1].suffix, "suffixes must differ across the two deletes");
      // Both rows refer to distinct on-disk files.
      let mut names: Vec<_> = std::fs::read_dir(datadir.join("gail/files_trashbin/files"))
          .unwrap()
          .filter_map(|r| r.ok())
          .map(|e| e.file_name().into_string().unwrap())
          .collect();
      names.sort();
      assert_eq!(names.len(), 2);
      let _ = (id1, id2);
  }
  ```

- [ ] **Step 2: Run the test (RED — service.rs doesn't exist)**

  ```bash
  cargo test -p crabcloud-trash --test trash_e2e
  ```

  Expected: compile failure on `Trash::new`, `Trash::soft_delete`, etc.

- [ ] **Step 3: Write `src/service.rs`**

  ```rust
  //! `Trash` — soft-delete + list + restore + purge + sweep.
  //!
  //! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
  //! Trashbin layout on disk: `<datadir>/<uid>/files_trashbin/files/<basename>.<suffix>`.
  //! Restored files go back to `<datadir>/<uid>/files/<location>/<basename>`,
  //! creating missing parents and suffixing the basename with ` (restored)`
  //! on collision.

  use crate::error::TrashError;
  use crate::sql;
  use crate::types::{RestoredTo, TrashEntry, TrashType};
  use crabcloud_db::DbPool;
  use sqlx::Row as _;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  /// Cap on restore-collision suffix attempts before giving up.
  const RESTORE_COLLISION_CAP: u32 = 99;

  #[derive(Clone)]
  pub struct Trash {
      pool: Arc<DbPool>,
      /// Filesystem root that contains `<uid>/files/...` and `<uid>/files_trashbin/...`.
      /// Same value as `FileConfig::datadirectory`.
      datadir: PathBuf,
  }

  impl Trash {
      pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
          Self { pool, datadir }
      }

      // -------- soft_delete --------

      /// Move `<datadir>/<uid>/files/<src_path>` to the trashbin and write
      /// the metadata row. Returns the new trash row id.
      pub async fn soft_delete(
          &self,
          uid: &str,
          src_path: &str,
          r#type: TrashType,
          fileid_legacy: Option<i64>,
      ) -> Result<i64, TrashError> {
          let src_path = src_path.trim_start_matches('/').to_string();
          let basename = Path::new(&src_path)
              .file_name()
              .and_then(|s| s.to_str())
              .ok_or(TrashError::SourceMissing)?
              .to_string();
          let location = match Path::new(&src_path).parent().and_then(|p| p.to_str()) {
              Some("") | None => "/".to_string(),
              Some(parent) => format!("/{parent}"),
          };

          let now = chrono::Utc::now().timestamp();
          let suffix = self.resolve_unique_suffix(uid, &basename, now).await?;
          let trash_dir = self.datadir.join(uid).join("files_trashbin").join("files");
          tokio::fs::create_dir_all(&trash_dir).await?;
          let src = self.datadir.join(uid).join("files").join(&src_path);
          let dst = trash_dir.join(format!("{basename}.{suffix}"));
          if !tokio::fs::try_exists(&src).await? {
              return Err(TrashError::SourceMissing);
          }
          tokio::fs::rename(&src, &dst).await?;

          let id = self
              .insert_row(uid, &basename, &suffix, &location, now, r#type, fileid_legacy)
              .await?;
          Ok(id)
      }

      async fn resolve_unique_suffix(
          &self,
          uid: &str,
          basename: &str,
          now_secs: i64,
      ) -> Result<String, TrashError> {
          let base = format!("d{now_secs}");
          let like = format!("{base}%");
          let n: i64 = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_QM)
                  .bind(uid).bind(basename).bind(&like)
                  .fetch_one(p).await?.try_get("n")?,
              DbPool::MySql(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_QM)
                  .bind(uid).bind(basename).bind(&like)
                  .fetch_one(p).await?.try_get("n")?,
              DbPool::Postgres(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_PG)
                  .bind(uid).bind(basename).bind(&like)
                  .fetch_one(p).await?.try_get("n")?,
          };
          Ok(if n == 0 { base } else { format!("{base}_{n_plus}", n_plus = n + 1) })
      }

      async fn insert_row(
          &self,
          uid: &str,
          basename: &str,
          suffix: &str,
          location: &str,
          deleted_at: i64,
          r#type: TrashType,
          fileid_legacy: Option<i64>,
      ) -> Result<i64, TrashError> {
          let ty = r#type.as_str();
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(uid).bind(basename).bind(suffix).bind(location)
                      .bind(deleted_at).bind(ty).bind(fileid_legacy)
                      .execute(p).await?;
                  Ok(r.last_insert_rowid())
              }
              DbPool::MySql(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(uid).bind(basename).bind(suffix).bind(location)
                      .bind(deleted_at).bind(ty).bind(fileid_legacy)
                      .execute(p).await?;
                  Ok(r.last_insert_id() as i64)
              }
              DbPool::Postgres(p) => {
                  let row = sqlx::query(sql::INSERT_PG)
                      .bind(uid).bind(basename).bind(suffix).bind(location)
                      .bind(deleted_at).bind(ty).bind(fileid_legacy)
                      .fetch_one(p).await?;
                  Ok(row.try_get::<i64, _>("id")?)
              }
          }
      }

      // -------- list --------

      pub async fn list(&self, uid: &str) -> Result<Vec<TrashEntry>, TrashError> {
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::LIST_QM).bind(uid).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::LIST_QM).bind(uid).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::LIST_PG).bind(uid).fetch_all(p).await?,
          };
          rows.into_iter().map(|r| row_to_entry(&r)).collect()
      }

      pub async fn get_by_id(&self, id: i64) -> Result<TrashEntry, TrashError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::GET_BY_ID_QM).bind(id).fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::GET_BY_ID_QM).bind(id).fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::GET_BY_ID_PG).bind(id).fetch_optional(p).await?,
          };
          row.map(|r| row_to_entry(&r)).transpose()?.ok_or(TrashError::NotFound)
      }

      pub async fn get_by_name(
          &self,
          uid: &str,
          basename: &str,
          suffix: &str,
      ) -> Result<TrashEntry, TrashError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::GET_BY_NAME_QM)
                  .bind(uid).bind(basename).bind(suffix).fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::GET_BY_NAME_QM)
                  .bind(uid).bind(basename).bind(suffix).fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::GET_BY_NAME_PG)
                  .bind(uid).bind(basename).bind(suffix).fetch_optional(p).await?,
          };
          row.map(|r| row_to_entry(&r)).transpose()?.ok_or(TrashError::NotFound)
      }

      // -------- restore --------

      /// Restore `id`. If `dest_override` is None, restore to the row's
      /// original `location/basename`. Caller (DAV MOVE) may pass an
      /// explicit destination ("/dav/files/<uid>/foo/bar" reduced to
      /// "/foo/bar").
      pub async fn restore(
          &self,
          uid: &str,
          id: i64,
          dest_override: Option<&str>,
      ) -> Result<RestoredTo, TrashError> {
          let entry = self.get_by_id(id).await?;
          if entry.user != uid {
              return Err(TrashError::WrongUser);
          }
          let target_dir_rel = match dest_override {
              Some(d) => parent_of(d.trim_start_matches('/')).to_string(),
              None => entry.location.trim_start_matches('/').to_string(),
          };
          let target_basename = match dest_override {
              Some(d) => Path::new(d).file_name().and_then(|s| s.to_str()).unwrap_or(&entry.basename).to_string(),
              None => entry.basename.clone(),
          };

          let target_dir_abs = self.datadir.join(uid).join("files").join(&target_dir_rel);
          tokio::fs::create_dir_all(&target_dir_abs).await?;

          // Collision-resolved final filename inside target_dir_abs.
          let (final_name, final_rel) = pick_non_colliding_name(
              &target_dir_abs, &target_dir_rel, &target_basename,
          ).await?;

          let src = self.datadir
              .join(uid)
              .join("files_trashbin")
              .join("files")
              .join(format!("{}.{}", entry.basename, entry.suffix));
          if !tokio::fs::try_exists(&src).await? {
              return Err(TrashError::SourceMissing);
          }
          let dst = target_dir_abs.join(&final_name);
          tokio::fs::rename(&src, &dst).await?;

          self.delete_row(id).await?;
          Ok(RestoredTo { path: final_rel })
      }

      // -------- purge --------

      pub async fn purge(&self, uid: &str, id: i64) -> Result<(), TrashError> {
          let entry = self.get_by_id(id).await?;
          if entry.user != uid {
              return Err(TrashError::WrongUser);
          }
          self.purge_entry(&entry).await
      }

      /// Empty the user's bin. Returns count of rows removed.
      pub async fn purge_all(&self, uid: &str) -> Result<u64, TrashError> {
          let rows = self.list(uid).await?;
          let mut n = 0u64;
          for e in rows {
              self.purge_entry(&e).await?;
              n += 1;
          }
          Ok(n)
      }

      async fn purge_entry(&self, entry: &TrashEntry) -> Result<(), TrashError> {
          let src = self.datadir
              .join(&entry.user)
              .join("files_trashbin")
              .join("files")
              .join(format!("{}.{}", entry.basename, entry.suffix));
          if tokio::fs::try_exists(&src).await? {
              // Files: remove_file. Directories: remove_dir_all.
              if entry.r#type == TrashType::Dir {
                  tokio::fs::remove_dir_all(&src).await?;
              } else {
                  tokio::fs::remove_file(&src).await?;
              }
          }
          self.delete_row(entry.id).await?;
          Ok(())
      }

      async fn delete_row(&self, id: i64) -> Result<(), TrashError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => { sqlx::query(sql::DELETE_QM).bind(id).execute(p).await?; }
              DbPool::MySql(p) => { sqlx::query(sql::DELETE_QM).bind(id).execute(p).await?; }
              DbPool::Postgres(p) => { sqlx::query(sql::DELETE_PG).bind(id).execute(p).await?; }
          }
          Ok(())
      }

      // -------- sweep_expired --------

      /// Delete rows with `deleted_at < cutoff`. Returns the count
      /// deleted. Best-effort: file-removal errors on individual entries
      /// are logged but don't abort the sweep.
      pub async fn sweep_expired(&self, cutoff: i64, batch: i64) -> Result<u64, TrashError> {
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::SELECT_EXPIRED_QM)
                  .bind(cutoff).bind(batch).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::SELECT_EXPIRED_QM)
                  .bind(cutoff).bind(batch).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::SELECT_EXPIRED_PG)
                  .bind(cutoff).bind(batch).fetch_all(p).await?,
          };
          let mut n = 0u64;
          for r in rows {
              let entry = row_to_entry(&r)?;
              if let Err(e) = self.purge_entry(&entry).await {
                  tracing::warn!(error = %e, id = entry.id, "trash sweep: purge failed");
                  continue;
              }
              n += 1;
          }
          Ok(n)
      }
  }

  fn row_to_entry(r: &sqlx::any::AnyRow) -> Result<TrashEntry, TrashError> {
      // sqlx::any rows aren't actually used here — keep simple per-dialect
      // generic. The concrete `Row` types each impl `Row` trait so try_get
      // works uniformly.
      let _ = r;
      unreachable!("row_to_entry is implemented per-dialect via the macro below")
  }
  ```

  **Note on `row_to_entry`:** sqlx's per-dialect `Row` types don't share a single concrete enum like the pool does. Replace the placeholder above with a small helper macro OR inline the extraction inside each `match self.pool.as_ref()` arm. The cleanest pattern (mirrored from `crabcloud-sharing/src/service/mod.rs`) is a per-dialect closure or an inline `let entry = TrashEntry { id: r.try_get("id")?, ... };` block per arm. Pick the same shape `crabcloud-sharing` uses (look at how it materializes `ShareRow` from each dialect's rows).

  After studying the sharing crate's pattern, replace the `row_to_entry` placeholder with the appropriate per-dialect inlining. If that pattern uses a helper trait or a closure, mirror it here.

  Also add `parent_of` and `pick_non_colliding_name` helpers at module scope:

  ```rust
  /// Strip the last path segment. "a/b/c" -> "a/b", "a" -> "", "" -> "".
  fn parent_of(p: &str) -> &str {
      match p.rfind('/') {
          Some(i) => &p[..i],
          None => "",
      }
  }

  /// Find a free filename inside `target_dir_abs` starting with `basename`,
  /// then `<stem> (restored)<ext>`, then `<stem> (restored 2)<ext>`, etc.
  /// Returns `(final_name, final_rel)` where `final_rel` is the full
  /// user-relative path (e.g. "/foo/bar.txt (restored)").
  async fn pick_non_colliding_name(
      target_dir_abs: &Path,
      target_dir_rel: &str,
      basename: &str,
  ) -> Result<(String, String), TrashError> {
      let (stem, ext) = split_stem_ext(basename);
      for n in 0..=RESTORE_COLLISION_CAP {
          let candidate = match n {
              0 => basename.to_string(),
              1 => format!("{stem} (restored){ext}"),
              k => format!("{stem} (restored {k}){ext}"),
          };
          if !tokio::fs::try_exists(target_dir_abs.join(&candidate)).await? {
              let rel = if target_dir_rel.is_empty() {
                  format!("/{candidate}")
              } else {
                  format!("/{target_dir_rel}/{candidate}")
              };
              return Ok((candidate, rel));
          }
      }
      Err(TrashError::RestoreCollision)
  }

  fn split_stem_ext(name: &str) -> (String, String) {
      match name.rfind('.') {
          Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
          _ => (name.to_string(), String::new()),
      }
  }
  ```

- [ ] **Step 4: Iterate against the e2e test until GREEN**

  ```bash
  cargo test -p crabcloud-trash --test trash_e2e
  ```

  All 7 tests must pass. Common compile issues to expect:
  - sqlx `Row` extraction differs by dialect — copy the pattern from `crates/crabcloud-sharing/src/service/mod.rs`.
  - `chrono::Utc::now().timestamp()` returns `i64` directly; no conversion needed.

- [ ] **Step 5: Add unit tests for the pure helpers**

  Add to bottom of `service.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn parent_of_handles_root_and_nested() {
          assert_eq!(parent_of("foo.txt"), "");
          assert_eq!(parent_of("a/foo.txt"), "a");
          assert_eq!(parent_of("a/b/c/foo.txt"), "a/b/c");
          assert_eq!(parent_of(""), "");
      }

      #[test]
      fn split_stem_ext_typical_cases() {
          assert_eq!(split_stem_ext("doc.txt"), ("doc".into(), ".txt".into()));
          assert_eq!(split_stem_ext("noext"), ("noext".into(), "".into()));
          assert_eq!(split_stem_ext(".hidden"), (".hidden".into(), "".into()));
          assert_eq!(split_stem_ext("a.b.c"), ("a.b".into(), ".c".into()));
      }
  }
  ```

  Run `cargo test -p crabcloud-trash` — everything passes.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-trash/src/service.rs crates/crabcloud-trash/tests/trash_e2e.rs
  git commit -m "trash: Trash service (soft_delete / list / restore / purge / sweep)"
  ```

### Task A5: `TrashSweeper` background task

**Files:**
- Create: `crates/crabcloud-core/src/trash_sweeper.rs`
- Modify: `crates/crabcloud-core/src/lib.rs`
- Modify: `crates/crabcloud-core/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-trash` dep to `crabcloud-core`**

  In `crates/crabcloud-core/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-trash = { workspace = true }
  ```

- [ ] **Step 2: Write `src/trash_sweeper.rs`**

  Mirror the shape of `preview_cache_cleanup.rs` exactly.

  ```rust
  //! Background task: daily sweep of `oc_files_trash` that purges rows
  //! older than `retention_days`. Mirrors the
  //! `PreviewCacheCleanup` / `MailQueueCleanup` shape: cooperative
  //! shutdown via `Arc<Notify>`, `sweep_once()` for sync test drive.

  use crabcloud_trash::Trash;
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::sync::Notify;

  /// 24-hour sleep between sweeps.
  const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
  /// Cap on rows per pass so a giant backlog can't starve other tasks.
  const SWEEP_BATCH: i64 = 500;

  #[derive(Clone)]
  pub struct TrashSweeper {
      trash: Arc<Trash>,
      retention: chrono::Duration,
      shutdown: Arc<Notify>,
  }

  impl TrashSweeper {
      pub fn new(trash: Arc<Trash>, retention_days: u32) -> (Self, Arc<Notify>) {
          let shutdown = Arc::new(Notify::new());
          (
              Self {
                  trash,
                  retention: chrono::Duration::seconds(retention_days as i64 * 86400),
                  shutdown: shutdown.clone(),
              },
              shutdown,
          )
      }

      /// Long-running loop. Cancels cooperatively when shutdown notified.
      pub async fn run(self) {
          loop {
              if let Err(e) = self.sweep_once().await {
                  tracing::warn!(error = %e, "trash sweeper: sweep_once failed");
              }
              tokio::select! {
                  _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                  _ = self.shutdown.notified() => return,
              }
          }
      }

      /// Drive a single sweep. Returns count of rows purged. Retention 0
      /// disables sweeping (returns Ok(0) without scanning).
      pub async fn sweep_once(&self) -> Result<u64, crabcloud_trash::TrashError> {
          let secs = self.retention.num_seconds();
          if secs <= 0 {
              return Ok(0);
          }
          let cutoff = chrono::Utc::now().timestamp() - secs;
          self.trash.sweep_expired(cutoff, SWEEP_BATCH).await
      }
  }
  ```

- [ ] **Step 3: Wire into `lib.rs`**

  In `crates/crabcloud-core/src/lib.rs`:
  ```rust
  mod trash_sweeper;
  pub use trash_sweeper::TrashSweeper;
  ```

  (Insert in alphabetical position next to the existing `mail_*` modules.)

- [ ] **Step 4: Build**

  ```bash
  cargo build -p crabcloud-core
  ```

  Expected: clean.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-core/Cargo.toml crates/crabcloud-core/src/trash_sweeper.rs crates/crabcloud-core/src/lib.rs
  git commit -m "trash: TrashSweeper background task (daily age-based purge)"
  ```

### Task A6: Config knob `trash_retention_days`

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`
- Modify: `crates/crabcloud-config/src/test_support.rs`

- [ ] **Step 1: Add field + default fn in `types.rs`**

  Add after the existing `preview_retention_days` field on `FileConfig`:
  ```rust
  /// How many days to keep trash bin entries before the daily sweeper
  /// purges them. `0` disables sweeping (manual purge only).
  #[serde(default = "default_trash_retention_days")]
  pub trash_retention_days: u32,
  ```

  And add the default fn next to the other `default_*_retention_days`:
  ```rust
  fn default_trash_retention_days() -> u32 {
      30
  }
  ```

  Update `FileConfig::default()` (the impl block at the bottom of `types.rs`) to set `trash_retention_days: default_trash_retention_days()`.

- [ ] **Step 2: Update `test_support.rs::minimal_sqlite_config`**

  Add `trash_retention_days: 30,` next to the existing `preview_retention_days` line in the struct literal.

- [ ] **Step 3: Build and test**

  ```bash
  cargo test -p crabcloud-config
  ```

  Expected: all pass; defaults parse correctly.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/crabcloud-config/src/types.rs crates/crabcloud-config/src/test_support.rs
  git commit -m "trash: trash_retention_days config knob (default 30)"
  ```

### Task A7: Wire `Trash` + `TrashSweeper` into `AppState`

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Add `crabcloud_trash::Trash` import**

  At the top of `state.rs` with the other `use crabcloud_*` lines.

- [ ] **Step 2: Add two fields to `AppState`**

  Find the existing `mail_*` field block; add after `expiration_sweeper_shutdown`:
  ```rust
  /// Trash bin service. Cheap to clone.
  pub trash: Arc<crabcloud_trash::Trash>,
  /// Trash sweeper shutdown handle. Always present; spawned unconditionally
  /// in `AppStateBuilder::build`.
  pub trash_sweeper_shutdown: Arc<tokio::sync::Notify>,
  ```

- [ ] **Step 3: Construct `Trash` and spawn the sweeper in `build()`**

  In `AppStateBuilder::build`, before the existing `let state = AppState { ... }` block (and after `preview_cache_cleanup` wiring):

  ```rust
  let trash = Arc::new(crabcloud_trash::Trash::new(
      Arc::new(pool.clone()),
      self.config.datadirectory.clone(),
  ));
  let (trash_sweeper, trash_sweeper_shutdown) =
      crate::trash_sweeper::TrashSweeper::new(trash.clone(), self.config.trash_retention_days);
  // Always spawned (trash exists regardless of mail transport). The
  // JoinHandle is intentionally dropped; the task terminates on shutdown.
  std::mem::drop(tokio::spawn(async move { trash_sweeper.run().await }));
  ```

- [ ] **Step 4: Add to the `AppState` struct literal**

  ```rust
  trash,
  trash_sweeper_shutdown,
  ```

- [ ] **Step 5: Build the workspace**

  ```bash
  cargo build --workspace
  ```

  Expected: clean.

- [ ] **Step 6: Run the AppState build tests**

  ```bash
  cargo test -p crabcloud-core state::tests
  ```

  Expected: all pass (the existing `build_assembles_state_from_minimal_config` test now constructs the trash service + spawns the sweeper).

- [ ] **Step 7: Commit**

  ```bash
  git add crates/crabcloud-core/src/state.rs
  git commit -m "trash: wire Trash + TrashSweeper into AppState"
  ```

### Task A8: Reroute `View::delete` + add `View::hard_delete`

**Files:**
- Modify: `crates/crabcloud-fs/Cargo.toml`
- Modify: `crates/crabcloud-fs/src/view.rs` (and wherever `delete` actually lives — may be a sibling module like `view/delete.rs`)

- [ ] **Step 1: Add `crabcloud-trash` dep**

  In `crates/crabcloud-fs/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-trash = { workspace = true }
  ```

- [ ] **Step 2: Find the current `View::delete` impl**

  Run `grep "pub async fn delete" crates/crabcloud-fs/src/`. Note the file + line; understand what it returns and what it does today (likely a pass-through to `Mount::delete` via the underlying storage trait).

- [ ] **Step 3: Add a `trash` handle to `View`**

  `View::new` currently takes `(uid, mounts, filecache, storage_sink)`. Add a 5th parameter `trash: Arc<crabcloud_trash::Trash>`. Update `View::new`. Update every call site (grep `View::new(`) — there will be several across `crabcloud-core::state::AppState::view_for`, `crabcloud-app::server_fns`, test fixtures, etc.

  Then store it on `View`:
  ```rust
  pub struct View {
      // ... existing fields ...
      trash: Arc<crabcloud_trash::Trash>,
  }
  ```

  (If `View::new` is already painful and a `ViewConfig` would help, mirror the SP12 polish-C `SharesConfig` pattern. Out of scope unless the call-site update is unwieldy.)

- [ ] **Step 4: Rename the existing `delete` to `hard_delete` and add the new `delete` wrapper**

  In the same file:
  ```rust
  impl View {
      /// Public-link / admin / sweeper-side bypass: removes the file
      /// without creating a trash entry. Use only when the caller has
      /// explicit authority to skip trash (anonymous public-link DELETE,
      /// the trash sweeper itself, etc.).
      pub async fn hard_delete(&self, path: &UserPath) -> FsResult<()> {
          // ... existing delete body (rename of the original impl) ...
      }

      /// Soft-delete: routes through the trash service. Authed UI / DAV /
      /// OCS surfaces all reach this entry point.
      pub async fn delete(&self, path: &UserPath) -> FsResult<()> {
          // Look up the type and fileid_legacy from filecache before the
          // bytes move (the soft_delete itself moves them; we want the
          // pre-move metadata).
          let storage_path = /* resolve UserPath to (storage_id, StoragePath) via mounts */;
          let row = self.filecache.lookup(&storage_id, &storage_path).await?;
          let r#type = match row.as_ref().map(|r| r.kind) {
              Some(crabcloud_filecache::Kind::Directory) => crabcloud_trash::TrashType::Dir,
              _ => crabcloud_trash::TrashType::File,
          };
          let fileid_legacy = row.as_ref().map(|r| r.fileid);

          self.trash
              .soft_delete(
                  self.uid.as_str(),
                  path.as_str(),
                  r#type,
                  fileid_legacy,
              )
              .await
              .map_err(map_trash_err)?;

          // Notify storage_sink so the scanner sees the disappearance
          // (matches what hard_delete used to do).
          let _ = self.storage_sink.publish(/* StorageEvent::Removed { ... } */);
          Ok(())
      }
  }

  fn map_trash_err(e: crabcloud_trash::TrashError) -> FsError {
      use crabcloud_trash::TrashError::*;
      match e {
          NotFound | SourceMissing => FsError::NotFound,
          WrongUser => FsError::Forbidden,
          RestoreCollision => FsError::Conflict,
          CrossStorage => FsError::Unsupported,
          Io(e) => FsError::Io(e),
          Db(e) => FsError::Storage(format!("trash db: {e}")),
          FileCache(s) => FsError::Storage(format!("trash filecache: {s}")),
      }
  }
  ```

  **Note:** The placeholder "resolve UserPath to (storage_id, StoragePath)" and the `StorageEvent::Removed` shape must be adapted to the actual `crabcloud-fs` types. Read the original `delete` body — it almost certainly already does this resolve via `self.resolve(path)` or similar. Mirror that, but call `self.trash.soft_delete` instead of `mount.delete`.

  **Subtle:** `soft_delete` does its own on-disk rename. Don't *also* call the storage backend's delete. That would either error (file already moved) or worse, delete from the wrong place if the trash rename failed silently. The soft-delete path is the single source of truth for "bytes go away from `<uid>/files/...`".

- [ ] **Step 5: Pass `state.trash.clone()` into `View::new` from the constructor sites**

  - `AppState::view_for` in `crates/crabcloud-core/src/state.rs`: thread `self.trash.clone()` through.
  - `AppState::uploads_for` likely doesn't need it (uploads aren't trash-aware).
  - Any test fixture that builds a `View` directly: pass an `Arc<Trash>` (test helper: `let trash = Arc::new(Trash::new(pool.clone(), datadir.clone()))`).

- [ ] **Step 6: Run the existing `crabcloud-fs` tests**

  ```bash
  cargo test -p crabcloud-fs
  ```

  Expected: most pass; any that called `View::delete` and expected hard-delete semantics need adjustment. Specifically: if a test asserted "after delete, file is gone from disk", it still passes (the file IS gone from `<uid>/files/...` — it's now in `<uid>/files_trashbin/files/...`). If a test asserted "after delete, no `oc_files_trash` row exists", that test needs reconsidering (the new behavior IS a trash row).

  For any test that needs the old hard-delete semantics, switch it to `view.hard_delete(...)`.

- [ ] **Step 7: Add a focused regression test for the reroute**

  In `crates/crabcloud-fs/tests/` (or wherever delete tests live), add:

  ```rust
  #[tokio::test]
  async fn view_delete_creates_trash_row_and_keeps_bytes() {
      // Setup: AppState with a user 'alice' and a seeded file '/notes/x.txt'.
      // ... call view.delete("/notes/x.txt").await.unwrap();
      // Assert: trash list for alice has 1 entry with basename "x.txt", location "/notes".
      // Assert: bytes live under <datadir>/alice/files_trashbin/files/x.txt.d<ts>.
      // Assert: <datadir>/alice/files/notes/x.txt does NOT exist.
  }

  #[tokio::test]
  async fn view_hard_delete_does_not_create_trash_row() {
      // Same setup; call view.hard_delete(...).await.unwrap();
      // Assert: trash list for alice is empty.
      // Assert: <datadir>/alice/files/notes/x.txt does NOT exist.
      // Assert: <datadir>/alice/files_trashbin/ does not exist (no trashbin dir created).
  }
  ```

  Fill in the setup per the existing test patterns in `crabcloud-fs/tests/`.

- [ ] **Step 8: Run the full workspace tests**

  ```bash
  cargo test --workspace
  ```

  Fix any regressions surfaced by the reroute. Likely candidates: DAV DELETE e2e tests, OCS files delete tests, server-fn delete tests. Each can either keep using `View::delete` (and gain trash semantics) or switch to `View::hard_delete` if they explicitly need the old behavior.

- [ ] **Step 9: Commit**

  ```bash
  git add crates/crabcloud-fs/ crates/crabcloud-core/src/state.rs
  git commit -m "trash: route View::delete through Trash::soft_delete; add View::hard_delete"
  ```

### Task A9: Batch A pre-PR

- [ ] **Step 1: Full pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push and open PR**

  ```bash
  git push -u origin sp12/a-trash-crate
  gh pr create --title "sp12(a): crabcloud-trash crate + View::delete reroute + sweeper" \
    --body "Implements Batch A of the SP12 trash bin plan: new crabcloud-trash crate, 0009_files_trash migration, View::delete reroute to soft-delete, View::hard_delete for the public-link bypass (wired in Batch B), TrashSweeper background task, trash_retention_days config (default 30), AppState wiring. See docs/superpowers/specs/2026-05-16-trash-bin-design.md and docs/superpowers/plans/2026-05-16-trash-bin-implementation.md."
  ```

---

# Batch B — DAV `/dav/trashbin/{uid}/...` surface

**Branch:** `sp12/b-dav` (off the merged Batch A master)

**Goal:** Add the DAV trashbin endpoint matching Nextcloud's wire shape, and switch the public-link DELETE handlers to `View::hard_delete`. After this batch, desktop / KIO clients see and manipulate the trash bin via the standard `/dav/trashbin/{uid}/...` paths.

### Task B1: Trashbin router module + handler skeletons

**Files:**
- Create: `crates/crabcloud-http/src/routes/trashbin/mod.rs`
- Create: `crates/crabcloud-http/src/routes/trashbin/propfind.rs`
- Create: `crates/crabcloud-http/src/routes/trashbin/delete.rs`
- Create: `crates/crabcloud-http/src/routes/trashbin/move_.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs` (add `pub mod trashbin;`)
- Modify: `crates/crabcloud-http/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-trash` dep**

  In `crates/crabcloud-http/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-trash = { workspace = true }
  ```

- [ ] **Step 2: Write `routes/trashbin/mod.rs`**

  ```rust
  //! DAV `/dav/trashbin/{uid}/...` surface.
  //!
  //! Routes are nest-relative; mount via `Router::nest("/dav/trashbin", ...)`
  //! and `Router::nest("/remote.php/dav/trashbin", ...)` in `router.rs`.
  //!
  //! Inside this namespace:
  //!   PROPFIND /{uid}/                                 — list root
  //!   PROPFIND /{uid}/trash/{basename_dot_suffix}      — single entry
  //!   DELETE   /{uid}/trash/{basename_dot_suffix}      — purge
  //!   MOVE     /{uid}/trash/{basename_dot_suffix}      — restore
  //!   *        anything else                            — 405
  //!
  //! The `{basename_dot_suffix}` segment is `<basename>.<suffix>` —
  //! matches Nextcloud's wire shape so desktop clients work without
  //! translation.

  mod delete;
  mod move_;
  mod propfind;

  use axum::routing::{any, on, MethodFilter};
  use axum::Router;
  use crabcloud_core::AppState;

  pub fn router() -> Router<AppState> {
      Router::new()
          // Root: PROPFIND only.
          .route(
              "/{uid}/",
              on(MethodFilter::from_bits(0).expect("custom method handled via any"),
                 |req, state| async { propfind::root(state, req).await })
                  .fallback(any(method_not_allowed)),
          )
          // Per-entry routes. axum routes match by method, so PROPFIND, DELETE,
          // MOVE each get their own handler.
          .route(
              "/{uid}/trash/{name}",
              any(per_entry_dispatch),
          )
  }

  async fn per_entry_dispatch(
      axum::extract::State(state): axum::extract::State<AppState>,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      match req.method().as_str() {
          "PROPFIND" => propfind::entry(state, req).await,
          "DELETE"   => delete::purge(state, req).await,
          "MOVE"     => move_::restore(state, req).await,
          _ => method_not_allowed().await,
      }
  }

  async fn method_not_allowed() -> axum::response::Response {
      use axum::response::IntoResponse;
      (axum::http::StatusCode::METHOD_NOT_ALLOWED, "").into_response()
  }
  ```

  **Note on dispatch:** axum 0.8 doesn't have first-class extension method support; the pattern above uses `any()` + manual method match. If `crates/crabcloud-http/src/routes/dav/mod.rs` already establishes a different DAV dispatch pattern (look at how it handles PROPFIND / MOVE / COPY), mirror that pattern instead — consistency with the authed DAV surface beats reinventing.

- [ ] **Step 3: Stub the three handler modules**

  `propfind.rs`:
  ```rust
  use axum::response::Response;
  use crabcloud_core::AppState;

  pub async fn root(
      _state: AppState,
      _req: axum::http::Request<axum::body::Body>,
  ) -> Response {
      todo!("Task B2")
  }

  pub async fn entry(
      _state: AppState,
      _req: axum::http::Request<axum::body::Body>,
  ) -> Response {
      todo!("Task B2")
  }
  ```

  `delete.rs`:
  ```rust
  use axum::response::Response;
  use crabcloud_core::AppState;

  pub async fn purge(
      _state: AppState,
      _req: axum::http::Request<axum::body::Body>,
  ) -> Response {
      todo!("Task B3")
  }
  ```

  `move_.rs`:
  ```rust
  use axum::response::Response;
  use crabcloud_core::AppState;

  pub async fn restore(
      _state: AppState,
      _req: axum::http::Request<axum::body::Body>,
  ) -> Response {
      todo!("Task B4")
  }
  ```

- [ ] **Step 4: Add `pub mod trashbin;` in `routes/mod.rs`**

- [ ] **Step 5: Build**

  ```bash
  cargo build -p crabcloud-http
  ```

  Expected: clean.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-http/
  git commit -m "trash dav: router skeleton + stubbed handlers"
  ```

### Task B2: `PROPFIND` — list + per-entry

**Files:**
- Modify: `crates/crabcloud-http/src/routes/trashbin/propfind.rs`
- Reference: existing `routes/dav/propfind.rs` for the XML envelope pattern

- [ ] **Step 1: Study the existing PROPFIND helpers**

  Read `crates/crabcloud-http/src/routes/dav/propfind.rs` and `dav/headers.rs`. Identify:
  - The XML envelope helper (likely emits `<d:multistatus>` with `<d:response>` children).
  - The Depth header parser (`Depth: 0` / `Depth: 1` / `Depth: infinity`).
  - The auth extractor (`AuthenticatedUser` or similar) — make sure trashbin uses the same one.

- [ ] **Step 2: Implement `root()` for `PROPFIND /{uid}/`**

  - Extract the authed `uid` via `AuthenticatedUser`.
  - Confirm the `{uid}` path param matches the authed user (otherwise 403).
  - On `Depth: 0`: return a single `<d:response>` for the trash root with `<d:resourcetype><d:collection/></d:resourcetype>`.
  - On `Depth: 1`: also include one `<d:response>` per trash entry returned by `state.trash.list(uid)`.
  - For each entry, the `<d:href>` is `/dav/trashbin/{uid}/trash/{basename}.{suffix}` (URL-encode the basename). The properties to emit:
    - `<d:displayname>{basename}</d:displayname>`
    - `<d:getlastmodified>` = HTTP-date format of `deleted_at` (Unix → RFC 2822).
    - `<d:resourcetype>` = empty for files, `<d:collection/>` for dirs.
    - `<d:getcontentlength>{size}</d:getcontentlength>` — for files, stat the on-disk file to get the size. (For directories, omit.)
    - `<nc:trashbin-original-location xmlns:nc="http://nextcloud.org/ns">{location}/{basename}</nc:trashbin-original-location>`

  Build the response body as a `String` and return with `Content-Type: application/xml; charset=utf-8` and `Status: 207 Multi-Status`.

  Code shape (concrete):
  ```rust
  pub async fn root(
      state: AppState,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      use axum::response::IntoResponse;
      use http::StatusCode;

      // Authed user.
      let authed = match crate::middleware::auth::AuthenticatedUser::from_request(&req) {
          Ok(u) => u,
          Err(r) => return r,
      };
      let uid_param = match path_param(&req, "uid") {
          Some(u) => u,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      if uid_param != authed.uid.as_str() {
          return (StatusCode::FORBIDDEN, "").into_response();
      }
      let depth = parse_depth(req.headers());

      let mut xml = String::new();
      xml.push_str(r#"<?xml version="1.0" encoding="utf-8"?>"#);
      xml.push_str(r#"<d:multistatus xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns">"#);
      // Root response.
      push_response_root(&mut xml, &uid_param);

      if matches!(depth, Depth::One | Depth::Infinity) {
          let entries = match state.trash.list(&uid_param).await {
              Ok(v) => v,
              Err(e) => {
                  tracing::warn!(error = %e, "trash list failed");
                  return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
              }
          };
          for e in entries {
              let size = file_size_for_entry(&state, &e).await;
              push_response_entry(&mut xml, &uid_param, &e, size);
          }
      }
      xml.push_str("</d:multistatus>");
      ([(http::header::CONTENT_TYPE, "application/xml; charset=utf-8")],
       (StatusCode::from_u16(207).unwrap(), xml)).into_response()
  }
  ```

  Helpers (`push_response_root`, `push_response_entry`, `file_size_for_entry`, `parse_depth`, `path_param`): write them in the same file. `path_param` extracts a named segment from the matched route (axum API: `req.extensions().get::<MatchedPath>()` plus index — or use `axum::extract::Path::<HashMap<String,String>>` from the request, whichever the rest of the codebase prefers; mirror `routes/public_link/mod.rs` extraction style).

  `parse_depth` returns an enum:
  ```rust
  enum Depth { Zero, One, Infinity }
  fn parse_depth(h: &http::HeaderMap) -> Depth {
      match h.get("Depth").and_then(|v| v.to_str().ok()) {
          Some("0") => Depth::Zero,
          Some("infinity") | None => Depth::Infinity,
          _ => Depth::One,
      }
  }
  ```

- [ ] **Step 3: Implement `entry()` for `PROPFIND /{uid}/trash/{name}`**

  Same pattern but with `{name}` parsed via `split_basename_and_suffix` (helper: rsplit on last `.d` segment; e.g. `report.pdf.d1716000000` → `("report.pdf", "d1716000000")`). Look up via `state.trash.get_by_name(uid, basename, suffix)`. Emit a single `<d:response>` wrapped in `<d:multistatus>`. Return 207. 404 if `get_by_name` returns `TrashError::NotFound`.

- [ ] **Step 4: Add an e2e test**

  Create `crates/crabcloud-http/tests/dav_trashbin_propfind.rs` (mirror existing `dav_propfind.rs` setup):
  - Setup AppState + authed user 'alice'.
  - Seed a file '/notes/x.txt'; call `state.shares.create` — no wait, just call `view.delete` directly (or `state.trash.soft_delete` directly with the appropriate uid + path + type).
  - Hit `PROPFIND /dav/trashbin/alice/` with `Depth: 1` and Basic auth.
  - Assert status 207, XML contains `<d:href>/dav/trashbin/alice/trash/x.txt.d`.
  - Assert XML contains the `nc:trashbin-original-location` element with value `/notes/x.txt`.

  Then PROPFIND the per-entry path and assert the single-entry XML shape.

- [ ] **Step 5: Run and iterate**

  ```bash
  cargo test -p crabcloud-http --test dav_trashbin_propfind
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-http/src/routes/trashbin/propfind.rs crates/crabcloud-http/tests/dav_trashbin_propfind.rs
  git commit -m "trash dav: PROPFIND root + per-entry"
  ```

### Task B3: `DELETE` — purge

**Files:**
- Modify: `crates/crabcloud-http/src/routes/trashbin/delete.rs`
- Create: `crates/crabcloud-http/tests/dav_trashbin_delete.rs`

- [ ] **Step 1: Implement `purge`**

  ```rust
  pub async fn purge(
      state: AppState,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      use axum::response::IntoResponse;
      use http::StatusCode;

      let authed = match crate::middleware::auth::AuthenticatedUser::from_request(&req) {
          Ok(u) => u,
          Err(r) => return r,
      };
      let uid_param = match path_param(&req, "uid") {
          Some(u) => u,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      if uid_param != authed.uid.as_str() {
          return (StatusCode::FORBIDDEN, "").into_response();
      }
      let name = match path_param(&req, "name") {
          Some(n) => n,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      let (basename, suffix) = match split_basename_and_suffix(&name) {
          Some(p) => p,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      let entry = match state.trash.get_by_name(&uid_param, &basename, &suffix).await {
          Ok(e) => e,
          Err(crabcloud_trash::TrashError::NotFound) => return (StatusCode::NOT_FOUND, "").into_response(),
          Err(e) => {
              tracing::warn!(error = %e, "trash get_by_name failed");
              return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
          }
      };
      match state.trash.purge(&uid_param, entry.id).await {
          Ok(()) => (StatusCode::NO_CONTENT, "").into_response(),
          Err(e) => {
              tracing::warn!(error = %e, "trash purge failed");
              (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
          }
      }
  }

  // Co-located helpers; can later move to a sibling `helpers.rs` if both
  // delete.rs and move_.rs end up sharing more than this.
  pub(super) fn path_param(req: &axum::http::Request<axum::body::Body>, name: &str) -> Option<String> {
      // Use whatever pattern the rest of crabcloud-http does — likely
      // `axum::extract::Path::<HashMap<String,String>>::from_request_parts`
      // or `req.extensions().get::<MatchedPath>()` + manual parse.
      todo!("mirror routes/public_link extraction style")
  }

  pub(super) fn split_basename_and_suffix(name: &str) -> Option<(String, String)> {
      // The on-disk filename is `<basename>.<suffix>` where suffix starts
      // with 'd' followed by digits (and optionally `_n`). Find the last
      // `.d<digits>` boundary.
      let bytes = name.as_bytes();
      let mut last_d = None;
      let mut i = 0;
      while i < bytes.len() {
          if bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1] == b'd' {
              last_d = Some(i);
          }
          i += 1;
      }
      let dot = last_d?;
      let (basename, dot_suffix) = name.split_at(dot);
      let suffix = &dot_suffix[1..]; // strip the leading '.'
      // Validate: suffix is "d<digits>" or "d<digits>_<digits>".
      if !suffix.starts_with('d') {
          return None;
      }
      let rest = &suffix[1..];
      let valid = match rest.split_once('_') {
          Some((a, b)) => a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()),
          None => rest.chars().all(|c| c.is_ascii_digit()),
      };
      if !valid { return None; }
      Some((basename.to_string(), suffix.to_string()))
  }
  ```

  Also add a small unit test below for `split_basename_and_suffix`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn split_basename_and_suffix_typical() {
          let (b, s) = split_basename_and_suffix("report.pdf.d1716000000").unwrap();
          assert_eq!(b, "report.pdf");
          assert_eq!(s, "d1716000000");
      }
      #[test]
      fn split_basename_and_suffix_with_collision() {
          let (b, s) = split_basename_and_suffix("a.txt.d1716000000_2").unwrap();
          assert_eq!(b, "a.txt");
          assert_eq!(s, "d1716000000_2");
      }
      #[test]
      fn split_basename_and_suffix_rejects_non_trash_name() {
          assert!(split_basename_and_suffix("notes.txt").is_none());
          assert!(split_basename_and_suffix("notes.dfoo").is_none());
      }
  }
  ```

- [ ] **Step 2: Write the e2e test**

  Create `crates/crabcloud-http/tests/dav_trashbin_delete.rs`:
  - Setup AppState + alice + seed file.
  - Soft-delete via `state.trash.soft_delete(...)` to get a known basename+suffix.
  - Send `DELETE /dav/trashbin/alice/trash/<basename>.<suffix>` with Basic auth.
  - Assert 204; `state.trash.list("alice")` is empty.
  - Assert the on-disk file is gone.

- [ ] **Step 3: Run and iterate; commit**

  ```bash
  cargo test -p crabcloud-http --test dav_trashbin_delete
  git add crates/crabcloud-http/src/routes/trashbin/delete.rs crates/crabcloud-http/tests/dav_trashbin_delete.rs
  git commit -m "trash dav: DELETE (purge)"
  ```

### Task B4: `MOVE` — restore

**Files:**
- Modify: `crates/crabcloud-http/src/routes/trashbin/move_.rs`
- Create: `crates/crabcloud-http/tests/dav_trashbin_move.rs`

- [ ] **Step 1: Implement `restore`**

  ```rust
  pub async fn restore(
      state: AppState,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      use axum::response::IntoResponse;
      use http::StatusCode;

      let authed = match crate::middleware::auth::AuthenticatedUser::from_request(&req) {
          Ok(u) => u,
          Err(r) => return r,
      };
      let uid_param = match crate::routes::trashbin::delete::path_param(&req, "uid") {
          Some(u) => u,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      if uid_param != authed.uid.as_str() {
          return (StatusCode::FORBIDDEN, "").into_response();
      }
      let name = match crate::routes::trashbin::delete::path_param(&req, "name") {
          Some(n) => n,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };
      let (basename, suffix) = match crate::routes::trashbin::delete::split_basename_and_suffix(&name) {
          Some(p) => p,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };

      let entry = match state.trash.get_by_name(&uid_param, &basename, &suffix).await {
          Ok(e) => e,
          Err(crabcloud_trash::TrashError::NotFound) => return (StatusCode::NOT_FOUND, "").into_response(),
          Err(e) => {
              tracing::warn!(error = %e, "trash get_by_name failed");
              return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
          }
      };

      // Parse Destination header (optional). If present, must point at
      // `/dav/files/<uid>/<path>` or `/remote.php/dav/files/<uid>/<path>`.
      let dest_override = match req.headers().get("Destination").and_then(|v| v.to_str().ok()) {
          Some(s) => match parse_destination(s, &uid_param) {
              Ok(p) => Some(p),
              Err(e) => return e,
          },
          None => None,
      };

      let restored = match state.trash.restore(&uid_param, entry.id, dest_override.as_deref()).await {
          Ok(r) => r,
          Err(crabcloud_trash::TrashError::RestoreCollision) => {
              return (StatusCode::CONFLICT, "").into_response();
          }
          Err(crabcloud_trash::TrashError::WrongUser) => {
              return (StatusCode::FORBIDDEN, "").into_response();
          }
          Err(e) => {
              tracing::warn!(error = %e, "trash restore failed");
              return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
          }
      };

      let _ = restored;
      (StatusCode::CREATED, "").into_response()
  }

  /// Strip the surface prefix and the `/files/<uid>/` segment from a
  /// `Destination` header; return the user-relative path. Rejects
  /// destinations outside the user's files namespace.
  fn parse_destination(dest: &str, uid: &str) -> Result<String, axum::response::Response> {
      use axum::response::IntoResponse;
      use http::StatusCode;
      // Strip absolute URL prefix if present (some clients send full URLs).
      let path = match dest.find("://") {
          Some(_) => {
              let after_scheme = dest.split_once("://").unwrap().1;
              match after_scheme.find('/') {
                  Some(i) => after_scheme[i..].to_string(),
                  None => return Err((StatusCode::BAD_REQUEST, "").into_response()),
              }
          }
          None => dest.to_string(),
      };
      // Accept both surface prefixes.
      let prefixes = [
          format!("/remote.php/dav/files/{uid}/"),
          format!("/dav/files/{uid}/"),
      ];
      for p in &prefixes {
          if let Some(rest) = path.strip_prefix(p.as_str()) {
              return Ok(rest.to_string());
          }
      }
      Err((StatusCode::BAD_REQUEST, "").into_response())
  }
  ```

- [ ] **Step 2: Write the e2e test**

  Create `crates/crabcloud-http/tests/dav_trashbin_move.rs`:
  - Setup + soft-delete.
  - Test A: `MOVE /dav/trashbin/alice/trash/x.txt.d<ts>` WITHOUT a Destination header. Expect 201; `state.trash.list("alice")` empty; file back at original location.
  - Test B: same, but with `Destination: /dav/files/alice/restored-elsewhere/x.txt`. Expect 201; file at the new location.
  - Test C: pre-create a file at the restore target so the restore collides. Expect 201; file restored at `x.txt (restored)`.
  - Test D: collision-cap exhaustion — out of scope (test would be slow); skip.
  - Test E: Destination outside `/dav/files/alice/` → 400.

- [ ] **Step 3: Run and iterate; commit**

  ```bash
  cargo test -p crabcloud-http --test dav_trashbin_move
  git add crates/crabcloud-http/src/routes/trashbin/move_.rs crates/crabcloud-http/tests/dav_trashbin_move.rs
  git commit -m "trash dav: MOVE (restore with optional Destination)"
  ```

### Task B5: Mount the trashbin router

**Files:**
- Modify: `crates/crabcloud-http/src/router.rs`

- [ ] **Step 1: Mount at both surface prefixes**

  In `build_router`, near the existing `dav_router` definition, add:
  ```rust
  let trashbin_router = Router::new()
      .nest(
          "/remote.php/dav/trashbin",
          crate::routes::trashbin::router().with_state(state.clone()),
      )
      .nest(
          "/dav/trashbin",
          crate::routes::trashbin::router().with_state(state.clone()),
      );
  ```

  And merge it into the final router alongside `dav_router`:
  ```rust
  Router::new()
      .merge(dav_router)
      .merge(trashbin_router)
      .merge(public_dav_router)
      // ... rest unchanged ...
  ```

- [ ] **Step 2: Build + test**

  ```bash
  cargo build -p crabcloud-http
  cargo test -p crabcloud-http
  ```

  Expected: all green.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-http/src/router.rs
  git commit -m "trash dav: mount trashbin router at /dav/trashbin and /remote.php/dav/trashbin"
  ```

### Task B6: Switch public-link DELETE to `View::hard_delete`

**Files:**
- Modify: `crates/crabcloud-http/src/routes/public_link/{mod.rs, download.rs, upload.rs}` (find the actual DELETE handler — likely in `mod.rs` or a sibling)
- Modify: `crates/crabcloud-http/src/routes/public_dav.rs`

- [ ] **Step 1: Find the DELETE handlers**

  Grep `crates/crabcloud-http/src/routes/public_link/` and `crates/crabcloud-http/src/routes/public_dav.rs` for `\.delete\(` or method matching `DELETE`. There should be two surfaces: the REST-shape public-link delete and the public-DAV delete.

- [ ] **Step 2: Replace each `view.delete(path).await` with `view.hard_delete(path).await`**

  Public-link DELETE is anonymous; the visitor has no trashbin. The fix is a one-line method-call change at each site.

- [ ] **Step 3: Add a regression test**

  Create or extend `crates/crabcloud-http/tests/public_dav_delete.rs` (or extend existing public-link e2e):
  - Setup AppState + alice + a file inside her share.
  - Issue an anonymous DELETE via `/s/<token>/<path>` (or the public-DAV equivalent).
  - Assert 204.
  - Assert `state.trash.list("alice")` is **empty** (this is the key regression assertion — no trash row created by the anonymous visitor's action).
  - Assert the on-disk file is gone.

- [ ] **Step 4: Run and iterate**

  ```bash
  cargo test -p crabcloud-http
  ```

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-http/src/routes/public_link/ crates/crabcloud-http/src/routes/public_dav.rs crates/crabcloud-http/tests/
  git commit -m "trash: public-link DELETE bypasses trash (uses View::hard_delete)"
  ```

### Task B7: Batch B pre-PR

- [ ] **Step 1: Pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push + open PR**

  ```bash
  git push -u origin sp12/b-dav
  gh pr create --title "sp12(b): DAV /dav/trashbin/{uid} (PROPFIND, DELETE, MOVE) + public-link bypass" \
    --body "Batch B of the SP12 trash bin plan: Nextcloud-compatible DAV trashbin endpoint mounted at /dav/trashbin and /remote.php/dav/trashbin. PROPFIND lists / inspects, DELETE purges, MOVE restores (with optional Destination header). Public-link DELETE handlers switched to View::hard_delete so anonymous visitors don't create trash entries."
  ```

---

# Batch C — OCS REST + server fns

**Branch:** `sp12/c-ocs-and-server-fns` (off the merged Batch B master)

**Goal:** Add the Nextcloud-shape OCS endpoints and the Dioxus `#[server]` fn equivalents. After this batch, the web UI (Batch D) can call typed server fns and third-party OCS clients can hit the standard URL space.

### Task C1: OCS endpoints

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/files_trashbin.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`

- [ ] **Step 1: Study an existing OCS module**

  Read `crates/crabcloud-http/src/routes/ocs/files_sharing.rs` (or similar). Identify the:
  - Router setup (`pub fn router() -> Router<AppState>`).
  - OCS envelope helper (something like `OcsResponse::ok(data)` or `ocs_json(status, message, data)`).
  - JSON DTO style (serde structs with `#[serde(rename_all = "camelCase")]`).

- [ ] **Step 2: Write `files_trashbin.rs`**

  Skeleton (concrete envelope helpers depend on what `files_sharing.rs` uses — mirror exactly):

  ```rust
  //! OCS endpoints for the trash bin.
  //!
  //! Nextcloud spelling: `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin`.
  //! All endpoints require the authed user; `{uid}` is implicit (the row
  //! filter is always the authed uid).
  //!
  //! GET    /trashbin              — list
  //! POST   /restore/{id}          — restore
  //! DELETE /trash/{id}            — purge one
  //! DELETE /trash                 — empty bin

  use axum::extract::{Path, State};
  use axum::routing::{delete, get, post};
  use axum::{Json, Router};
  use crabcloud_core::AppState;
  use crabcloud_trash::{TrashEntry, TrashType};
  use serde::Serialize;

  pub fn router() -> Router<AppState> {
      Router::new()
          .route("/trashbin", get(list))
          .route("/restore/{id}", post(restore))
          .route("/trash/{id}", delete(purge_one))
          .route("/trash", delete(empty_bin))
  }

  #[derive(Serialize)]
  pub struct TrashEntryDto {
      pub id: i64,
      pub basename: String,
      pub suffix: String,
      pub location: String,
      pub deleted_at: i64,
      pub r#type: String,
  }

  impl From<TrashEntry> for TrashEntryDto {
      fn from(e: TrashEntry) -> Self {
          Self {
              id: e.id,
              basename: e.basename,
              suffix: e.suffix,
              location: e.location,
              deleted_at: e.deleted_at,
              r#type: e.r#type.as_str().to_string(),
          }
      }
  }

  async fn list(
      State(state): State<AppState>,
      // authed user extractor — copy from files_sharing.rs
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      let rows = match state.trash.list(&uid).await {
          Ok(r) => r,
          Err(e) => return ocs_error(500, format!("trash list: {e}")),
      };
      let dtos: Vec<TrashEntryDto> = rows.into_iter().map(TrashEntryDto::from).collect();
      ocs_ok(Json(dtos))
  }

  async fn restore(
      State(state): State<AppState>,
      Path(id): Path<i64>,
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      match state.trash.restore(&uid, id, None).await {
          Ok(r) => ocs_ok(Json(serde_json::json!({ "path": r.path }))),
          Err(crabcloud_trash::TrashError::NotFound) => ocs_error(404, "not found".into()),
          Err(crabcloud_trash::TrashError::WrongUser) => ocs_error(403, "forbidden".into()),
          Err(crabcloud_trash::TrashError::RestoreCollision) => ocs_error(409, "collision".into()),
          Err(e) => ocs_error(500, format!("restore: {e}")),
      }
  }

  async fn purge_one(
      State(state): State<AppState>,
      Path(id): Path<i64>,
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      match state.trash.purge(&uid, id).await {
          Ok(()) => ocs_ok(Json(serde_json::json!({}))),
          Err(crabcloud_trash::TrashError::NotFound) => ocs_error(404, "not found".into()),
          Err(crabcloud_trash::TrashError::WrongUser) => ocs_error(403, "forbidden".into()),
          Err(e) => ocs_error(500, format!("purge: {e}")),
      }
  }

  async fn empty_bin(
      State(state): State<AppState>,
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      match state.trash.purge_all(&uid).await {
          Ok(n) => ocs_ok(Json(serde_json::json!({ "purged": n }))),
          Err(e) => ocs_error(500, format!("purge_all: {e}")),
      }
  }
  ```

  Replace `/* authed uid */`, `ocs_ok`, and `ocs_error` with the exact helpers used in `files_sharing.rs`. Don't invent new ones.

- [ ] **Step 3: Mount in `routes/ocs/mod.rs`**

  Add `pub mod files_trashbin;` and nest it: somewhere in the OCS router assembly, add:
  ```rust
  .nest(
      "/v2.php/apps/files_trashbin/api/v1",
      files_trashbin::router().with_state(state.clone()),
  )
  ```

  (Mirror exactly how `files_sharing` is mounted — same `nest` style, same v1/v2 spelling.)

- [ ] **Step 4: E2E test**

  Create `crates/crabcloud-http/tests/ocs_trashbin.rs`:
  - Setup + soft-delete to seed one entry.
  - GET `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin` → 200, OCS envelope decodes, contains the entry.
  - POST `/restore/<id>` → 200, file back at original location, list now empty.
  - DELETE `/trash/<id>` → 200; bin now empty.
  - DELETE `/trash` after seeding 3 entries → 200, `purged: 3`.

- [ ] **Step 5: Run and iterate; commit**

  ```bash
  cargo test -p crabcloud-http --test ocs_trashbin
  git add crates/crabcloud-http/src/routes/ocs/
  git add crates/crabcloud-http/tests/ocs_trashbin.rs
  git commit -m "trash ocs: /apps/files_trashbin/api/v1/trashbin endpoints"
  ```

### Task C2: Server fns

**Files:**
- Create: `crates/crabcloud-app/src/server_fns/trash.rs`
- Modify: `crates/crabcloud-app/src/server_fns/mod.rs`
- Modify: `crates/crabcloud-app/Cargo.toml`
- Create: `crates/crabcloud-app/tests/server_fns_trash.rs`

- [ ] **Step 1: Add `crabcloud-trash` dep**

  In `crates/crabcloud-app/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-trash = { workspace = true }
  ```

- [ ] **Step 2: Write `server_fns/trash.rs`**

  ```rust
  //! `#[server]` functions for the Dioxus trash view. Mirror the OCS
  //! surface (Batch C) but with typed inputs / outputs the UI can call
  //! directly without round-tripping through OCS JSON.

  use crate::server_fns::AuthenticatedUser;  // adjust path if extractor lives elsewhere
  use dioxus::prelude::*;
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct TrashEntryDto {
      pub id: i64,
      pub basename: String,
      pub suffix: String,
      pub location: String,
      pub deleted_at: i64,
      pub r#type: String,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct RestoredDto {
      pub path: String,
  }

  #[server]
  pub async fn list_trash() -> Result<Vec<TrashEntryDto>, ServerFnError> {
      use crabcloud_trash::TrashEntry;
      use dioxus::fullstack::FullstackContext;
      let fs = FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
      let state = fs.extension::<crabcloud_core::AppState>()
          .ok_or_else(|| ServerFnError::new("AppState missing"))?;
      let authed = AuthenticatedUser::from_fullstack(&fs)
          .ok_or_else(|| ServerFnError::new("unauthorized"))?;
      let rows = state.trash.list(authed.uid.as_str())
          .await
          .map_err(|e| ServerFnError::new(format!("trash list: {e}")))?;
      Ok(rows.into_iter().map(|e| TrashEntryDto {
          id: e.id,
          basename: e.basename,
          suffix: e.suffix,
          location: e.location,
          deleted_at: e.deleted_at,
          r#type: e.r#type.as_str().to_string(),
      }).collect())
  }

  #[server]
  pub async fn restore_trash(id: i64) -> Result<RestoredDto, ServerFnError> {
      use dioxus::fullstack::FullstackContext;
      let fs = FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
      let state = fs.extension::<crabcloud_core::AppState>()
          .ok_or_else(|| ServerFnError::new("AppState missing"))?;
      let authed = AuthenticatedUser::from_fullstack(&fs)
          .ok_or_else(|| ServerFnError::new("unauthorized"))?;
      state.trash.restore(authed.uid.as_str(), id, None)
          .await
          .map(|r| RestoredDto { path: r.path })
          .map_err(|e| ServerFnError::new(format!("trash restore: {e}")))
  }

  #[server]
  pub async fn purge_trash(id: i64) -> Result<(), ServerFnError> {
      use dioxus::fullstack::FullstackContext;
      let fs = FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
      let state = fs.extension::<crabcloud_core::AppState>()
          .ok_or_else(|| ServerFnError::new("AppState missing"))?;
      let authed = AuthenticatedUser::from_fullstack(&fs)
          .ok_or_else(|| ServerFnError::new("unauthorized"))?;
      state.trash.purge(authed.uid.as_str(), id)
          .await
          .map_err(|e| ServerFnError::new(format!("trash purge: {e}")))
  }

  #[server]
  pub async fn empty_trash() -> Result<u64, ServerFnError> {
      use dioxus::fullstack::FullstackContext;
      let fs = FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
      let state = fs.extension::<crabcloud_core::AppState>()
          .ok_or_else(|| ServerFnError::new("AppState missing"))?;
      let authed = AuthenticatedUser::from_fullstack(&fs)
          .ok_or_else(|| ServerFnError::new("unauthorized"))?;
      state.trash.purge_all(authed.uid.as_str())
          .await
          .map_err(|e| ServerFnError::new(format!("trash purge_all: {e}")))
  }
  ```

  Mirror the existing server-fn pattern for `AuthenticatedUser` extraction precisely — copy the import path and the `from_fullstack` / `from_request` shape from a working example like `crates/crabcloud-app/src/server_fns/files.rs` or `notification_prefs.rs`.

- [ ] **Step 3: Wire into `server_fns/mod.rs`**

  Add `pub mod trash;` and re-export the public types and fns if the module follows that pattern.

- [ ] **Step 4: Integration test**

  Create `crates/crabcloud-app/tests/server_fns_trash.rs`. Mirror `server_fns_files.rs` and `server_fns_public_link.rs` for the AppState + axum router setup. Tests:
  - Seed a file via `view.delete`; `list_trash()` returns the entry.
  - `restore_trash(id)` returns the path; subsequent `list_trash()` empty.
  - `purge_trash(id)` 200; bin empty.
  - `empty_trash()` after seeding 3 → returns 3; bin empty.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-app --test server_fns_trash
  git add crates/crabcloud-app/Cargo.toml crates/crabcloud-app/src/server_fns/
  git add crates/crabcloud-app/tests/server_fns_trash.rs
  git commit -m "trash: server fns (list / restore / purge / empty)"
  ```

### Task C3: Batch C pre-PR

- [ ] **Step 1: Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp12/c-ocs-and-server-fns
  gh pr create --title "sp12(c): OCS /apps/files_trashbin endpoints + server fns" \
    --body "Batch C of the SP12 trash bin plan: Nextcloud-shape OCS REST endpoints at /ocs/v2.php/apps/files_trashbin/api/v1/, plus #[server] fns (list_trash, restore_trash, purge_trash, empty_trash) the Dioxus UI consumes in Batch D."
  ```

---

# Batch D — Dioxus UI

**Branch:** `sp12/d-ui` (off the merged Batch C master)

**Goal:** Add a "Deleted files" sidebar entry and a trash view that lists entries with Restore / Delete permanently per-row and Empty trash as a bulk action.

### Task D1: Sidebar entry

**Files:**
- Modify: `crates/crabcloud-app/src/pages/files/chrome.rs`

- [ ] **Step 1: Find the existing sidebar block**

  Read `chrome.rs` and find the left-sidebar render block. Currently there's only "All files".

- [ ] **Step 2: Add the "Deleted files" entry**

  Add a second link below "All files" pointing at `/trash`. Match the existing styling (CSS class, icon — pick a 🗑 emoji if the existing entry uses emoji, or a `files-icon` class if it uses CSS sprites).

  Example shape (adjust to match the actual code):
  ```rust
  rsx! {
      // ... existing "All files" link ...
      Link {
          to: "/trash",
          class: "sidebar-entry",
          span { class: "sidebar-icon", "🗑" }
          span { class: "sidebar-label", "Deleted files" }
      }
  }
  ```

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-app/src/pages/files/chrome.rs
  git commit -m "trash ui: 'Deleted files' sidebar entry"
  ```

### Task D2: Trash view component

**Files:**
- Create: `crates/crabcloud-app/src/pages/trash.rs`
- Modify: `crates/crabcloud-app/src/pages/mod.rs`
- Modify: `crates/crabcloud-app/src/app.rs`

- [ ] **Step 1: Write `pages/trash.rs`**

  Use the existing files-list components as scaffolding. The view is read-mostly: no upload zone, no inline rename, no breadcrumb (trash is flat by design). Each row shows: basename, original location (`location/`), deleted_at (formatted), and two action buttons: `Restore` and `Delete permanently`. Top bar: `Empty trash` button (with confirm).

  Concrete skeleton:

  ```rust
  //! Trash bin page.
  //!
  //! Reads trash entries via `server_fns::trash::list_trash`. Each row
  //! exposes Restore + Delete permanently. The page header has an
  //! Empty-trash button (with confirm). On any successful mutation the
  //! resource is refetched so the row disappears.

  use crate::server_fns::trash::{empty_trash, list_trash, purge_trash, restore_trash, TrashEntryDto};
  use crate::pages::files::chrome::Chrome;  // adjust path as needed
  use dioxus::prelude::*;

  #[component]
  pub fn TrashPage() -> Element {
      let mut refresh = use_signal(|| 0u64);
      let entries = use_resource(move || async move {
          let _ = refresh();
          list_trash().await
      });

      let on_restore = {
          let mut refresh = refresh.clone();
          move |id: i64| {
              spawn(async move {
                  if let Err(e) = restore_trash(id).await {
                      tracing::warn!(error = %e, id, "restore failed");
                  }
                  refresh.set(refresh() + 1);
              });
          }
      };

      let on_purge = {
          let mut refresh = refresh.clone();
          move |id: i64| {
              spawn(async move {
                  if let Err(e) = purge_trash(id).await {
                      tracing::warn!(error = %e, id, "purge failed");
                  }
                  refresh.set(refresh() + 1);
              });
          }
      };

      let on_empty = {
          let mut refresh = refresh.clone();
          move |_evt: MouseEvent| {
              spawn(async move {
                  if let Err(e) = empty_trash().await {
                      tracing::warn!(error = %e, "empty_trash failed");
                  }
                  refresh.set(refresh() + 1);
              });
          }
      };

      rsx! {
          Chrome {
              div { class: "trash-page",
                  div { class: "trash-header",
                      h2 { "Deleted files" }
                      button {
                          class: "trash-empty-btn",
                          onclick: on_empty,
                          "Empty trash"
                      }
                  }
                  match &*entries.read_unchecked() {
                      Some(Ok(rows)) if rows.is_empty() => rsx! {
                          p { class: "trash-empty", "Nothing in trash." }
                      },
                      Some(Ok(rows)) => rsx! {
                          ul { class: "trash-list",
                              for entry in rows.iter() {
                                  TrashRow {
                                      key: "{entry.id}",
                                      entry: entry.clone(),
                                      on_restore: on_restore.clone(),
                                      on_purge: on_purge.clone(),
                                  }
                              }
                          }
                      },
                      Some(Err(e)) => rsx! { p { class: "trash-error", "Error: {e}" } },
                      None => rsx! { p { class: "trash-loading", "Loading..." } },
                  }
              }
          }
      }
  }

  #[derive(Props, Clone, PartialEq)]
  struct TrashRowProps {
      entry: TrashEntryDto,
      on_restore: EventHandler<i64>,
      on_purge: EventHandler<i64>,
  }

  #[component]
  fn TrashRow(props: TrashRowProps) -> Element {
      let id = props.entry.id;
      let basename = props.entry.basename.clone();
      let location = props.entry.location.clone();
      let deleted_at = props.entry.deleted_at;
      let when = format_deleted_at(deleted_at);

      rsx! {
          li { class: "trash-row",
              span { class: "trash-row-icon",
                  if props.entry.r#type == "dir" { "📁" } else { "📄" }
              }
              span { class: "trash-row-name", "{basename}" }
              span { class: "trash-row-location", "from {location}" }
              span { class: "trash-row-when", "{when}" }
              div { class: "trash-row-actions",
                  button {
                      class: "trash-restore-btn",
                      onclick: move |_| props.on_restore.call(id),
                      "Restore"
                  }
                  button {
                      class: "trash-purge-btn",
                      onclick: move |_| props.on_purge.call(id),
                      "Delete permanently"
                  }
              }
          }
      }
  }

  fn format_deleted_at(unix_secs: i64) -> String {
      // Use whatever date-formatting helper the existing files row uses.
      // If none exists, fall back to a simple "N hours ago" or RFC 3339.
      use chrono::{TimeZone, Utc};
      Utc.timestamp_opt(unix_secs, 0)
          .single()
          .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
          .unwrap_or_else(|| unix_secs.to_string())
  }
  ```

  Adjust the `Chrome { ... }` import to match the actual component the files page uses for top bar + sidebar wrapping; if there's no such wrapper component, render the trash view stand-alone (the chrome lives in the App-level layout already).

- [ ] **Step 2: Wire into `pages/mod.rs`**

  ```rust
  pub mod trash;
  ```

- [ ] **Step 3: Register the route in `app.rs`**

  Find the router definition. Add:
  ```rust
  Route { to: "/trash", element: rsx! { TrashPage {} } }
  ```

  Adjust to match the dioxus-router 0.7 macro syntax actually used in the project (likely `#[route("/trash")]` enum variant or the `Router` macro DSL — copy the shape from the existing files route).

- [ ] **Step 4: SSR snapshot test**

  In `crates/crabcloud-app/tests/`, add or extend a snapshot test that:
  - Renders the trash page in SSR with a stubbed `list_trash` returning two known entries.
  - Asserts the rendered HTML contains the basenames and the "Restore" / "Delete permanently" button text.

  Mirror the pattern used by other page SSR tests in this crate (probably uses `dioxus::server::ssr::SsrRenderer` or `crate::server::render`).

- [ ] **Step 5: Manual smoke test**

  ```bash
  cargo build -p crabcloud-app
  # Run dev server per project convention, log in, click "Deleted files",
  # verify the page renders + a soft-deleted file appears + Restore moves
  # it back + Empty trash works.
  ```

  Note in the PR description that manual smoke was performed.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-app/src/pages/trash.rs \
          crates/crabcloud-app/src/pages/mod.rs \
          crates/crabcloud-app/src/app.rs \
          crates/crabcloud-app/tests/
  git commit -m "trash ui: trash view + route + sidebar wire-up"
  ```

### Task D3: Batch D pre-PR

- [ ] **Step 1: Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp12/d-ui
  gh pr create --title "sp12(d): trash bin UI (sidebar + view + actions)" \
    --body "Batch D of the SP12 trash bin plan: 'Deleted files' sidebar entry, /trash route, TrashPage component with per-row Restore + Delete permanently and bulk Empty trash. Calls into the server fns from Batch C."
  ```

---

## Self-review notes

- **Spec coverage:** Every section in `2026-05-16-trash-bin-design.md` maps to a task. §1 goal → all batches. §2 decisions → Tasks A1–A8 (decisions 1–10), B1–B5 (11), C1 (12), C2 (14), D1–D2 (13). §3 architecture → Tasks A2–A8. §4 schema → A1. §5 surfaces → B + C. §6 edge cases → covered by tests in A4, A8, B4 (collision), B6 (public-link bypass), A5 (retention 0 — implicit in `sweep_once` early return). §7 testing list → tests across A4 (unit + e2e), A5 (sweeper sync), A8 (View reroute), B2/B3/B4 (DAV), B6 (public-link bypass), C1 (OCS), C2 (server-fn integration), D2 (SSR snapshot). §8 batches → 4 batches.
- **Placeholder scan:** A few `todo!` macros in handler skeletons that will be replaced in the implementing task; clearly marked with the implementing task name. No vague "add error handling" or "fill in details".
- **Type consistency:** `TrashEntry` shape is consistent A2→A4→C1→C2; `RestoredTo`/`RestoredDto` matches across A2/C2. `TrashType::{File,Dir}` consistent A2→A4→A8→C1/C2. `Trash::soft_delete` parameter order matches between A4 definition and A8 caller.
- **Known underspecified spots** (call out for the implementer to resolve from the codebase, not from this plan):
  - The exact `AuthenticatedUser` extractor path / shape — copied from a working server-fn rather than re-derived.
  - The exact OCS envelope helpers (`ocs_ok` / `ocs_error`) — mirror `files_sharing.rs`.
  - The exact axum `Path` extractor style for the trashbin router — mirror `public_link/mod.rs`.
  - The `View::new` signature change ripples to many call sites; A8 asks the implementer to grep + update each one.
