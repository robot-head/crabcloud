# File Versioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Nextcloud-compatible file versioning. Every byte-changing write (`View::write_file` / `View::move_with_overwrite`) of a non-empty file snapshots the prior bytes into the owner's `files_versions/` tree with a metadata row in `oc_files_versions`. Surfaces: tiered-retention background sweeper, DAV `/dav/versions/{uid}/{fileid}`, OCS `/ocs/v2.php/apps/files_versions/api/v1/...`, server fns, and a per-row "Versions" panel in the Dioxus files page. Restores are lossless (snapshot-then-replace).

**Architecture:** New `crabcloud-versions` crate owns the `Versions` service (`snapshot_if_needed` / `list_for` / `restore` / `delete` / `sweep_tiered` / `purge_for_fileid`) backed by `oc_files_versions` + on-disk files at `<datadir>/<uid>/files_versions/<relative>.v<mtime>`. `crabcloud-fs::View` calls `Versions::snapshot_if_needed` before each byte-changing write. `crabcloud-core::VersionsSweeper` runs daily with the tiered retention rules (every / hourly / daily / weekly). `Trash::purge_entry` cascades into `Versions::purge_for_fileid` so hard-deletes don't leave orphan version bytes.

**Tech Stack:** Rust 1.95, sqlx 0.8 (sqlite + mysql + postgres), axum 0.8, Dioxus 0.7 fullstack. No new external dependencies.

**Spec:** `docs/superpowers/specs/2026-05-17-file-versioning-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is its own PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp13/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-versions-crate`, `b-dav`, `c-ocs-and-server-fns`, `d-ui`.

- **Commit cadence:** Commit at every "Commit" step. Each batch lands as a single squash-merged PR; intermediate commits get squashed.

- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- **Merge:** After CI green and user merges via GitHub UI.

- **Established workaround for AppState tests:** Tests building `AppState` set `cfg.filecache.enabled = false`, `cfg.mail.transport = "disabled"`, and `cfg.trash_retention_days = 30`. Add: `cfg.versions_retention_disabled = false`, `cfg.versions_min_interval_secs = 2`, `cfg.versions_max_bytes = 1024 * 1024 * 1024` (1 GiB). Override per-test only when the test specifically exercises sweeper / throttle / size-cap behavior.

- **Pre-existing patterns to mirror:**
  - **Crate shape:** `crates/crabcloud-trash/` (SP12) — focused service crate, multidialect SQL via `match self.pool.as_ref()`, per-dialect inline row decode, error type in `error.rs`, types in `types.rs`.
  - **Background sweeper:** `crates/crabcloud-core/src/trash_sweeper.rs` and `preview_cache_cleanup.rs` — `pub fn new(...) -> (Self, Arc<Notify>)`, `pub async fn run(self)` with `tokio::select!` shutdown, `pub async fn sweep_once()` for sync test drive.
  - **Migration triplet:** `migrations/core/0009_files_trash/{sqlite,mysql,postgres}.sql`. Next migration number is `0010`.
  - **DAV handlers:** `crates/crabcloud-http/src/routes/trashbin/` (SP12 Batch B) — mod.rs dispatch + per-method files, suffix-encoded resource names, `Allow:` header on 405, `Router::nest` at two prefix aliases.
  - **OCS shape:** `crates/crabcloud-http/src/routes/ocs/files_trashbin.rs` and the shared `envelope.rs` (from SP12 polish G).
  - **Server fns:** `crates/crabcloud-app/src/server_fns/trash.rs` — `require_user()` extractor, `map_err` helper that centralizes error strings.
  - **UI page:** `crates/crabcloud-app/src/pages/trash.rs` — per-row in-flight tracking, dismissable error banner, confirm modal reusing `.files-modal-*` chrome.
  - **Cascade purge pattern:** `crates/crabcloud-trash/src/service.rs::purge_entry` will need to call `versions.purge_for_fileid(fileid)` — see Task A8.

---

## File-by-file map

### New crate: `crabcloud-versions`

```
crates/crabcloud-versions/
├── Cargo.toml
├── src/
│   ├── lib.rs       — re-exports + crate doc
│   ├── error.rs     — VersionsError
│   ├── service.rs   — Versions struct + snapshot_if_needed / list_for / restore / delete / sweep_tiered / purge_for_fileid / get_by_id
│   ├── sql.rs       — multidialect SQL constants
│   └── types.rs     — VersionEntry
└── tests/
    └── versions_e2e.rs   — sqlite e2e (snapshot + list + restore round-trip + sweeper + throttle + size cap + cascade purge)
```

### New migration

```
migrations/core/0010_files_versions/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-versions` member.
- `crates/crabcloud-fs/Cargo.toml` — adds `crabcloud-versions` workspace dep.
- `crates/crabcloud-fs/src/view.rs` — `View::write_file` + `View::move_with_overwrite` call `Versions::snapshot_if_needed`. `View::new` gains a 6th parameter `versions: Arc<crabcloud_versions::Versions>` and stores it.
- `crates/crabcloud-config/src/types.rs` — `versions_min_interval_secs: u32`, `versions_max_bytes: u64`, `versions_retention_disabled: bool` fields + default fns.
- `crates/crabcloud-config/src/test_support.rs` — fills the three new fields.
- `crates/crabcloud-core/Cargo.toml` — adds `crabcloud-versions` workspace dep.
- `crates/crabcloud-core/src/versions_sweeper.rs` (new) — `VersionsSweeper::{new, run, sweep_once}`.
- `crates/crabcloud-core/src/lib.rs` — `mod versions_sweeper;` + re-export.
- `crates/crabcloud-core/src/state.rs` — `AppState.versions`, `AppState.versions_sweeper_shutdown`; construct + spawn.
- `crates/crabcloud-trash/Cargo.toml` — adds `crabcloud-versions` workspace dep.
- `crates/crabcloud-trash/src/service.rs` — `Trash::purge_entry` calls `versions.purge_for_fileid(fileid)`. `Trash::new` gains a `versions: Arc<crabcloud_versions::Versions>` field; the `TrashConfig` shape (if it exists) gets a new field, or we introduce one.
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-versions` dep.
- `crates/crabcloud-http/src/routes/versions/` (new) — `mod.rs`, `propfind.rs`, `get.rs`, `copy.rs`.
- `crates/crabcloud-http/src/router.rs` — mount `/dav/versions/{uid}/{fileid}` and `/remote.php/dav/versions/{uid}/{fileid}`.
- `crates/crabcloud-http/src/routes/ocs/files_versions.rs` (new) — OCS endpoints.
- `crates/crabcloud-http/src/routes/ocs/mod.rs` — mount `apps/files_versions/api/v1/`.
- `crates/crabcloud-app/Cargo.toml` — adds `crabcloud-versions` workspace dep.
- `crates/crabcloud-app/src/server_fns/mod.rs` — `pub mod versions;` + re-export.
- `crates/crabcloud-app/src/server_fns/versions.rs` (new) — 3 server fns.
- `crates/crabcloud-app/src/pages/files/row.rs` (or wherever the row × menu lives) — add "Versions" menu item.
- `crates/crabcloud-app/src/pages/files/mod.rs` — wire a `VersionsPanel` modal triggered from the row.
- `crates/crabcloud-app/src/pages/files/versions_panel.rs` (new) — the panel component.
- `crates/crabcloud-app/assets/app.css` — versions panel + row styles.

---

# Batch A — `crabcloud-versions` core + triggers + sweeper + trash cascade

**Branch:** `sp13/a-versions-crate`

**Goal:** Stand up the versions crate, the 0010 migration, the `VersionsSweeper`, the three config knobs, wire `View::write_file` + `View::move_with_overwrite` to snapshot via the new service, and have `Trash::purge_entry` cascade into `Versions::purge_for_fileid`.

After this batch:
- Every authed PUT / MOVE-with-overwrite of a non-empty file snapshots the prior bytes (subject to throttle + size cap) into `<owner>/files_versions/...` with an `oc_files_versions` row.
- `VersionsSweeper` runs daily; `versions_retention_disabled = true` short-circuits.
- Hard-deletes via trash cascade-purge version rows + bytes.
- No surface yet — UI / DAV / OCS land in B/C/D.

### Task A1: Migration `0010_files_versions`

**Files:**
- Create: `migrations/core/0010_files_versions/sqlite.sql`
- Create: `migrations/core/0010_files_versions/mysql.sql`
- Create: `migrations/core/0010_files_versions/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs` (or wherever `core_set()` lives — grep)

- [ ] **Step 1: Confirm migration registration pattern**

  Look at the existing `0009_files_trash` registration in the core migrations file. New entry registers identically.

- [ ] **Step 2: Write `sqlite.sql`**

  ```sql
  CREATE TABLE oc_files_versions (
      id             INTEGER PRIMARY KEY AUTOINCREMENT,
      storage_id     BIGINT       NOT NULL,
      fileid         BIGINT       NOT NULL,
      "user"         VARCHAR(64)  NOT NULL,
      path           VARCHAR(512) NOT NULL,
      version_mtime  BIGINT       NOT NULL,
      size           BIGINT       NOT NULL
  );

  CREATE INDEX idx_versions_user_fileid    ON oc_files_versions ("user", fileid);
  CREATE INDEX idx_versions_user_mtime     ON oc_files_versions ("user", version_mtime);
  CREATE INDEX idx_versions_storage_fileid ON oc_files_versions (storage_id, fileid);
  ```

- [ ] **Step 3: Write `mysql.sql`**

  ```sql
  CREATE TABLE oc_files_versions (
      id             BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
      storage_id     BIGINT       NOT NULL,
      fileid         BIGINT       NOT NULL,
      `user`         VARCHAR(64)  NOT NULL,
      path           VARCHAR(512) NOT NULL,
      version_mtime  BIGINT       NOT NULL,
      size           BIGINT       NOT NULL,
      INDEX idx_versions_user_fileid    (`user`, fileid),
      INDEX idx_versions_user_mtime     (`user`, version_mtime),
      INDEX idx_versions_storage_fileid (storage_id, fileid)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
  ```

- [ ] **Step 4: Write `postgres.sql`**

  ```sql
  CREATE TABLE oc_files_versions (
      id             BIGSERIAL    PRIMARY KEY,
      storage_id     BIGINT       NOT NULL,
      fileid         BIGINT       NOT NULL,
      "user"         VARCHAR(64)  NOT NULL,
      path           VARCHAR(512) NOT NULL,
      version_mtime  BIGINT       NOT NULL,
      size           BIGINT       NOT NULL
  );

  CREATE INDEX idx_versions_user_fileid    ON oc_files_versions ("user", fileid);
  CREATE INDEX idx_versions_user_mtime     ON oc_files_versions ("user", version_mtime);
  CREATE INDEX idx_versions_storage_fileid ON oc_files_versions (storage_id, fileid);
  ```

- [ ] **Step 5: Register in core migrations**

  Add the new directory to `core_set()` mirroring the 0009 registration.

- [ ] **Step 6: Verify migration runs**

  ```bash
  cargo test -p crabcloud-db
  ```

  Expected: all migration tests pass; the new 0010 directory is registered.

- [ ] **Step 7: Commit**

  ```bash
  git add migrations/core/0010_files_versions crates/crabcloud-db/src/core_migrations.rs
  git commit -m "versions: 0010_files_versions migration triplet"
  ```

### Task A2: Crate skeleton

**Files:**
- Create: `crates/crabcloud-versions/Cargo.toml`
- Create: `crates/crabcloud-versions/src/lib.rs`
- Create: `crates/crabcloud-versions/src/error.rs`
- Create: `crates/crabcloud-versions/src/types.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Register the crate in the workspace**

  In root `Cargo.toml`:
  - Add `"crates/crabcloud-versions",` to `members`.
  - Add to `[workspace.dependencies]`:
    ```toml
    crabcloud-versions = { path = "crates/crabcloud-versions" }
    ```

- [ ] **Step 2: Write `Cargo.toml`**

  Mirror `crates/crabcloud-trash/Cargo.toml` exactly. Substitute crate name; the dep set is identical.

  ```toml
  [package]
  name = "crabcloud-versions"
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
  tempfile = { workspace = true }
  tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
  ```

- [ ] **Step 3: Write `src/lib.rs`**

  ```rust
  //! File versioning service for Crabcloud.
  //!
  //! Spec: `docs/superpowers/specs/2026-05-17-file-versioning-design.md`.
  //!
  //! Public entry points are [`Versions`] and the value types in [`types`].
  //! SQL dispatch is multidialect via `match self.pool.as_ref()` mirroring
  //! `crabcloud-trash` / `crabcloud-sharing`.

  mod error;
  mod service;
  mod sql;
  mod types;

  pub use error::VersionsError;
  pub use service::Versions;
  pub use types::VersionEntry;
  ```

- [ ] **Step 4: Write `src/error.rs`**

  ```rust
  use thiserror::Error;

  #[derive(Debug, Error)]
  pub enum VersionsError {
      #[error("version row not found")]
      NotFound,
      #[error("version belongs to a different user")]
      WrongUser,
      #[error("source missing on disk")]
      SourceMissing,
      #[error("io: {0}")]
      Io(#[from] std::io::Error),
      #[error("db: {0}")]
      Db(#[from] sqlx::Error),
  }
  ```

- [ ] **Step 5: Write `src/types.rs`**

  ```rust
  //! Public-facing value types for the versions service.

  use serde::{Deserialize, Serialize};

  /// A single row in `oc_files_versions`. Returned from
  /// [`crate::Versions::list_for`].
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct VersionEntry {
      pub id: i64,
      pub storage_id: i64,
      pub fileid: i64,
      pub user: String,
      pub path: String,
      /// Unix seconds at snapshot time. Matches the on-disk suffix
      /// `<path>.v<version_mtime>`.
      pub version_mtime: i64,
      pub size: i64,
  }
  ```

- [ ] **Step 6: Build**

  ```bash
  cargo build -p crabcloud-versions
  ```

  Expected: clean. The `service` and `sql` modules don't exist yet — the build will fail on `pub use service::Versions;` if those modules are missing. Add minimal stubs so the crate compiles in this step OR include the stubs (see steps below) before building.

- [ ] **Step 7: Stub `src/service.rs` and `src/sql.rs`**

  Add these minimal stubs so the crate compiles (they'll be implemented in A3/A4):

  ```rust
  // crates/crabcloud-versions/src/sql.rs
  //! Multidialect SQL constants. Filled in Task A3.
  ```

  ```rust
  // crates/crabcloud-versions/src/service.rs
  //! Versions service. Filled in Task A4.

  use crabcloud_db::DbPool;
  use std::path::PathBuf;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Versions {
      #[allow(dead_code)]
      pool: Arc<DbPool>,
      #[allow(dead_code)]
      datadir: PathBuf,
  }

  impl Versions {
      pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
          Self { pool, datadir }
      }
  }
  ```

- [ ] **Step 8: Build**

  ```bash
  cargo build -p crabcloud-versions
  ```

  Expected: clean.

- [ ] **Step 9: Commit**

  ```bash
  git add Cargo.toml crates/crabcloud-versions/
  git commit -m "versions: crate skeleton (error + types + lib facade)"
  ```

### Task A3: Multidialect SQL constants

**Files:**
- Modify: `crates/crabcloud-versions/src/sql.rs`

- [ ] **Step 1: Write the constants**

  Mirror `crabcloud-trash/src/sql.rs`'s `_QM` (sqlite + mysql) vs `_PG` (postgres) split.

  ```rust
  //! Multidialect SQL constants for the versions service.
  //!
  //! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres (`$N`).
  //! Dispatch in `service.rs` via `match self.pool.as_ref()`.

  // -- INSERT a new version row. Returns id via RETURNING (pg) or
  //    last_insert_rowid/last_insert_id (sqlite/mysql).
  pub const INSERT_QM: &str = "\
      INSERT INTO oc_files_versions \
      (storage_id, fileid, \"user\", path, version_mtime, size) \
      VALUES (?, ?, ?, ?, ?, ?)";

  pub const INSERT_PG: &str = "\
      INSERT INTO oc_files_versions \
      (storage_id, fileid, \"user\", path, version_mtime, size) \
      VALUES ($1, $2, $3, $4, $5, $6) RETURNING id";

  // -- LIST all versions for a (user, fileid), newest-first.
  pub const LIST_FOR_QM: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE \"user\" = ? AND fileid = ? \
      ORDER BY version_mtime DESC";

  pub const LIST_FOR_PG: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE \"user\" = $1 AND fileid = $2 \
      ORDER BY version_mtime DESC";

  // -- GET one by id (restore + delete + cascade lookup).
  pub const GET_BY_ID_QM: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE id = ?";

  pub const GET_BY_ID_PG: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE id = $1";

  // -- GET most-recent version for throttle check.
  pub const GET_LATEST_FOR_QM: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions \
      WHERE storage_id = ? AND fileid = ? \
      ORDER BY version_mtime DESC LIMIT 1";

  pub const GET_LATEST_FOR_PG: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions \
      WHERE storage_id = $1 AND fileid = $2 \
      ORDER BY version_mtime DESC LIMIT 1";

  // -- DELETE one row by id.
  pub const DELETE_QM: &str = "DELETE FROM oc_files_versions WHERE id = ?";
  pub const DELETE_PG: &str = "DELETE FROM oc_files_versions WHERE id = $1";

  // -- LIST distinct (user, fileid) pairs for the tiered sweeper. Used to
  //    drive per-file bucket classification.
  pub const LIST_GROUPS_QM: &str = "\
      SELECT DISTINCT \"user\", fileid FROM oc_files_versions \
      ORDER BY \"user\", fileid";

  pub const LIST_GROUPS_PG: &str = "\
      SELECT DISTINCT \"user\", fileid FROM oc_files_versions \
      ORDER BY \"user\", fileid";

  // -- LIST all version rows for a (storage_id, fileid). Used for purge_for_fileid.
  pub const LIST_FOR_FILEID_QM: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE storage_id = ? AND fileid = ?";

  pub const LIST_FOR_FILEID_PG: &str = "\
      SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
      FROM oc_files_versions WHERE storage_id = $1 AND fileid = $2";
  ```

- [ ] **Step 2: Build**

  ```bash
  cargo build -p crabcloud-versions
  ```

  Expected: clean.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-versions/src/sql.rs
  git commit -m "versions: multidialect SQL constants"
  ```

### Task A4: `Versions` service — TDD with sqlite e2e

**Files:**
- Modify: `crates/crabcloud-versions/src/service.rs`
- Create: `crates/crabcloud-versions/tests/versions_e2e.rs`

This is the meat of Batch A.

- [ ] **Step 1: Write the e2e test file (RED)**

  Create `crates/crabcloud-versions/tests/versions_e2e.rs`:

  ```rust
  //! sqlite e2e for the Versions service.

  use crabcloud_config::test_support::minimal_sqlite_config;
  use crabcloud_db::{core_set, DbPool, MigrationRunner};
  use crabcloud_versions::{Versions, VersionsError};
  use std::path::PathBuf;
  use std::sync::Arc;
  use tempfile::TempDir;

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

  /// Write a file under <datadir>/<uid>/files/<rel>.
  async fn write_user_file(datadir: &PathBuf, uid: &str, rel: &str, contents: &[u8]) {
      let p = datadir.join(uid).join("files").join(rel.trim_start_matches('/'));
      tokio::fs::create_dir_all(p.parent().unwrap()).await.unwrap();
      tokio::fs::write(&p, contents).await.unwrap();
  }

  #[tokio::test]
  async fn snapshot_writes_row_and_copies_bytes() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/report.docx", b"v1").await;

      let id = versions
          .snapshot_if_needed("alice", /*storage_id*/ 1, /*fileid*/ 100, "/report.docx",
                              /*current_size*/ 2, /*now_secs*/ 1_716_000_000,
                              /*throttle_secs*/ 2, /*max_bytes*/ 1024)
          .await
          .unwrap()
          .expect("snapshot id");
      assert!(id > 0);

      // On-disk version file exists.
      let v_path = datadir.join("alice/files_versions/report.docx.v1716000000");
      assert!(v_path.exists());
      assert_eq!(tokio::fs::read(&v_path).await.unwrap(), b"v1");

      // Original is untouched (snapshot is a copy, not a move).
      let original = datadir.join("alice/files/report.docx");
      assert_eq!(tokio::fs::read(&original).await.unwrap(), b"v1");

      // List returns the entry.
      let rows = versions.list_for("alice", 100).await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].size, 2);
      assert_eq!(rows[0].version_mtime, 1_716_000_000);
      assert_eq!(rows[0].path, "/report.docx");
  }

  #[tokio::test]
  async fn snapshot_skips_when_throttled() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/a.txt", b"v1").await;

      versions
          .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_000, 2, 1024)
          .await.unwrap().expect("first snapshot");
      // Second snapshot at now=1001 (within throttle window of 2s) → None.
      let r = versions
          .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_001, 2, 1024)
          .await.unwrap();
      assert!(r.is_none());

      // Past throttle (now=1003) → snapshot.
      let r = versions
          .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_003, 2, 1024)
          .await.unwrap();
      assert!(r.is_some());
  }

  #[tokio::test]
  async fn snapshot_skips_when_oversize() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/big.bin", b"hello").await;

      let r = versions
          .snapshot_if_needed("alice", 1, 100, "/big.bin", /*current_size*/ 999_999_999, 1_000, 2, /*max_bytes*/ 1024)
          .await.unwrap();
      assert!(r.is_none());
      assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 0);
  }

  #[tokio::test]
  async fn snapshot_skips_on_zero_byte() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/empty.txt", b"").await;

      let r = versions
          .snapshot_if_needed("alice", 1, 100, "/empty.txt", 0, 1_000, 2, 1024)
          .await.unwrap();
      assert!(r.is_none());
  }

  #[tokio::test]
  async fn restore_snapshots_current_then_replaces() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/report.docx", b"v1").await;
      let id = versions
          .snapshot_if_needed("alice", 1, 100, "/report.docx", 2, 1_000, 2, 1024)
          .await.unwrap().expect("snapshot");

      // Current file changes to v2.
      write_user_file(&datadir, "alice", "/report.docx", b"v2-newer").await;

      // Restore v1. now is 2_000 — well outside throttle, so the auto-snapshot fires.
      versions.restore("alice", id, /*current_size_for_snapshot*/ 8, /*now_secs*/ 2_000,
                       /*throttle_secs*/ 2, /*max_bytes*/ 1024).await.unwrap();

      // Current is now v1 again.
      let current = datadir.join("alice/files/report.docx");
      assert_eq!(tokio::fs::read(&current).await.unwrap(), b"v1");

      // Two versions exist: the original v1 + a snapshot of v2 (taken before
      // the restore overwrote current).
      let rows = versions.list_for("alice", 100).await.unwrap();
      assert_eq!(rows.len(), 2);
      // Newest-first: the new snapshot is first.
      assert_eq!(rows[0].size, 8);
      assert_eq!(rows[1].id, id);
  }

  #[tokio::test]
  async fn delete_removes_row_and_file() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
      let id = versions
          .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
          .await.unwrap().expect("snapshot");

      versions.delete("alice", id).await.unwrap();
      assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
      assert!(!datadir.join("alice/files_versions/x.txt.v1000").exists());
  }

  #[tokio::test]
  async fn purge_for_fileid_removes_all_versions() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
      versions.snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024).await.unwrap();
      write_user_file(&datadir, "alice", "/x.txt", b"v2").await;
      versions.snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_003, 2, 1024).await.unwrap();

      let n = versions.purge_for_fileid(1, 100).await.unwrap();
      assert_eq!(n, 2);
      assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
  }

  #[tokio::test]
  async fn sweep_tiered_keeps_newest_per_bucket() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());

      // Seed 6 versions of the same file spanning several buckets.
      // now = 1_000_000_000 (~Sep 2001)
      // Bucket boundaries: 24h, 30d, 180d.
      let now: i64 = 1_000_000_000;
      let day = 86_400;
      let hour = 3_600;
      // Seed by writing the file and snapshotting each time. Use the throttle-
      // bypass shape — passes the per-call now and a 0 throttle to allow rapid
      // repeated snapshots (or set a wide throttle and stride past it).
      for offset in [-1, -hour - 1, -2 * day, -10 * day, -45 * day, -200 * day] {
          write_user_file(&datadir, "alice", "/y.txt", &format!("v{offset}").into_bytes()).await;
          versions
              .snapshot_if_needed("alice", 1, 200, "/y.txt", 4, now + offset, /*throttle*/ 0, 1024)
              .await.unwrap();
      }
      let pre_count = versions.list_for("alice", 200).await.unwrap().len();
      assert_eq!(pre_count, 6);

      let purged = versions.sweep_tiered(now).await.unwrap();
      let post = versions.list_for("alice", 200).await.unwrap();
      // Buckets after sweep (newest-per-bucket):
      //  - 0-24h: keep -1 and -hour-1 (both within 24h, each keeps newest-in-bucket;
      //    spec keeps EVERY version in this bucket, not one — see Versions::sweep_tiered).
      //  - 24h-30d: -2d (within hour-bucket for that hour, single rep)
      //  - 24h-30d: -10d
      //  - 30d-180d: -45d
      //  - 180d+: -200d
      // Net: 6 kept (all of them within a single "newest in slot" per spec). Sanity:
      // when the bucket slot for each version differs, all 6 survive.
      assert!(post.len() >= 4 && post.len() <= 6, "got {} versions after sweep", post.len());
      let _ = purged;
  }

  #[tokio::test]
  async fn wrong_user_on_delete_returns_error() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Versions::new(pool.clone(), datadir.clone());
      write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
      let id = versions
          .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
          .await.unwrap().expect("snapshot");

      let r = versions.delete("bob", id).await;
      assert!(matches!(r, Err(VersionsError::WrongUser)));
  }
  ```

- [ ] **Step 2: Run the test (RED — service is a stub)**

  ```bash
  cargo test -p crabcloud-versions --test versions_e2e
  ```

  Expected: compile failures on every method (`snapshot_if_needed`, `list_for`, `restore`, `delete`, `sweep_tiered`, `purge_for_fileid`).

- [ ] **Step 3: Implement `src/service.rs`**

  Replace the stub from A2 with the full service. Follow the `crabcloud-trash/src/service.rs` pattern for per-dialect row decode.

  ```rust
  //! `Versions` — file version snapshot + list + restore + delete + sweep.
  //!
  //! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
  //! On-disk layout: `<datadir>/<uid>/files_versions/<relative>.v<mtime>`.

  use crate::error::VersionsError;
  use crate::sql;
  use crate::types::VersionEntry;
  use crabcloud_db::DbPool;
  use sqlx::Row as _;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Versions {
      pool: Arc<DbPool>,
      datadir: PathBuf,
  }

  impl Versions {
      pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
          Self { pool, datadir }
      }

      pub fn datadir(&self) -> &Path {
          &self.datadir
      }

      // -------- snapshot_if_needed --------

      /// Snapshot the current bytes at `<datadir>/<uid>/files/<src_path>` to
      /// the versions tree, recording an `oc_files_versions` row. Returns
      /// `Ok(Some(id))` on a successful snapshot, `Ok(None)` if the snapshot
      /// was skipped (zero-byte, oversize, throttled), or `Err` on real
      /// failure.
      ///
      /// The caller passes the pre-write current size (cheap to compute from
      /// the filecache row) plus `now_secs`, `throttle_secs`, and `max_bytes`
      /// drawn from config. Decoupling these from `Versions` itself keeps
      /// the service free of clock + config dependencies and makes tests
      /// deterministic.
      pub async fn snapshot_if_needed(
          &self,
          uid: &str,
          storage_id: i64,
          fileid: i64,
          src_path: &str,
          current_size: i64,
          now_secs: i64,
          throttle_secs: i64,
          max_bytes: u64,
      ) -> Result<Option<i64>, VersionsError> {
          if current_size == 0 {
              return Ok(None);
          }
          if (current_size as u64) > max_bytes {
              tracing::warn!(
                  uid, fileid, current_size, max_bytes,
                  "versions: skipping snapshot, size exceeds max_bytes"
              );
              return Ok(None);
          }
          if throttle_secs > 0 {
              if let Some(latest) = self.get_latest_for(storage_id, fileid).await? {
                  if now_secs - latest.version_mtime < throttle_secs {
                      return Ok(None);
                  }
              }
          }
          // OK to snapshot.
          let rel = src_path.trim_start_matches('/');
          let src_abs = self.datadir.join(uid).join("files").join(rel);
          if !tokio::fs::try_exists(&src_abs).await? {
              return Err(VersionsError::SourceMissing);
          }
          let dst_dir = self.datadir.join(uid).join("files_versions").join(
              Path::new(rel).parent().unwrap_or_else(|| Path::new("")),
          );
          tokio::fs::create_dir_all(&dst_dir).await?;
          let basename = Path::new(rel).file_name().and_then(|s| s.to_str())
              .ok_or(VersionsError::SourceMissing)?;
          let dst_abs = dst_dir.join(format!("{basename}.v{now_secs}"));
          if let Err(e) = tokio::fs::copy(&src_abs, &dst_abs).await {
              let _ = tokio::fs::remove_file(&dst_abs).await;
              return Err(e.into());
          }

          let path_for_row = format!("/{rel}");
          let id = match self.insert_row(storage_id, fileid, uid, &path_for_row, now_secs, current_size).await {
              Ok(id) => id,
              Err(e) => {
                  tracing::warn!(
                      error = %e, orphan_path = %dst_abs.display(),
                      "versions: INSERT failed after copy; bytes stranded"
                  );
                  return Err(e);
              }
          };
          Ok(Some(id))
      }

      async fn insert_row(
          &self,
          storage_id: i64,
          fileid: i64,
          uid: &str,
          path: &str,
          version_mtime: i64,
          size: i64,
      ) -> Result<i64, VersionsError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(storage_id).bind(fileid).bind(uid).bind(path)
                      .bind(version_mtime).bind(size)
                      .execute(p).await?;
                  Ok(r.last_insert_rowid())
              }
              DbPool::MySql(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(storage_id).bind(fileid).bind(uid).bind(path)
                      .bind(version_mtime).bind(size)
                      .execute(p).await?;
                  Ok(r.last_insert_id() as i64)
              }
              DbPool::Postgres(p) => {
                  let row = sqlx::query(sql::INSERT_PG)
                      .bind(storage_id).bind(fileid).bind(uid).bind(path)
                      .bind(version_mtime).bind(size)
                      .fetch_one(p).await?;
                  Ok(row.try_get::<i64, _>("id")?)
              }
          }
      }

      // -------- list_for / get_by_id / get_latest_for --------

      pub async fn list_for(&self, uid: &str, fileid: i64) -> Result<Vec<VersionEntry>, VersionsError> {
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::LIST_FOR_QM).bind(uid).bind(fileid).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::LIST_FOR_QM).bind(uid).bind(fileid).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::LIST_FOR_PG).bind(uid).bind(fileid).fetch_all(p).await?,
          };
          rows.iter().map(row_to_entry).collect()
      }

      pub async fn get_by_id(&self, id: i64) -> Result<VersionEntry, VersionsError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::GET_BY_ID_QM).bind(id).fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::GET_BY_ID_QM).bind(id).fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::GET_BY_ID_PG).bind(id).fetch_optional(p).await?,
          };
          row.as_ref().map(row_to_entry).transpose()?.ok_or(VersionsError::NotFound)
      }

      async fn get_latest_for(
          &self,
          storage_id: i64,
          fileid: i64,
      ) -> Result<Option<VersionEntry>, VersionsError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::GET_LATEST_FOR_QM)
                  .bind(storage_id).bind(fileid).fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::GET_LATEST_FOR_QM)
                  .bind(storage_id).bind(fileid).fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::GET_LATEST_FOR_PG)
                  .bind(storage_id).bind(fileid).fetch_optional(p).await?,
          };
          row.as_ref().map(row_to_entry).transpose()
      }

      // -------- restore --------

      /// Snapshot the current file at `<uid>/files/<entry.path>` (so the
      /// pre-restore state is not lost), then copy the version's bytes over
      /// current. Caller passes `current_size_for_snapshot`, `now_secs`, and
      /// the throttle / size_cap config — same shape as snapshot_if_needed.
      pub async fn restore(
          &self,
          uid: &str,
          version_id: i64,
          current_size_for_snapshot: i64,
          now_secs: i64,
          throttle_secs: i64,
          max_bytes: u64,
      ) -> Result<(), VersionsError> {
          let entry = self.get_by_id(version_id).await?;
          if entry.user != uid {
              return Err(VersionsError::WrongUser);
          }
          // Snapshot current first (best-effort: a None snapshot here means
          // current was zero or throttled; restore still proceeds).
          let _ = self.snapshot_if_needed(
              uid, entry.storage_id, entry.fileid, &entry.path,
              current_size_for_snapshot, now_secs, throttle_secs, max_bytes,
          ).await?;

          let rel = entry.path.trim_start_matches('/');
          let src_abs = self.datadir.join(uid).join("files_versions").join(
              Path::new(rel).parent().unwrap_or_else(|| Path::new("")),
          ).join(format!(
              "{}.v{}",
              Path::new(rel).file_name().and_then(|s| s.to_str()).ok_or(VersionsError::SourceMissing)?,
              entry.version_mtime
          ));
          if !tokio::fs::try_exists(&src_abs).await? {
              return Err(VersionsError::SourceMissing);
          }
          let dst_abs = self.datadir.join(uid).join("files").join(rel);
          tokio::fs::copy(&src_abs, &dst_abs).await?;
          Ok(())
      }

      // -------- delete --------

      pub async fn delete(&self, uid: &str, id: i64) -> Result<(), VersionsError> {
          let entry = self.get_by_id(id).await?;
          if entry.user != uid {
              return Err(VersionsError::WrongUser);
          }
          self.delete_entry(&entry).await
      }

      async fn delete_entry(&self, entry: &VersionEntry) -> Result<(), VersionsError> {
          let rel = entry.path.trim_start_matches('/');
          let basename = Path::new(rel).file_name().and_then(|s| s.to_str())
              .ok_or(VersionsError::SourceMissing)?;
          let on_disk = self.datadir.join(&entry.user).join("files_versions").join(
              Path::new(rel).parent().unwrap_or_else(|| Path::new("")),
          ).join(format!("{basename}.v{}", entry.version_mtime));
          if tokio::fs::try_exists(&on_disk).await? {
              tokio::fs::remove_file(&on_disk).await?;
          }
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => { sqlx::query(sql::DELETE_QM).bind(entry.id).execute(p).await?; }
              DbPool::MySql(p) => { sqlx::query(sql::DELETE_QM).bind(entry.id).execute(p).await?; }
              DbPool::Postgres(p) => { sqlx::query(sql::DELETE_PG).bind(entry.id).execute(p).await?; }
          }
          Ok(())
      }

      // -------- purge_for_fileid --------

      /// Remove every version row + on-disk file for `(storage_id, fileid)`.
      /// Invoked by `Trash::purge_entry` on hard-delete cascade. Best-effort
      /// on individual file removals; row deletion follows the file delete.
      pub async fn purge_for_fileid(&self, storage_id: i64, fileid: i64) -> Result<u64, VersionsError> {
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::LIST_FOR_FILEID_QM)
                  .bind(storage_id).bind(fileid).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::LIST_FOR_FILEID_QM)
                  .bind(storage_id).bind(fileid).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::LIST_FOR_FILEID_PG)
                  .bind(storage_id).bind(fileid).fetch_all(p).await?,
          };
          let mut n = 0u64;
          for r in rows {
              let entry = row_to_entry(&r)?;
              if let Err(e) = self.delete_entry(&entry).await {
                  tracing::warn!(
                      error = %e, version_id = entry.id,
                      "versions purge_for_fileid: delete_entry failed"
                  );
                  continue;
              }
              n += 1;
          }
          Ok(n)
      }

      // -------- sweep_tiered --------

      /// Apply the tiered retention rule per `(user, fileid)` group. Returns
      /// the number of rows purged. Bucket schedule (relative to `now_secs`):
      ///   0-24h: keep every version
      ///   24h-30d: keep one per hour bucket (newest in each)
      ///   30d-180d: keep one per day bucket
      ///   180d+: keep one per week bucket
      pub async fn sweep_tiered(&self, now_secs: i64) -> Result<u64, VersionsError> {
          let groups: Vec<(String, i64)> = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::LIST_GROUPS_QM).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::LIST_GROUPS_QM).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::LIST_GROUPS_PG).fetch_all(p).await?,
          }
          .iter()
          .map(|r| -> Result<_, VersionsError> {
              Ok((r.try_get::<String, _>("user")?, r.try_get::<i64, _>("fileid")?))
          })
          .collect::<Result<_, _>>()?;

          let mut purged_total = 0u64;
          for (uid, fileid) in groups {
              let entries = self.list_for(&uid, fileid).await?; // newest-first
              let mut seen_slots: std::collections::HashSet<(u8, i64)> = std::collections::HashSet::new();
              for entry in entries {
                  let age_secs = now_secs - entry.version_mtime;
                  let slot = bucket_slot(age_secs, entry.version_mtime);
                  // slot.0 = 0 means "keep every" — don't dedupe.
                  let keep = if slot.0 == 0 {
                      true
                  } else {
                      seen_slots.insert(slot)
                  };
                  if !keep {
                      if let Err(e) = self.delete_entry(&entry).await {
                          tracing::warn!(error = %e, id = entry.id, "versions sweep: delete_entry failed");
                          continue;
                      }
                      purged_total += 1;
                  }
              }
          }
          Ok(purged_total)
      }
  }

  /// Bucket classifier. Returns `(bucket_tag, slot_key)`. The sweeper keeps
  /// the newest version per `(bucket_tag, slot_key)` pair, except for
  /// `bucket_tag == 0` (the 0-24h "keep every" tier) where the slot is
  /// ignored.
  ///
  /// Tag values:
  ///   0 — within 24h (keep every)
  ///   1 — 24h-30d (one per hour bucket)
  ///   2 — 30d-180d (one per day bucket)
  ///   3 — 180d+ (one per week bucket)
  fn bucket_slot(age_secs: i64, version_mtime: i64) -> (u8, i64) {
      const HOUR: i64 = 3_600;
      const DAY: i64 = 86_400;
      const WEEK: i64 = 7 * DAY;
      if age_secs < DAY {
          (0, 0) // ignored
      } else if age_secs < 30 * DAY {
          (1, version_mtime / HOUR)
      } else if age_secs < 180 * DAY {
          (2, version_mtime / DAY)
      } else {
          (3, version_mtime / WEEK)
      }
  }

  /// Decode a row of either dialect into a `VersionEntry`.
  fn row_to_entry(r: &sqlx::any::AnyRow) -> Result<VersionEntry, VersionsError> {
      // sqlx::any rows aren't actually used — the per-dialect Row impls each
      // satisfy `Row` so `try_get` works uniformly. The crabcloud-trash crate
      // uses per-dialect inline decode with a shared `RowParts` struct. Mirror
      // that exact pattern here. Look at crabcloud-trash/src/service.rs for
      // the canonical shape, then adapt to the VersionEntry fields.
      let _ = r;
      unreachable!("row_to_entry: replace with the per-dialect inline pattern from crabcloud-trash")
  }
  ```

  **Important:** the `row_to_entry` placeholder must be replaced with the per-dialect inline pattern from `crates/crabcloud-trash/src/service.rs`. Read that file first to see the exact macro / closure / inline shape (look for `row_from_sqlite` / `row_from_mysql` / `row_from_postgres` or whatever it's called). Mirror it precisely.

- [ ] **Step 4: Iterate against the e2e until GREEN**

  ```bash
  cargo test -p crabcloud-versions --test versions_e2e
  ```

  All 9 tests must pass. The sweep test is the trickiest — read the bucket comments + the assertion bounds carefully. If it fails, the bucket math is wrong or the test seed widths are off; debug by printing pre/post version mtimes and the computed `(tag, slot)` for each.

- [ ] **Step 5: Add unit test for `bucket_slot`**

  Bottom of `service.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn bucket_slot_classification() {
          // 0-24h → tag 0
          assert_eq!(bucket_slot(60, 1_000), (0, 0));
          assert_eq!(bucket_slot(86_399, 1_000), (0, 0));
          // 24h-30d → tag 1, hour bucket
          assert_eq!(bucket_slot(86_400, 1_000), (1, 1_000 / 3_600));
          assert_eq!(bucket_slot(30 * 86_400 - 1, 1_000), (1, 1_000 / 3_600));
          // 30d-180d → tag 2, day bucket
          assert_eq!(bucket_slot(30 * 86_400, 1_000_000), (2, 1_000_000 / 86_400));
          // 180d+ → tag 3, week bucket
          assert_eq!(bucket_slot(180 * 86_400, 1_000_000), (3, 1_000_000 / (7 * 86_400)));
      }
  }
  ```

  Run `cargo test -p crabcloud-versions` — both unit + e2e pass.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-versions/src/service.rs crates/crabcloud-versions/tests/versions_e2e.rs
  git commit -m "versions: Versions service (snapshot / list / restore / delete / sweep / purge)"
  ```

### Task A5: `VersionsSweeper` background task

**Files:**
- Create: `crates/crabcloud-core/src/versions_sweeper.rs`
- Modify: `crates/crabcloud-core/src/lib.rs`
- Modify: `crates/crabcloud-core/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-versions` dep to `crabcloud-core`**

  In `crates/crabcloud-core/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-versions = { workspace = true }
  ```

- [ ] **Step 2: Write `src/versions_sweeper.rs`**

  Mirror `trash_sweeper.rs` exactly.

  ```rust
  //! Background task: daily tiered-retention sweep of `oc_files_versions`.
  //! Mirrors the `TrashSweeper` / `MailQueueCleanup` shape: cooperative
  //! shutdown via `Arc<Notify>`, `sweep_once()` for sync test drive.

  use crabcloud_versions::Versions;
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::sync::Notify;

  /// 24-hour sleep between sweeps.
  const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

  #[derive(Clone)]
  pub struct VersionsSweeper {
      versions: Arc<Versions>,
      retention_disabled: bool,
      shutdown: Arc<Notify>,
  }

  impl VersionsSweeper {
      pub fn new(versions: Arc<Versions>, retention_disabled: bool) -> (Self, Arc<Notify>) {
          let shutdown = Arc::new(Notify::new());
          (
              Self { versions, retention_disabled, shutdown: shutdown.clone() },
              shutdown,
          )
      }

      pub async fn run(self) {
          loop {
              if let Err(e) = self.sweep_once().await {
                  tracing::warn!(error = %e, "versions sweeper: sweep_once failed");
              }
              tokio::select! {
                  _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                  _ = self.shutdown.notified() => return,
              }
          }
      }

      pub async fn sweep_once(&self) -> Result<u64, crabcloud_versions::VersionsError> {
          if self.retention_disabled {
              return Ok(0);
          }
          let now = chrono::Utc::now().timestamp();
          self.versions.sweep_tiered(now).await
      }
  }
  ```

- [ ] **Step 3: Wire into `lib.rs`**

  ```rust
  mod versions_sweeper;
  pub use versions_sweeper::VersionsSweeper;
  ```

  (Alphabetical position next to `trash_sweeper`.)

- [ ] **Step 4: Add inline unit tests** in `versions_sweeper.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crabcloud_config::test_support::minimal_sqlite_config;
      use crabcloud_db::{core_set, DbPool, MigrationRunner};
      use tempfile::TempDir;

      #[tokio::test]
      async fn sweep_once_disabled_returns_zero() {
          let db_dir = TempDir::new().unwrap();
          let data_dir = TempDir::new().unwrap();
          let cfg = minimal_sqlite_config(db_dir.path().join("t.db"));
          let pool = DbPool::connect(&cfg).await.unwrap();
          let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
          runner.register(core_set());
          runner.run().await.unwrap();
          let versions = Arc::new(Versions::new(Arc::new(pool), data_dir.path().to_path_buf()));
          let (sw, _) = VersionsSweeper::new(versions, /*retention_disabled*/ true);
          assert_eq!(sw.sweep_once().await.unwrap(), 0);
      }
  }
  ```

  Add `crabcloud-config` and `crabcloud-db` to `[dev-dependencies]` of `crabcloud-core` if not already present (they almost certainly are — check existing `trash_sweeper` tests).

- [ ] **Step 5: Build + test**

  ```bash
  cargo build -p crabcloud-core
  cargo test -p crabcloud-core versions_sweeper
  ```

  Expected: clean + passing.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-core/Cargo.toml crates/crabcloud-core/src/versions_sweeper.rs crates/crabcloud-core/src/lib.rs
  git commit -m "versions: VersionsSweeper background task (daily tiered retention)"
  ```

### Task A6: Config knobs

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`
- Modify: `crates/crabcloud-config/src/test_support.rs`

- [ ] **Step 1: Add three fields to `FileConfig`**

  Add after `trash_retention_days`:
  ```rust
  /// Minimum seconds between two versions of the same file. Writes within
  /// this window after the most-recent version do not create a new version
  /// (the actual write still happens). 0 disables throttling.
  #[serde(default = "default_versions_min_interval_secs")]
  pub versions_min_interval_secs: u32,

  /// Max size of a file (in bytes) that gets versioned. Larger writes
  /// still succeed but skip the snapshot. Default 1 GiB.
  #[serde(default = "default_versions_max_bytes")]
  pub versions_max_bytes: u64,

  /// When true the daily versions sweeper short-circuits — versions
  /// accumulate forever (compliance retain-forever escape hatch).
  /// Default false.
  #[serde(default)]
  pub versions_retention_disabled: bool,
  ```

  And the defaults at the bottom of the file:
  ```rust
  fn default_versions_min_interval_secs() -> u32 { 2 }
  fn default_versions_max_bytes() -> u64 { 1024 * 1024 * 1024 }  // 1 GiB
  ```

  Update `FileConfig::default()` (or whatever exists) to set all three.

- [ ] **Step 2: Update `test_support.rs::minimal_sqlite_config`**

  Add:
  ```rust
  versions_min_interval_secs: 2,
  versions_max_bytes: 1024 * 1024 * 1024,
  versions_retention_disabled: false,
  ```

- [ ] **Step 3: Build + test**

  ```bash
  cargo test -p crabcloud-config
  ```

  Expected: passes; defaults parse.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/crabcloud-config/src/types.rs crates/crabcloud-config/src/test_support.rs
  git commit -m "versions: versions_* config knobs (min_interval, max_bytes, retention_disabled)"
  ```

### Task A7: Wire `Versions` + `VersionsSweeper` into `AppState`

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Add import + two fields**

  Top: `use crabcloud_versions::Versions;`

  Inside `AppState`:
  ```rust
  /// File versions service. Cheap to clone.
  pub versions: Arc<crabcloud_versions::Versions>,
  /// Versions sweeper shutdown handle. Always present; spawned
  /// unconditionally in `AppStateBuilder::build`.
  pub versions_sweeper_shutdown: Arc<tokio::sync::Notify>,
  ```

- [ ] **Step 2: Construct + spawn in `AppStateBuilder::build`**

  After the trash construction block:
  ```rust
  let versions = Arc::new(crabcloud_versions::Versions::new(
      Arc::new(pool.clone()),
      self.config.datadirectory.clone(),
  ));
  let (versions_sweeper, versions_sweeper_shutdown) =
      crate::versions_sweeper::VersionsSweeper::new(
          versions.clone(),
          self.config.versions_retention_disabled,
      );
  std::mem::drop(tokio::spawn(async move { versions_sweeper.run().await }));
  ```

- [ ] **Step 3: Add to the AppState literal**

  ```rust
  versions,
  versions_sweeper_shutdown,
  ```

- [ ] **Step 4: Build + workspace tests**

  ```bash
  cargo build --workspace
  cargo test -p crabcloud-core state
  ```

  Expected: all pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-core/src/state.rs
  git commit -m "versions: wire Versions + VersionsSweeper into AppState"
  ```

### Task A8: Trash cascade on hard-delete

**Files:**
- Modify: `crates/crabcloud-trash/Cargo.toml`
- Modify: `crates/crabcloud-trash/src/service.rs`
- Modify: `crates/crabcloud-core/src/state.rs` (pass versions into trash)
- Modify: `crates/crabcloud-trash/tests/trash_e2e.rs` (new test for cascade)

- [ ] **Step 1: Add `crabcloud-versions` dep**

  In `crates/crabcloud-trash/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-versions = { workspace = true }
  ```

- [ ] **Step 2: Add `versions` field to `Trash`**

  In `crates/crabcloud-trash/src/service.rs`, extend the struct:
  ```rust
  pub struct Trash {
      pool: Arc<DbPool>,
      datadir: PathBuf,
      versions: Arc<crabcloud_versions::Versions>,
  }
  ```

  Update `Trash::new`:
  ```rust
  pub fn new(
      pool: Arc<DbPool>,
      datadir: PathBuf,
      versions: Arc<crabcloud_versions::Versions>,
  ) -> Self {
      Self { pool, datadir, versions }
  }
  ```

- [ ] **Step 3: Cascade in `purge_entry`**

  Find `Trash::purge_entry`. Before/after the row delete (the order matters — cascade BEFORE the trash row goes away so we can read `fileid_legacy`), add:
  ```rust
  // Cascade: purge any versions for this fileid. fileid_legacy is the
  // pre-trash filecache fileid; if it's None there's no cascade target.
  if let Some(fileid) = entry.fileid_legacy {
      if let Err(e) = self.versions.purge_for_fileid(/*storage_id*/ ??, fileid).await {
          tracing::warn!(
              error = %e, trash_id = entry.id, fileid,
              "trash purge: versions cascade failed; row purge continues"
          );
      }
  }
  ```

  **Problem:** `purge_for_fileid` needs `storage_id`, but `oc_files_trash` doesn't store it. Two options:
  - (a) **Look up the storage_id via the user**: every trash row has `user`; `oc_storages` has a numeric_id for that user's home. The lookup is one `SELECT numeric_id FROM oc_storages WHERE id LIKE 'local::...<uid>%'` (or however the existing code maps uid → home storage_id — look at `crabcloud-fs::HomeMountResolver` for the pattern).
  - (b) **Add `storage_id` to `oc_files_trash`**: cleaner long-term but requires a migration. Reject for this batch (out of scope).
  - **(c) Pick (a) for now** — copy whatever pattern the trash service already uses to resolve the user's home storage. If no such pattern exists, add a small helper in `Trash` that does the lookup.

  Either approach: get the storage_id and pass it through.

- [ ] **Step 4: Update `AppState` to pass versions into Trash**

  In `crates/crabcloud-core/src/state.rs`, find the `Trash::new(...)` call and add the versions arg:
  ```rust
  let trash = Arc::new(crabcloud_trash::Trash::new(
      Arc::new(pool.clone()),
      self.config.datadirectory.clone(),
      versions.clone(),  // NEW
  ));
  ```

  This means `versions` needs to be constructed BEFORE `trash`. Check the existing order; reshuffle if needed.

- [ ] **Step 5: Update every test fixture that builds a `Trash` directly**

  Grep `Trash::new(` and update each. For tests that don't care about versions, instantiate a no-op `Versions` (the real `Versions::new` works fine on a fresh sqlite + tempdir — share the test setup helper).

- [ ] **Step 6: Add cascade e2e test in `trash_e2e.rs`**

  ```rust
  #[tokio::test]
  async fn purge_cascades_to_versions() {
      let (pool, datadir, _d, _dd) = setup().await;
      let versions = Arc::new(crabcloud_versions::Versions::new(pool.clone(), datadir.clone()));
      let trash = Trash::new(pool.clone(), datadir.clone(), versions.clone());

      // Seed: write a file under alice, snapshot a version, then soft-delete + purge.
      write_user_file(&datadir, "alice", "/report.docx", b"v1").await;
      let storage_id = 1; // sqlite test pool: home storage gets id 1 (verify via your seed)
      versions.snapshot_if_needed("alice", storage_id, /*fileid*/ 100, "/report.docx",
                                  2, 1_000, 2, 1024).await.unwrap();
      assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 1);

      let id = trash.soft_delete("alice", "/report.docx", TrashType::File, Some(100)).await.unwrap();
      trash.purge("alice", id).await.unwrap();

      // Cascade: version row is gone.
      assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
  }
  ```

- [ ] **Step 7: Run the full workspace tests**

  ```bash
  cargo test --workspace
  ```

  Fix any regressions caused by the `Trash::new` signature change. Likely candidates: AppState tests, e2e tests that build a Trash directly.

- [ ] **Step 8: Commit**

  ```bash
  git add crates/crabcloud-trash/ crates/crabcloud-core/src/state.rs
  git commit -m "trash: cascade-purge to versions on hard-delete"
  ```

### Task A9: `View::write_file` + `View::move_with_overwrite` snapshot hooks

**Files:**
- Modify: `crates/crabcloud-fs/Cargo.toml`
- Modify: `crates/crabcloud-fs/src/view.rs`

- [ ] **Step 1: Add `crabcloud-versions` dep**

  ```toml
  crabcloud-versions = { workspace = true }
  ```

- [ ] **Step 2: Add `versions` field to `View`**

  Extend `View`:
  ```rust
  pub struct View {
      // ...
      versions: Arc<crabcloud_versions::Versions>,
  }
  ```

  Extend `View::new` to take a 6th argument `versions: Arc<crabcloud_versions::Versions>`. Update every call site (grep `View::new(`).

  If `View::new` is getting unwieldy with 6 positional args, mirror SP12 polish C and introduce a `ViewConfig` struct. Decision: defer ViewConfig unless the call-site update is unmanageable.

- [ ] **Step 3: Hook the snapshot before write**

  Find `View::write_file` (PUT). Before the storage backend's write, add:
  ```rust
  // Snapshot the pre-write bytes if this is a real overwrite of a non-empty file.
  if let Some(current_row) = self.filecache.lookup(&storage_id, &storage_path).await.map_err(map_filecache)? {
      if current_row.size > 0 {
          let now = chrono::Utc::now().timestamp();
          let owner_uid = mount_owner_uid;  // resolve via mount metadata, like trash does
          if let Err(e) = self.versions.snapshot_if_needed(
              &owner_uid,
              current_row.storage_id,
              current_row.fileid,
              &owner_relative_path,
              current_row.size,
              now,
              /*throttle_secs*/ self.versions_min_interval_secs as i64,
              /*max_bytes*/ self.versions_max_bytes,
          ).await {
              // Versioning failure is a hard error per spec §6 ("Disk full during snapshot").
              return Err(FsError::Storage(format!("versions: {e}")));
          }
      }
  }
  // ... existing write code follows ...
  ```

  **Where does `View` get `versions_min_interval_secs` / `versions_max_bytes`?** Two options:
  - (a) Add them as fields on `View` (`View::new` takes them as additional args). Plumb from `AppState` config.
  - (b) Pass them to `Versions::snapshot_if_needed` from `AppState` (View has access to AppState? — it doesn't today; AppState has views, not vice versa).
  - **Pick (a)** but clean: add a `ViewConfig` struct holding `(versions, versions_min_interval_secs, versions_max_bytes, filecache, mounts, etc.)` and pass that to `View::new`. Otherwise the signature explodes. This is the moment to introduce ViewConfig — the SP12 polish C precedent applies.

- [ ] **Step 4: Same hook for `View::move_with_overwrite`**

  Find the MOVE-overwrite path. Add a symmetric snapshot of the destination's current bytes before the move. The source side is NOT versioned (it's being moved away; whatever was there stays the same — just at a new path).

- [ ] **Step 5: Read `crates/crabcloud-fs/src/view.rs` carefully for ALL write entry points**

  PUT, MOVE-overwrite, COPY-overwrite (if exists), any other rename / chunked-upload finalize path. Each one that mutates an existing file needs the same snapshot call. Don't trust your guesses — grep for `storage.write` and `storage.copy` and audit each call site.

- [ ] **Step 6: Tests in `crates/crabcloud-fs/tests/view_versions.rs` (new)**

  Mirror the `view_trash.rs` structure. Cover:
  - PUT overwriting a non-empty file creates a version row.
  - Two PUTs within the throttle window create only one version.
  - PUT of a zero-byte file does NOT create a version.
  - PUT of a file > max_bytes does NOT create a version (but the write succeeds).
  - MOVE with overwrite creates a version of the destination.
  - Shared file: Alice shares /report.docx with Bob, Bob PUTs an update → version row has `user='alice'`.
  - Read-only share: Bob's PUT is denied at the storage layer → no version row.

- [ ] **Step 7: Run the workspace tests**

  ```bash
  cargo test --workspace
  ```

  Fix any regressions. The `View::new` signature change will ripple — same drill as Batch A's Trash version.

- [ ] **Step 8: Commit**

  ```bash
  git add crates/crabcloud-fs/ crates/crabcloud-core/src/state.rs
  git commit -m "versions: hook View::write_file + View::move_with_overwrite"
  ```

### Task A10: Batch A pre-PR

- [ ] **Step 1: Pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push and open PR**

  ```bash
  git push -u origin sp13/a-versions-crate
  gh pr create --title "sp13(a): crabcloud-versions crate + View hooks + sweeper + trash cascade" \
    --body "Batch A of SP13 versioning. New crabcloud-versions crate, 0010_files_versions migration, View::write_file + View::move_with_overwrite snapshot hooks, VersionsSweeper background task with tiered retention, three new config knobs (versions_min_interval_secs, versions_max_bytes, versions_retention_disabled), AppState wiring, and Trash::purge_entry cascade to Versions::purge_for_fileid on hard-delete. Spec: docs/superpowers/specs/2026-05-17-file-versioning-design.md."
  ```

---

# Batch B — DAV `/dav/versions/{uid}/{fileid}/...` surface

**Branch:** `sp13/b-dav` (off the merged Batch A master)

**Goal:** Add the Nextcloud-shape DAV versions endpoint with PROPFIND / GET / COPY at both `/dav/versions` and `/remote.php/dav/versions` prefixes.

### Task B1: Versions router skeleton

**Files:**
- Create: `crates/crabcloud-http/src/routes/versions/mod.rs`
- Create: `crates/crabcloud-http/src/routes/versions/propfind.rs`
- Create: `crates/crabcloud-http/src/routes/versions/get.rs`
- Create: `crates/crabcloud-http/src/routes/versions/copy.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs` (add `pub mod versions;`)
- Modify: `crates/crabcloud-http/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-versions` dep**

  In `crates/crabcloud-http/Cargo.toml`:
  ```toml
  crabcloud-versions = { workspace = true }
  ```

- [ ] **Step 2: Write `routes/versions/mod.rs`**

  Mirror `routes/trashbin/mod.rs` exactly — same dispatch shape, same 405 fallthrough with `Allow:` header, same per-method file split.

  ```rust
  //! DAV `/dav/versions/{uid}/{fileid}/...` surface.
  //!
  //! Routes are nest-relative; mount via `Router::nest("/dav/versions", ...)`
  //! and `Router::nest("/remote.php/dav/versions", ...)` in `router.rs`.
  //!
  //! Inside this namespace:
  //!   PROPFIND /{uid}/{fileid}/                      — list versions of fileid
  //!   PROPFIND /{uid}/{fileid}/{version_mtime}       — single version detail
  //!   GET      /{uid}/{fileid}/{version_mtime}       — stream the version's bytes
  //!   COPY     /{uid}/{fileid}/{version_mtime}       — restore (Destination required)
  //!   *        anything else                          — 405 with Allow: header

  mod copy;
  mod get;
  mod propfind;

  use axum::routing::any;
  use axum::Router;
  use crabcloud_core::AppState;

  const ALLOW_HEADER: &str = "OPTIONS, PROPFIND, GET, COPY";

  pub fn router() -> Router<AppState> {
      Router::new()
          .route("/{uid}/{fileid}/", any(dispatch_root))
          .route("/{uid}/{fileid}/{version_mtime}", any(dispatch_entry))
  }

  async fn dispatch_root(
      axum::extract::State(state): axum::extract::State<AppState>,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      match req.method().as_str() {
          "PROPFIND" => propfind::list(state, req).await,
          "OPTIONS" => options_response(),
          _ => method_not_allowed(),
      }
  }

  async fn dispatch_entry(
      axum::extract::State(state): axum::extract::State<AppState>,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      match req.method().as_str() {
          "PROPFIND" => propfind::entry(state, req).await,
          "GET"      => get::download(state, req).await,
          "COPY"     => copy::restore(state, req).await,
          "OPTIONS"  => options_response(),
          _          => method_not_allowed(),
      }
  }

  fn method_not_allowed() -> axum::response::Response {
      use axum::response::IntoResponse;
      let mut resp = (axum::http::StatusCode::METHOD_NOT_ALLOWED, "").into_response();
      resp.headers_mut().insert("Allow", ALLOW_HEADER.parse().unwrap());
      resp
  }

  fn options_response() -> axum::response::Response {
      use axum::response::IntoResponse;
      let mut resp = (axum::http::StatusCode::OK, "").into_response();
      resp.headers_mut().insert("DAV", "1, 2, 3".parse().unwrap());
      resp.headers_mut().insert("Allow", ALLOW_HEADER.parse().unwrap());
      resp
  }

  /// Shared path-param extraction. Mirror the trashbin pattern.
  pub(super) fn path_param(req: &axum::http::Request<axum::body::Body>, name: &str) -> Option<String> {
      // Implementation: same approach as crates/crabcloud-http/src/routes/trashbin/mod.rs
      // (probably axum::extract::Path<HashMap<String, String>> via from_request_parts,
      // or req.extensions().get::<MatchedPath>() + manual). Copy verbatim.
      todo!("mirror routes/trashbin/mod.rs path_param")
  }

  pub(super) fn versions_err(e: crabcloud_versions::VersionsError) -> crate::routes::dav::DavError {
      // Map VersionsError variants to DavError per the same shape as trashbin/mod.rs's trash_err.
      use crabcloud_versions::VersionsError::*;
      use crate::routes::dav::DavError;
      match e {
          NotFound | SourceMissing => DavError::NotFound,
          WrongUser => DavError::Forbidden,
          Io(_) | Db(_) => DavError::Internal(e.to_string()),
      }
  }
  ```

- [ ] **Step 3: Stub the three handler modules**

  Same shape as the trashbin handler stubs from SP12 Batch B Task B1. Each is a `pub async fn` returning `axum::response::Response` with a `todo!("Task B2/B3/B4")` body.

- [ ] **Step 4: Add `pub mod versions;` in `routes/mod.rs`**

- [ ] **Step 5: Build**

  ```bash
  cargo build -p crabcloud-http
  ```

  Expected: clean (todo!() compiles).

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-http/
  git commit -m "versions dav: router skeleton + stubbed handlers"
  ```

### Task B2: `PROPFIND` — list + per-entry

**Files:**
- Modify: `crates/crabcloud-http/src/routes/versions/propfind.rs`
- Create: `crates/crabcloud-http/tests/dav_versions_propfind.rs`

- [ ] **Step 1: Implement `list` and `entry`**

  Mirror `trashbin/propfind.rs` closely. Differences:
  - Resource path is `/dav/versions/{uid}/{fileid}/{version_mtime}` (vs `/dav/trashbin/{uid}/trash/{basename}.{suffix}`).
  - Per-entry props: `displayname` (= original basename, derived from `entry.path`), `getlastmodified` (= `version_mtime` formatted as HTTP-date), `getcontentlength` (= `size`), `getcontenttype` (look up mime from the current file via `state.filecache.lookup(storage_id, current_path)` if available; otherwise `application/octet-stream`), `resourcetype` (empty).
  - List the versions via `state.versions.list_for(&uid, fileid)`.
  - Authed via `AuthenticatedUser`; uid must match authed user (no cross-user version listing for MVP — even with share grants, list goes through the OCS endpoint).

- [ ] **Step 2: Write the e2e test**

  Mirror `dav_trashbin_propfind.rs`. Two cases (root + entry), one alias case for `/remote.php/dav/...`.

- [ ] **Step 3: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test dav_versions_propfind
  git add crates/crabcloud-http/src/routes/versions/propfind.rs crates/crabcloud-http/tests/dav_versions_propfind.rs
  git commit -m "versions dav: PROPFIND root + per-entry"
  ```

### Task B3: `GET` — stream version bytes

**Files:**
- Modify: `crates/crabcloud-http/src/routes/versions/get.rs`
- Create: `crates/crabcloud-http/tests/dav_versions_get.rs`

- [ ] **Step 1: Implement `download`**

  ```rust
  pub async fn download(
      state: AppState,
      req: axum::http::Request<axum::body::Body>,
  ) -> axum::response::Response {
      use axum::response::IntoResponse;
      use http::StatusCode;

      let authed = match crate::middleware::auth::AuthenticatedUser::from_request(&req) {
          Ok(u) => u,
          Err(r) => return r,
      };
      let uid = super::path_param(&req, "uid");
      let fileid: Option<i64> = super::path_param(&req, "fileid").and_then(|s| s.parse().ok());
      let version_mtime: Option<i64> = super::path_param(&req, "version_mtime").and_then(|s| s.parse().ok());
      let (Some(uid), Some(fileid), Some(version_mtime)) = (uid, fileid, version_mtime) else {
          return (StatusCode::NOT_FOUND, "").into_response();
      };
      if uid != authed.uid.as_str() {
          return (StatusCode::FORBIDDEN, "").into_response();
      }
      // Look up the matching row.
      let entries = match state.versions.list_for(&uid, fileid).await {
          Ok(v) => v,
          Err(e) => return super::versions_err(e).into_response(),
      };
      let entry = match entries.into_iter().find(|e| e.version_mtime == version_mtime) {
          Some(e) => e,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };

      // On-disk path for the version file.
      let rel = entry.path.trim_start_matches('/');
      let basename = match std::path::Path::new(rel).file_name().and_then(|s| s.to_str()) {
          Some(b) => b,
          None => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
      };
      let parent = std::path::Path::new(rel).parent().unwrap_or_else(|| std::path::Path::new(""));
      let abs = state.versions.datadir().join(&uid).join("files_versions").join(parent)
          .join(format!("{basename}.v{version_mtime}"));

      // Open as a Body. tokio_util::io::ReaderStream is the idiom.
      let f = match tokio::fs::File::open(&abs).await {
          Ok(f) => f,
          Err(e) => {
              tracing::error!(error = %e, path = %abs.display(), "versions GET: file missing");
              return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
          }
      };
      let stream = tokio_util::io::ReaderStream::new(f);
      let body = axum::body::Body::from_stream(stream);
      let mut resp = (StatusCode::OK, body).into_response();
      resp.headers_mut().insert("Content-Length", entry.size.to_string().parse().unwrap());
      // Content-Type from the current file's mime if available, else octet-stream.
      // (Look up via state.filecache.lookup against the current path — best-effort.)
      resp.headers_mut().insert("Content-Type", "application/octet-stream".parse().unwrap());
      resp
  }
  ```

  Verify `tokio_util` is already a dep on `crabcloud-http`; if not add it (workspace dep).

- [ ] **Step 2: Write e2e test**

  Create `crates/crabcloud-http/tests/dav_versions_get.rs`:
  - Setup user + write a file + snapshot one version via `state.versions.snapshot_if_needed`.
  - Send `GET /dav/versions/{uid}/{fileid}/{version_mtime}` with Basic auth.
  - Assert 200; body bytes == the snapshotted bytes; `Content-Length` header matches.
  - 404 case: nonexistent version_mtime.

- [ ] **Step 3: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test dav_versions_get
  git add crates/crabcloud-http/src/routes/versions/get.rs crates/crabcloud-http/tests/dav_versions_get.rs
  git commit -m "versions dav: GET (download version bytes)"
  ```

### Task B4: `COPY` — restore via Destination header

**Files:**
- Modify: `crates/crabcloud-http/src/routes/versions/copy.rs`
- Create: `crates/crabcloud-http/tests/dav_versions_copy.rs`

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
      let uid = super::path_param(&req, "uid");
      let fileid: Option<i64> = super::path_param(&req, "fileid").and_then(|s| s.parse().ok());
      let version_mtime: Option<i64> = super::path_param(&req, "version_mtime").and_then(|s| s.parse().ok());
      let (Some(uid), Some(fileid), Some(version_mtime)) = (uid, fileid, version_mtime) else {
          return (StatusCode::NOT_FOUND, "").into_response();
      };
      if uid != authed.uid.as_str() {
          return (StatusCode::FORBIDDEN, "").into_response();
      }
      // Destination header is required.
      let dest = match req.headers().get("Destination").and_then(|v| v.to_str().ok()) {
          Some(s) => s.to_string(),
          None => return (StatusCode::BAD_REQUEST, "Destination header required").into_response(),
      };
      // Look up row.
      let entries = match state.versions.list_for(&uid, fileid).await {
          Ok(v) => v,
          Err(e) => return super::versions_err(e).into_response(),
      };
      let entry = match entries.into_iter().find(|e| e.version_mtime == version_mtime) {
          Some(e) => e,
          None => return (StatusCode::NOT_FOUND, "").into_response(),
      };

      // Verify Destination matches the current path of fileid (entry.path).
      // Accept both /dav/files/{uid}/<path> and /remote.php/dav/files/{uid}/<path>.
      let expected_paths = [
          format!("/dav/files/{uid}{}", entry.path),
          format!("/remote.php/dav/files/{uid}{}", entry.path),
      ];
      let dest_path = match parse_dest(&dest) {
          Some(p) => p,
          None => return (StatusCode::BAD_REQUEST, "").into_response(),
      };
      if !expected_paths.iter().any(|p| p == &dest_path) {
          return (StatusCode::BAD_REQUEST, "Destination must point at the current file path").into_response();
      }

      // Etag preflight: caller may send If-Match; verify against the current
      // filecache row's etag. Skip if no If-Match provided.
      // (Implementation here — read state.filecache.lookup; compare).
      // For MVP, accept restores without If-Match.

      // Look up current size for the snapshot-before-restore.
      let current_size = match state.filecache.lookup(/*storage_id*/ entry.storage_id, /*storage_path*/ &/*...*/).await {
          Ok(Some(r)) => r.size,
          _ => 0,
      };
      // Note: resolving storage_path here requires translating entry.path into a
      // StoragePath under the owner's home storage. Mirror how View::delete does it,
      // or factor a helper. The exact shape depends on the filecache lookup signature.

      let now = chrono::Utc::now().timestamp();
      let cfg = &state.config;
      if let Err(e) = state.versions.restore(
          &uid, entry.id,
          current_size,
          now,
          cfg.versions_min_interval_secs as i64,
          cfg.versions_max_bytes,
      ).await {
          return super::versions_err(e).into_response();
      }
      (StatusCode::NO_CONTENT, "").into_response()
  }

  fn parse_dest(dest: &str) -> Option<String> {
      // Strip absolute URL prefix + query/fragment, same as routes/trashbin/move_.rs.
      todo!("mirror routes/trashbin/move_.rs parse_destination")
  }
  ```

  Honest TODOs above to be filled by reading the parallel trashbin code. The signature of `filecache.lookup` and the storage_path translation needs to be sourced from the existing trashbin handler's pattern.

- [ ] **Step 2: e2e test**

  - Restore happy path: seed v1, write v2 to current, COPY-restore to v1 → assert current bytes == v1's bytes + a NEW version row was added covering v2.
  - 400 when Destination header missing.
  - 400 when Destination doesn't match the file's current path.
  - 404 when version_mtime doesn't exist.

- [ ] **Step 3: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test dav_versions_copy
  git add crates/crabcloud-http/src/routes/versions/copy.rs crates/crabcloud-http/tests/dav_versions_copy.rs
  git commit -m "versions dav: COPY (restore via Destination)"
  ```

### Task B5: Mount the versions router

**Files:**
- Modify: `crates/crabcloud-http/src/router.rs`

- [ ] **Step 1: Mount at both prefixes**

  In `build_router`, near the trashbin router definition, add:
  ```rust
  let versions_router = Router::new()
      .nest(
          "/remote.php/dav/versions",
          crate::routes::versions::router().with_state(state.clone()),
      )
      .nest(
          "/dav/versions",
          crate::routes::versions::router().with_state(state.clone()),
      );
  ```

  Merge into the final router alongside `trashbin_router`.

- [ ] **Step 2: Build + test**

  ```bash
  cargo build -p crabcloud-http
  cargo test -p crabcloud-http
  ```

  Expected: all green.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-http/src/router.rs
  git commit -m "versions dav: mount router at /dav/versions and /remote.php/dav/versions"
  ```

### Task B6: Batch B pre-PR

- [ ] **Step 1: Pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push + PR**

  ```bash
  git push -u origin sp13/b-dav
  gh pr create --title "sp13(b): DAV /dav/versions/{uid}/{fileid} (PROPFIND, GET, COPY)" \
    --body "Batch B of SP13 versioning. Nextcloud-compatible DAV versions endpoint mounted at both /dav/versions and /remote.php/dav/versions. PROPFIND lists/inspects, GET downloads version bytes, COPY-with-Destination restores. Spec: docs/superpowers/specs/2026-05-17-file-versioning-design.md."
  ```

---

# Batch C — OCS + server fns

**Branch:** `sp13/c-ocs-and-server-fns` (off the merged Batch B master)

### Task C1: OCS endpoints

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/files_versions.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`

- [ ] **Step 1: Write `files_versions.rs`**

  Mirror `routes/ocs/files_trashbin.rs` precisely. Endpoints:
  - GET `/versions/{fileid}` → list
  - POST `/restore/{version_id}` → restore
  - DELETE `/version/{version_id}` → delete

  Use the shared `envelope` helpers from the polish-G refactor.

  ```rust
  //! OCS endpoints for file versions.
  //!
  //! /ocs/v2.php/apps/files_versions/api/v1/
  //!   GET    /versions/{fileid}        — list
  //!   POST   /restore/{version_id}     — restore
  //!   DELETE /version/{version_id}     — delete

  use axum::extract::{Path, State};
  use axum::routing::{delete, get, post};
  use axum::{Json, Router};
  use crabcloud_core::AppState;
  use crabcloud_versions::VersionEntry;
  use serde::Serialize;

  pub fn router() -> Router<AppState> {
      Router::new()
          .route("/versions/{fileid}", get(list))
          .route("/restore/{version_id}", post(restore))
          .route("/version/{version_id}", delete(purge_one))
  }

  #[derive(Serialize)]
  pub struct VersionDto {
      pub id: i64,
      pub fileid: i64,
      pub version_mtime: i64,
      pub size: i64,
  }

  impl From<VersionEntry> for VersionDto {
      fn from(e: VersionEntry) -> Self {
          Self { id: e.id, fileid: e.fileid, version_mtime: e.version_mtime, size: e.size }
      }
  }

  async fn list(
      State(state): State<AppState>,
      Path(fileid): Path<i64>,
      // require_user-equivalent extractor — copy from files_trashbin.rs
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      let rows = match state.versions.list_for(&uid, fileid).await {
          Ok(r) => r,
          Err(e) => return super::envelope::ocs_envelope(500, format!("versions list: {e}"), serde_json::json!({})),
      };
      let dtos: Vec<VersionDto> = rows.into_iter().map(VersionDto::from).collect();
      super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({ "versions": dtos }))
  }

  async fn restore(
      State(state): State<AppState>,
      Path(version_id): Path<i64>,
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      let cfg = &state.config;
      let now = chrono::Utc::now().timestamp();
      // current_size: look it up by reading the version's entry to get fileid + storage_id,
      // then filecache.lookup. Best-effort; pass 0 if missing.
      let current_size = 0; // TODO: implement the lookup, mirror Batch B COPY.
      match state.versions.restore(
          &uid, version_id, current_size, now,
          cfg.versions_min_interval_secs as i64,
          cfg.versions_max_bytes,
      ).await {
          Ok(()) => super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({})),
          Err(e) => super::envelope::ocs_envelope(/* map_versions_err code */ 500, format!("restore: {e}"), serde_json::json!({})),
      }
  }

  async fn purge_one(
      State(state): State<AppState>,
      Path(version_id): Path<i64>,
  ) -> impl axum::response::IntoResponse {
      let uid = /* authed uid */;
      match state.versions.delete(&uid, version_id).await {
          Ok(()) => super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({})),
          Err(crabcloud_versions::VersionsError::NotFound) => super::envelope::ocs_envelope(404, "not found".into(), serde_json::json!({})),
          Err(crabcloud_versions::VersionsError::WrongUser) => super::envelope::ocs_envelope(403, "forbidden".into(), serde_json::json!({})),
          Err(e) => {
              tracing::error!(error = %e, "versions delete failed");
              super::envelope::ocs_envelope(500, format!("delete: {e}"), serde_json::json!({}))
          }
      }
  }
  ```

  Replace placeholders by reading the actual `files_trashbin.rs` shape.

- [ ] **Step 2: Mount in `routes/ocs/mod.rs`**

  Add `pub mod files_versions;` and nest under `/v2.php/apps/files_versions/api/v1` (mirror how `files_trashbin` is mounted).

- [ ] **Step 3: E2E test**

  Create `crates/crabcloud-http/tests/ocs_versions.rs`: list, restore, delete; cross-user forbidden; not-found.

- [ ] **Step 4: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test ocs_versions
  git add crates/crabcloud-http/src/routes/ocs/files_versions.rs crates/crabcloud-http/src/routes/ocs/mod.rs crates/crabcloud-http/tests/ocs_versions.rs
  git commit -m "versions ocs: /apps/files_versions/api/v1 endpoints"
  ```

### Task C2: Server fns

**Files:**
- Create: `crates/crabcloud-app/src/server_fns/versions.rs`
- Modify: `crates/crabcloud-app/src/server_fns/mod.rs`
- Modify: `crates/crabcloud-app/Cargo.toml`
- Create: `crates/crabcloud-app/tests/server_fns_versions.rs`

- [ ] **Step 1: Add `crabcloud-versions` dep on `crabcloud-app`**

- [ ] **Step 2: Write `versions.rs`**

  Mirror `server_fns/trash.rs` exactly. Three fns:

  ```rust
  #[server]
  pub async fn list_versions(fileid: i64) -> Result<Vec<VersionDto>, ServerFnError> { /* … */ }

  #[server]
  pub async fn restore_version(version_id: i64) -> Result<(), ServerFnError> { /* … */ }

  #[server]
  pub async fn delete_version(version_id: i64) -> Result<(), ServerFnError> { /* … */ }
  ```

  `VersionDto`: `{ id: i64, version_mtime: i64, size: i64 }`. All gated by `require_user()`.

  The `restore_version` body needs the current_size + config knobs — same lookup pattern as the OCS POST.

- [ ] **Step 3: Wire into `server_fns/mod.rs`**

  `pub mod versions;` + re-export.

- [ ] **Step 4: Integration test**

  Create `crates/crabcloud-app/tests/server_fns_versions.rs` mirroring `server_fns_trash.rs`. Round-trip: list → restore → file contents match version.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-app --test server_fns_versions
  git add crates/crabcloud-app/Cargo.toml crates/crabcloud-app/src/server_fns/ crates/crabcloud-app/tests/server_fns_versions.rs
  git commit -m "versions: server fns (list / restore / delete)"
  ```

### Task C3: Batch C pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp13/c-ocs-and-server-fns
  gh pr create --title "sp13(c): OCS /apps/files_versions/api/v1 + server fns" \
    --body "Batch C of SP13 versioning."
  ```

---

# Batch D — Dioxus UI

**Branch:** `sp13/d-ui` (off the merged Batch C master)

**Goal:** Add a "Versions" item to each file row's `…` menu that opens a `VersionsPanel` modal listing versions with Restore + Delete per row.

### Task D1: Versions panel component

**Files:**
- Create: `crates/crabcloud-app/src/pages/files/versions_panel.rs`
- Modify: `crates/crabcloud-app/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-app/src/pages/files/row.rs` (add "Versions" menu item)
- Modify: `crates/crabcloud-app/assets/app.css`

- [ ] **Step 1: Write `versions_panel.rs`**

  Mirror `pages/trash.rs` for the in-flight tracking + error banner + per-row confirm patterns. The panel is gated by a `pending_versions_fileid: Signal<Option<i64>>`. When `Some`, render a modal (reuse `.files-modal-*` chrome) with a list fetched via `list_versions(fileid)`.

  Each row: timestamp ("5 minutes ago" / formatted), size (human-readable e.g. "1.2 MiB"), Restore button, Delete button.

  Page-scoped `in_flight: Signal<HashSet<i64>>` for per-row mutation tracking. Page-scoped `last_error: Signal<Option<String>>` for the error banner.

  Concrete component shape: study `pages/trash.rs` and copy the pattern wholesale. Same auto-refresh-on-mutation flow.

- [ ] **Step 2: Wire the row "Versions" item**

  In `pages/files/row.rs` (or wherever the `…` menu lives), add:
  ```rust
  MenuItem { onclick: move |_| props.on_show_versions.call(entry.fileid), "Versions" }
  ```

  Plumb an `on_show_versions: EventHandler<i64>` prop. In `pages/files/mod.rs`, wire it to set `pending_versions_fileid.set(Some(fileid))`.

- [ ] **Step 3: Add CSS**

  In `assets/app.css`, add `.versions-panel-*` styles. Aim for ~50 lines: list rows with timestamp + size + actions, hover state, action button styling.

- [ ] **Step 4: SSR snapshot tests**

  Mirror the trash page snapshot tests. Cover:
  - Panel renders 2 versions with their timestamps + sizes.
  - Empty state: file has no versions → "No versions yet."
  - In-flight: row with mutation in progress shows disabled buttons.

- [ ] **Step 5: Build + WASM + tests**

  ```bash
  cargo test -p crabcloud-app
  cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
  cargo test --workspace
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-app/ crates/crabcloud-app/assets/app.css
  git commit -m "versions ui: per-file versions panel + row menu wire-up"
  ```

### Task D2: Batch D pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp13/d-ui
  gh pr create --title "sp13(d): versions UI (per-row 'Versions' panel)" \
    --body "Batch D of SP13 versioning. Per-row 'Versions' item in the file row × menu opens a modal listing versions with Restore + Delete per row. Reuses .files-modal-* chrome + the trash page's in-flight / error-banner patterns."
  ```

---

## Self-review notes

- **Spec coverage:** §1 goal → all batches. §2 decisions → A1–A10 (1–11), B (12), C (13, 15), D (14). §3 architecture → A. §4 schema → A1. §5 surfaces → B + C. §6 edge cases → tested across A (throttle, size, zero, cascade), B (Destination), C (cross-user). §7 testing list → e2e + unit at every layer. §8 batches → 4 batches.
- **Placeholder scan:** A few honest TODOs in the Batch B/C handler skeletons (e.g. "mirror the parallel trashbin pattern for storage_path translation") — these are calling out cross-cutting helper code the implementer should source from the existing SP12 codebase rather than re-derive. Each is precisely scoped to a specific reference. The `row_to_entry` placeholder in service.rs has explicit "replace with the pattern from crabcloud-trash" instruction.
- **Type consistency:** `VersionEntry` shape is consistent A2→A4→B→C. `VersionDto` (UI/server-fn DTO) consistent C1→C2→D1. `Versions::snapshot_if_needed` signature consistent A4→A9 (View) →B4 (restore caller) →C2 (server fn).
- **Known underspecified spots** the implementer must resolve from the codebase, not from this plan:
  - The exact `path_param` extraction style (axum 0.8 — mirror the trashbin handler's choice).
  - The exact OCS envelope helpers (`super::envelope::ocs_envelope` from the polish G refactor).
  - The `filecache.lookup` signature for storage_path translation in COPY/restore handlers — mirror the trashbin MOVE handler.
  - The exact storage_id resolution from a uid (for the trash cascade in A8 step 3) — mirror whatever `crabcloud-fs::HomeMountResolver` does to turn `uid → local::<datadir>/<uid>/files` numeric_id.
  - The exact `View::new` callsite ripple (A9 step 2) — count + update each.
