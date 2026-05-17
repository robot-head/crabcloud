# Activity Feed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Nextcloud-compatible per-user activity feeds. Every file CRUD, share, trash restore, and version restore emits one row per recipient into `oc_activity` via a new `crabcloud-activity` crate. Recipients see their feed via OCS (`/ocs/v2.php/apps/activity/api/v2/activity`), via server fns, and via a new "Activity" sidebar entry → `/activity` page with infinite-scroll + per-event-type settings. Tiered events get coalesced within a 10-minute window. Background sweeper purges rows older than `activity_retention_days` (default 365).

**Architecture:** New `crabcloud-activity` crate owns `Activity::{emit, list, sweep_expired}` and `ActivitySettings::{get, set, get_all_for_user}` over multidialect SQL. New `ActivityEmitter` trait (in `crabcloud-activity`) is implemented by `Activity`; emitter crates (`crabcloud-fs`, `crabcloud-sharing`, `crabcloud-versions`, `crabcloud-trash`) take `Arc<dyn ActivityEmitter>` and call `emit(ActivityEvent { recipients: Vec<UserId>, … })` after each user-driven action. Recipient resolution happens at emit sites (no share lookup inside the activity crate; emitters compose the recipient list themselves). Coalesce check: if a row with matching `(affected_user, actor, event_type, object_id)` exists within `activity_coalesce_window_secs` (default 600), UPDATE its `count + last_seen_at`; otherwise INSERT.

**Tech Stack:** Rust 1.95, sqlx 0.8 (sqlite + mysql + postgres), axum 0.8, Dioxus 0.7 fullstack. No new external dependencies.

**Spec:** `docs/superpowers/specs/2026-05-17-activity-feed-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is its own PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp14/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-activity-crate`, `b-ocs`, `c-server-fns`, `d-ui`.

- **Commit cadence:** Commit at every "Commit" step. Each batch lands as one squash-merged PR; intermediate commits get squashed.

- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- **Established workaround for AppState tests:** Tests building `AppState` set `cfg.filecache.enabled = false`, `cfg.mail.transport = "disabled"`, `cfg.trash_retention_days = 30`, `cfg.versions_retention_disabled = false`. Add: `cfg.activity_retention_days = 365` (default, override to 0 for tests that disable sweeping). `cfg.activity_coalesce_window_secs = 600` (default, override to 0 to disable coalesce when a test wants distinct rows per emit).

- **Pre-existing patterns to mirror:**
  - **Crate shape:** `crates/crabcloud-versions/` (SP13) — focused service crate, multidialect SQL via `match self.pool.as_ref()`, per-dialect inline row decode, error type in `error.rs`, types in `types.rs`.
  - **Background sweeper:** `crates/crabcloud-core/src/versions_sweeper.rs` and `trash_sweeper.rs` — `pub fn new(...) -> (Self, Arc<Notify>)`, `pub async fn run(self)` with `tokio::select!` shutdown, `pub async fn sweep_once()` for sync test drive.
  - **Migration triplet:** `migrations/core/0010_files_versions/{sqlite,mysql,postgres}.sql`. Next migration number is `0011`.
  - **Trait pattern for cycle-free wiring:** `crates/crabcloud-sharing/src/mail.rs::MailEnqueuer` (SP11) — implementer crate defines the trait; emitter crates depend on the trait crate; implementer is wired via `Arc<dyn …>` in `AppState`. SP14 inverts this slightly: trait lives in `crabcloud-activity` (the implementer); emitter crates depend on `crabcloud-activity` for the trait. No cycle because activity doesn't depend back.
  - **OCS module shape:** `crates/crabcloud-http/src/routes/ocs/files_versions.rs` (SP13 Batch C) — uses shared `super::envelope::*` helpers + `Extension<AuthContext>`.
  - **Server fns:** `crates/crabcloud-app/src/server_fns/versions.rs` (SP13) — `require_user()` extractor, centralized `map_*_err` helper.
  - **UI page:** `crates/crabcloud-app/src/pages/trash.rs` (post-polish) — sidebar entry + dedicated route + per-row in-flight + dismissable error banner + reused `.files-modal-*` chrome.

---

## File-by-file map

### New crate: `crabcloud-activity`

```
crates/crabcloud-activity/
├── Cargo.toml
├── src/
│   ├── lib.rs        — re-exports + crate doc
│   ├── error.rs      — ActivityError + ActivityEmitError
│   ├── emitter.rs    — ActivityEmitter trait + NoopEmitter (for tests)
│   ├── service.rs    — Activity struct + impl ActivityEmitter (emit / list / sweep_expired)
│   ├── settings.rs   — ActivitySettings struct (get / set / get_all_for_user / stream_enabled)
│   ├── sql.rs        — multidialect SQL constants
│   ├── subjects.rs   — subject_id → English template map + render(subject_id, params)
│   └── types.rs      — ActivityEvent, ActivityRow, ActivitySetting, EventType, ObjectType
└── tests/
    └── activity_e2e.rs   — sqlite e2e (emit + coalesce + opt-out + list cursor + sweep + cross-recipient)
```

### New migration

```
migrations/core/0011_activity_and_settings/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-activity` member.
- `crates/crabcloud-fs/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-fs/src/view.rs` — emit hooks in `write_file` / `delete` / `hard_delete` / `rename` / `rename_force_overwrite`. New `activity: Arc<dyn ActivityEmitter>` field on `View` (added via `ViewConfig`-equivalent or as an additional positional arg, per the SP13 Batch A precedent — kept positional with `VersionsHooks` shape).
- `crates/crabcloud-trash/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-trash/src/service.rs` — `Trash::restore` emits `file_restored`. `Trash::new` gains an `activity: Arc<dyn ActivityEmitter>` field.
- `crates/crabcloud-sharing/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-sharing/src/service.rs` — `Shares::create` and `Shares::delete` emit `share_created` / `share_deleted`. The service gains an `activity` field via the existing `SharesConfig` struct (SP12 polish C precedent).
- `crates/crabcloud-versions/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-versions/src/service.rs` — `Versions::restore` emits `version_restored`. `Versions::new` gains an `activity` field.
- `crates/crabcloud-config/src/types.rs` — `activity_retention_days: u32`, `activity_coalesce_window_secs: u32` fields + default fns.
- `crates/crabcloud-config/src/test_support.rs` — fills the two new fields.
- `crates/crabcloud-core/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-core/src/activity_sweeper.rs` (new) — `ActivitySweeper::{new, run, sweep_once}`.
- `crates/crabcloud-core/src/lib.rs` — `mod activity_sweeper;` + re-export.
- `crates/crabcloud-core/src/state.rs` — construct `Activity` + `ActivitySettings` + spawn sweeper; expose `AppState.activity`, `AppState.activity_settings`, `AppState.activity_sweeper_shutdown`. Pass the activity emitter into trash + versions + shares + views.
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-http/src/routes/ocs/activity.rs` (new) — OCS endpoints.
- `crates/crabcloud-http/src/routes/ocs/mod.rs` — mount `apps/activity/api/v2/activity`.
- `crates/crabcloud-app/Cargo.toml` — adds `crabcloud-activity` workspace dep.
- `crates/crabcloud-app/src/server_fns/activity.rs` (new) — 3 server fns.
- `crates/crabcloud-app/src/server_fns/mod.rs` — `pub mod activity;` + re-export.
- `crates/crabcloud-app/src/pages/activity.rs` (new) — list page.
- `crates/crabcloud-app/src/pages/activity_settings.rs` (new) — settings page.
- `crates/crabcloud-app/src/pages/mod.rs` — `pub mod activity; pub mod activity_settings;`.
- `crates/crabcloud-app/src/app.rs` — `/activity` and `/activity/settings` routes.
- `crates/crabcloud-app/src/pages/files/chrome.rs` — "Activity" sidebar entry.
- `crates/crabcloud-app/assets/app.css` — `.activity-*` styles (~60 lines).

---

# Batch A — `crabcloud-activity` core + emit hooks + sweeper

**Branch:** `sp14/a-activity-crate`

**Goal:** Stand up the activity crate (`Activity` + `ActivitySettings` + `ActivityEmitter` trait), the 0011 migration, the `ActivitySweeper`, two config knobs (`activity_retention_days`, `activity_coalesce_window_secs`), wire emit hooks in all 4 emitter crates (fs, trash, sharing, versions), and have `AppState` expose the new handles.

After this batch:
- `crabcloud-activity` crate compiles with full multidialect SQL + sqlite e2e tests passing.
- Migration `0011_activity_and_settings` registered + runs in the test pool.
- Every authed file CRUD, share create/delete, trash restore, and version restore emits one row per recipient.
- `ActivitySweeper` spawned in `AppStateBuilder::build`.
- `AppState.activity`, `AppState.activity_settings`, `AppState.activity_sweeper_shutdown` exposed.
- No surface yet (OCS / server fn / UI land in B / C / D).

### Task A1: Migration `0011_activity_and_settings`

**Files:**
- Create: `migrations/core/0011_activity_and_settings/sqlite.sql`
- Create: `migrations/core/0011_activity_and_settings/mysql.sql`
- Create: `migrations/core/0011_activity_and_settings/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs` (whatever holds `core_set()`)

- [ ] **Step 1: Confirm migration registration pattern**

  Read the existing `0010_files_versions` registration in the core migrations file. New entry registers identically (next sequence number = 11).

- [ ] **Step 2: Write `sqlite.sql`**

  ```sql
  CREATE TABLE oc_activity (
      id              INTEGER PRIMARY KEY AUTOINCREMENT,
      affected_user   VARCHAR(64)  NOT NULL,
      actor           VARCHAR(64)  NOT NULL,
      event_type      VARCHAR(64)  NOT NULL,
      subject_id      VARCHAR(128) NOT NULL,
      subject_params  TEXT         NOT NULL,
      object_type     VARCHAR(32)  NOT NULL,
      object_id       BIGINT       NULL,
      occurred_at     BIGINT       NOT NULL,
      last_seen_at    BIGINT       NOT NULL,
      count           INTEGER      NOT NULL DEFAULT 1
  );

  CREATE INDEX idx_activity_user_time ON oc_activity (affected_user, occurred_at DESC);
  CREATE INDEX idx_activity_coalesce  ON oc_activity (affected_user, actor, event_type, object_id, last_seen_at);

  CREATE TABLE oc_activity_settings (
      user_id     VARCHAR(64) NOT NULL,
      event_type  VARCHAR(64) NOT NULL,
      stream      BOOLEAN     NOT NULL DEFAULT 1,
      PRIMARY KEY (user_id, event_type)
  );
  ```

- [ ] **Step 3: Write `mysql.sql`**

  ```sql
  CREATE TABLE oc_activity (
      id              BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
      affected_user   VARCHAR(64)  NOT NULL,
      actor           VARCHAR(64)  NOT NULL,
      event_type      VARCHAR(64)  NOT NULL,
      subject_id      VARCHAR(128) NOT NULL,
      subject_params  TEXT         NOT NULL,
      object_type     VARCHAR(32)  NOT NULL,
      object_id       BIGINT       NULL,
      occurred_at     BIGINT       NOT NULL,
      last_seen_at    BIGINT       NOT NULL,
      count           INT          NOT NULL DEFAULT 1,
      INDEX idx_activity_user_time (affected_user, occurred_at DESC),
      INDEX idx_activity_coalesce  (affected_user, actor, event_type, object_id, last_seen_at)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

  CREATE TABLE oc_activity_settings (
      user_id     VARCHAR(64)  NOT NULL,
      event_type  VARCHAR(64)  NOT NULL,
      stream      TINYINT(1)   NOT NULL DEFAULT 1,
      PRIMARY KEY (user_id, event_type)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
  ```

- [ ] **Step 4: Write `postgres.sql`**

  ```sql
  CREATE TABLE oc_activity (
      id              BIGSERIAL    PRIMARY KEY,
      affected_user   VARCHAR(64)  NOT NULL,
      actor           VARCHAR(64)  NOT NULL,
      event_type      VARCHAR(64)  NOT NULL,
      subject_id      VARCHAR(128) NOT NULL,
      subject_params  TEXT         NOT NULL,
      object_type     VARCHAR(32)  NOT NULL,
      object_id       BIGINT       NULL,
      occurred_at     BIGINT       NOT NULL,
      last_seen_at    BIGINT       NOT NULL,
      count           INTEGER      NOT NULL DEFAULT 1
  );

  CREATE INDEX idx_activity_user_time ON oc_activity (affected_user, occurred_at DESC);
  CREATE INDEX idx_activity_coalesce  ON oc_activity (affected_user, actor, event_type, object_id, last_seen_at);

  CREATE TABLE oc_activity_settings (
      user_id     VARCHAR(64) NOT NULL,
      event_type  VARCHAR(64) NOT NULL,
      stream      BOOLEAN     NOT NULL DEFAULT TRUE,
      PRIMARY KEY (user_id, event_type)
  );
  ```

- [ ] **Step 5: Register in core migrations**

  Add the new directory entry to `core_set()` mirroring the 0010 registration.

- [ ] **Step 6: Verify migration runs**

  ```bash
  cargo test -p crabcloud-db
  ```

  Expected: all migration tests pass; the new 0011 directory is registered.

- [ ] **Step 7: Commit**

  ```bash
  git add migrations/core/0011_activity_and_settings crates/crabcloud-db/src/core_migrations.rs
  git commit -m "activity: 0011_activity_and_settings migration triplet"
  ```

### Task A2: Crate skeleton

**Files:**
- Create: `crates/crabcloud-activity/Cargo.toml`
- Create: `crates/crabcloud-activity/src/lib.rs`
- Create: `crates/crabcloud-activity/src/error.rs`
- Create: `crates/crabcloud-activity/src/types.rs`
- Create: `crates/crabcloud-activity/src/emitter.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Register the crate**

  In root `Cargo.toml`:
  - Add `"crates/crabcloud-activity",` to `members`.
  - Add to `[workspace.dependencies]`:
    ```toml
    crabcloud-activity = { path = "crates/crabcloud-activity" }
    ```

- [ ] **Step 2: Write `Cargo.toml`**

  Mirror `crates/crabcloud-versions/Cargo.toml` (the most recent service crate). Drop `crabcloud-filecache` + `crabcloud-storage` (activity doesn't touch them).

  ```toml
  [package]
  name = "crabcloud-activity"
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
  //! Activity feed service for Crabcloud.
  //!
  //! Spec: `docs/superpowers/specs/2026-05-17-activity-feed-design.md`.
  //!
  //! Public entry points: [`Activity`] (emit/list/sweep), [`ActivitySettings`]
  //! (per-user-per-event-type opt-out), and the [`ActivityEmitter`] trait
  //! emitter crates depend on. SQL dispatch mirrors the
  //! `crabcloud-versions` / `crabcloud-trash` pattern.

  mod emitter;
  mod error;
  mod service;
  mod settings;
  mod sql;
  mod subjects;
  mod types;

  pub use emitter::{ActivityEmitter, NoopEmitter};
  pub use error::{ActivityEmitError, ActivityError};
  pub use service::Activity;
  pub use settings::ActivitySettings;
  pub use subjects::render_subject;
  pub use types::{
      ActivityEvent, ActivityRow, ActivitySetting, EventType, ObjectType,
  };
  ```

- [ ] **Step 4: Write `src/error.rs`**

  ```rust
  use thiserror::Error;

  #[derive(Debug, Error)]
  pub enum ActivityError {
      #[error("row not found")]
      NotFound,
      #[error("db: {0}")]
      Db(#[from] sqlx::Error),
      #[error("json: {0}")]
      Json(#[from] serde_json::Error),
  }

  /// Wrapper error type returned by [`crate::ActivityEmitter::emit`] so
  /// emitter crates can depend on a stable boundary type rather than the
  /// concrete [`ActivityError`].
  #[derive(Debug, Error)]
  #[error("activity emit failed: {0}")]
  pub struct ActivityEmitError(pub String);

  impl From<ActivityError> for ActivityEmitError {
      fn from(e: ActivityError) -> Self {
          Self(e.to_string())
      }
  }
  ```

- [ ] **Step 5: Write `src/types.rs`**

  ```rust
  //! Public-facing value types for the activity service.

  use crabcloud_users::UserId;
  use serde::{Deserialize, Serialize};

  /// Discriminates the event categories MVP supports. Wire form is the
  /// `as_str()` value stored in `oc_activity.event_type`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
  pub enum EventType {
      FileCreated,
      FileUpdated,
      FileDeleted,
      FileRenamed,
      FileRestored,
      ShareCreated,
      ShareDeleted,
      VersionRestored,
  }

  impl EventType {
      pub fn as_str(&self) -> &'static str {
          match self {
              EventType::FileCreated     => "file_created",
              EventType::FileUpdated     => "file_updated",
              EventType::FileDeleted     => "file_deleted",
              EventType::FileRenamed     => "file_renamed",
              EventType::FileRestored    => "file_restored",
              EventType::ShareCreated    => "share_created",
              EventType::ShareDeleted    => "share_deleted",
              EventType::VersionRestored => "version_restored",
          }
      }

      pub fn from_str(s: &str) -> Option<Self> {
          match s {
              "file_created"     => Some(Self::FileCreated),
              "file_updated"     => Some(Self::FileUpdated),
              "file_deleted"     => Some(Self::FileDeleted),
              "file_renamed"     => Some(Self::FileRenamed),
              "file_restored"    => Some(Self::FileRestored),
              "share_created"    => Some(Self::ShareCreated),
              "share_deleted"    => Some(Self::ShareDeleted),
              "version_restored" => Some(Self::VersionRestored),
              _ => None,
          }
      }
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "lowercase")]
  pub enum ObjectType {
      File,
      Share,
      Version,
  }

  impl ObjectType {
      pub fn as_str(&self) -> &'static str {
          match self {
              ObjectType::File    => "file",
              ObjectType::Share   => "share",
              ObjectType::Version => "version",
          }
      }

      pub fn from_str(s: &str) -> Option<Self> {
          match s {
              "file"    => Some(Self::File),
              "share"   => Some(Self::Share),
              "version" => Some(Self::Version),
              _ => None,
          }
      }
  }

  /// Input to [`crate::ActivityEmitter::emit`]. Emitter sites construct
  /// this with recipient list already resolved (actor + share recipients
  /// + group members where applicable).
  #[derive(Debug, Clone)]
  pub struct ActivityEvent {
      pub actor: String,                          // "" for public-link / system
      pub event_type: EventType,
      pub subject_id_actor: String,               // i18n key for the actor row ("file_updated_you")
      pub subject_id_recipient: String,           // i18n key for non-actor rows ("file_updated_by")
      pub subject_params: serde_json::Value,      // {"file": "...", "actor": "..."}
      pub object_type: ObjectType,
      pub object_id: Option<i64>,
      pub recipients: Vec<UserId>,                // includes the actor if they should see it
      pub occurred_at: i64,                       // unix seconds; emit site passes now()
  }

  /// Row returned from [`crate::Activity::list`].
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ActivityRow {
      pub id: i64,
      pub affected_user: String,
      pub actor: String,
      pub event_type: String,
      pub subject_id: String,
      pub subject_params: serde_json::Value,
      pub object_type: String,
      pub object_id: Option<i64>,
      pub occurred_at: i64,
      pub last_seen_at: i64,
      pub count: i32,
  }

  /// Row returned from [`crate::ActivitySettings::get_all_for_user`].
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ActivitySetting {
      pub event_type: String,
      pub stream: bool,
  }
  ```

- [ ] **Step 6: Write `src/emitter.rs`**

  ```rust
  //! The [`ActivityEmitter`] trait. Emitter crates depend on this trait
  //! and accept `Arc<dyn ActivityEmitter>` so they don't pull in the
  //! concrete service implementation (mirrors `MailEnqueuer` precedent).
  //!
  //! A [`NoopEmitter`] is provided for tests / configurations that want to
  //! skip activity logging without an `Option<Arc<...>>` plumbing dance.

  use crate::error::ActivityEmitError;
  use crate::types::ActivityEvent;
  use async_trait::async_trait;

  #[async_trait]
  pub trait ActivityEmitter: Send + Sync {
      async fn emit(&self, event: ActivityEvent) -> Result<(), ActivityEmitError>;
  }

  /// Drops every event. Useful for unit tests and for the boot phase
  /// before `Activity` is constructed (the `AppState` builder threads the
  /// real emitter through after construction).
  pub struct NoopEmitter;

  #[async_trait]
  impl ActivityEmitter for NoopEmitter {
      async fn emit(&self, _: ActivityEvent) -> Result<(), ActivityEmitError> {
          Ok(())
      }
  }
  ```

- [ ] **Step 7: Stub `src/service.rs`, `src/settings.rs`, `src/sql.rs`, `src/subjects.rs`**

  These are filled in subsequent tasks. Add minimal stubs so the crate compiles in this step:

  ```rust
  // src/sql.rs
  //! Multidialect SQL constants. Filled in Task A3.
  ```

  ```rust
  // src/service.rs
  //! Activity service. Filled in Task A4.

  use crate::emitter::ActivityEmitter;
  use crate::error::{ActivityEmitError, ActivityError};
  use crate::settings::ActivitySettings;
  use crate::types::{ActivityEvent, ActivityRow};
  use crabcloud_db::DbPool;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Activity {
      #[allow(dead_code)]
      pool: Arc<DbPool>,
      #[allow(dead_code)]
      settings: ActivitySettings,
      #[allow(dead_code)]
      coalesce_window_secs: i64,
  }

  impl Activity {
      pub fn new(pool: Arc<DbPool>, settings: ActivitySettings, coalesce_window_secs: i64) -> Self {
          Self { pool, settings, coalesce_window_secs }
      }

      pub async fn list(
          &self,
          _affected_user: &str,
          _since: Option<i64>,
          _limit: i64,
      ) -> Result<Vec<ActivityRow>, ActivityError> {
          Ok(Vec::new())
      }

      pub async fn sweep_expired(&self, _cutoff: i64) -> Result<u64, ActivityError> {
          Ok(0)
      }
  }

  #[async_trait::async_trait]
  impl ActivityEmitter for Activity {
      async fn emit(&self, _event: ActivityEvent) -> Result<(), ActivityEmitError> {
          Ok(())
      }
  }
  ```

  ```rust
  // src/settings.rs
  //! ActivitySettings — per-user-per-event stream toggles. Filled in Task A5.

  use crabcloud_db::DbPool;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct ActivitySettings {
      #[allow(dead_code)]
      pool: Arc<DbPool>,
  }

  impl ActivitySettings {
      pub fn new(pool: Arc<DbPool>) -> Self {
          Self { pool }
      }
  }
  ```

  ```rust
  // src/subjects.rs
  //! Subject template rendering. Filled in Task A6.

  pub fn render_subject(subject_id: &str, _params: &serde_json::Value) -> String {
      subject_id.to_string()
  }
  ```

- [ ] **Step 8: Build**

  ```bash
  cargo build -p crabcloud-activity
  ```

  Expected: clean.

- [ ] **Step 9: Commit**

  ```bash
  git add Cargo.toml crates/crabcloud-activity/
  git commit -m "activity: crate skeleton (trait + types + error + stub service)"
  ```

### Task A3: Multidialect SQL constants

**Files:**
- Modify: `crates/crabcloud-activity/src/sql.rs`

- [ ] **Step 1: Write the constants**

  Mirror `crates/crabcloud-versions/src/sql.rs` / `crates/crabcloud-trash/src/sql.rs` for the `_QM` / `_PG` split.

  ```rust
  //! Multidialect SQL constants for the activity service.
  //!
  //! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres (`$N`).

  // -- INSERT a new activity row. Returns id via RETURNING (pg) or
  //    last_insert_{rowid,id} (sqlite/mysql).
  pub const INSERT_QM: &str = "\
      INSERT INTO oc_activity \
      (affected_user, actor, event_type, subject_id, subject_params, \
       object_type, object_id, occurred_at, last_seen_at, count) \
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)";

  pub const INSERT_PG: &str = "\
      INSERT INTO oc_activity \
      (affected_user, actor, event_type, subject_id, subject_params, \
       object_type, object_id, occurred_at, last_seen_at, count) \
      VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1) RETURNING id";

  // -- LIST rows for one user, descending id, with optional `since` cursor
  //    (exclusive: id < since).
  pub const LIST_QM: &str = "\
      SELECT id, affected_user, actor, event_type, subject_id, subject_params, \
             object_type, object_id, occurred_at, last_seen_at, count \
      FROM oc_activity \
      WHERE affected_user = ? AND (? = 0 OR id < ?) \
      ORDER BY id DESC LIMIT ?";

  pub const LIST_PG: &str = "\
      SELECT id, affected_user, actor, event_type, subject_id, subject_params, \
             object_type, object_id, occurred_at, last_seen_at, count \
      FROM oc_activity \
      WHERE affected_user = $1 AND ($2 = 0 OR id < $2) \
      ORDER BY id DESC LIMIT $3";

  // -- COALESCE probe: most recent row matching (recipient, actor, event,
  //    object_id) within last_seen_at >= cutoff. Used to decide INSERT vs
  //    UPDATE in `emit`.
  pub const COALESCE_PROBE_QM: &str = "\
      SELECT id FROM oc_activity \
      WHERE affected_user = ? AND actor = ? AND event_type = ? \
        AND ((object_id IS NULL AND ? IS NULL) OR object_id = ?) \
        AND last_seen_at >= ? \
      ORDER BY last_seen_at DESC LIMIT 1";

  pub const COALESCE_PROBE_PG: &str = "\
      SELECT id FROM oc_activity \
      WHERE affected_user = $1 AND actor = $2 AND event_type = $3 \
        AND ((object_id IS NULL AND $4 IS NULL) OR object_id = $4) \
        AND last_seen_at >= $5 \
      ORDER BY last_seen_at DESC LIMIT 1";

  // -- COALESCE update: bump count + last_seen_at + subject_params.
  pub const COALESCE_UPDATE_QM: &str = "\
      UPDATE oc_activity SET count = count + 1, last_seen_at = ?, subject_params = ? \
      WHERE id = ?";

  pub const COALESCE_UPDATE_PG: &str = "\
      UPDATE oc_activity SET count = count + 1, last_seen_at = $1, subject_params = $2 \
      WHERE id = $3";

  // -- DELETE expired rows.
  pub const DELETE_EXPIRED_QM: &str = "DELETE FROM oc_activity WHERE occurred_at < ?";
  pub const DELETE_EXPIRED_PG: &str = "DELETE FROM oc_activity WHERE occurred_at < $1";

  // -- Settings: GET single toggle, GET all for user, UPSERT toggle.
  pub const SETTINGS_GET_QM: &str = "\
      SELECT stream FROM oc_activity_settings WHERE user_id = ? AND event_type = ?";

  pub const SETTINGS_GET_PG: &str = "\
      SELECT stream FROM oc_activity_settings WHERE user_id = $1 AND event_type = $2";

  pub const SETTINGS_GET_ALL_QM: &str = "\
      SELECT event_type, stream FROM oc_activity_settings WHERE user_id = ?";

  pub const SETTINGS_GET_ALL_PG: &str = "\
      SELECT event_type, stream FROM oc_activity_settings WHERE user_id = $1";

  // -- UPSERT — per-dialect; sqlite uses ON CONFLICT, mysql uses
  //    ON DUPLICATE KEY UPDATE, postgres uses ON CONFLICT.
  pub const SETTINGS_UPSERT_SQLITE: &str = "\
      INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES (?, ?, ?) \
      ON CONFLICT (user_id, event_type) DO UPDATE SET stream = excluded.stream";

  pub const SETTINGS_UPSERT_MYSQL: &str = "\
      INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES (?, ?, ?) \
      ON DUPLICATE KEY UPDATE stream = VALUES(stream)";

  pub const SETTINGS_UPSERT_PG: &str = "\
      INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES ($1, $2, $3) \
      ON CONFLICT (user_id, event_type) DO UPDATE SET stream = excluded.stream";
  ```

- [ ] **Step 2: Build**

  ```bash
  cargo build -p crabcloud-activity
  ```

  Expected: clean.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-activity/src/sql.rs
  git commit -m "activity: multidialect SQL constants"
  ```

### Task A4: `Activity` service — TDD with sqlite e2e

**Files:**
- Modify: `crates/crabcloud-activity/src/service.rs`
- Create: `crates/crabcloud-activity/tests/activity_e2e.rs`

This is the meat of Batch A.

- [ ] **Step 1: Write the e2e test file (RED)**

  Create `crates/crabcloud-activity/tests/activity_e2e.rs`:

  ```rust
  //! sqlite e2e for the Activity service + ActivitySettings + coalescing.

  use crabcloud_activity::{
      Activity, ActivityEmitter, ActivityEvent, ActivitySettings, EventType, ObjectType,
  };
  use crabcloud_config::test_support::minimal_sqlite_config;
  use crabcloud_db::{core_set, DbPool, MigrationRunner};
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

  fn event(now: i64, recipients: Vec<&str>, object_id: Option<i64>) -> ActivityEvent {
      ActivityEvent {
          actor: "alice".into(),
          event_type: EventType::FileUpdated,
          subject_id_actor: "file_updated_you".into(),
          subject_id_recipient: "file_updated_by".into(),
          subject_params: serde_json::json!({ "file": "report.docx", "actor": "alice" }),
          object_type: ObjectType::File,
          object_id,
          recipients: recipients.into_iter().map(uid).collect(),
          occurred_at: now,
      }
  }

  #[tokio::test]
  async fn emit_writes_one_row_per_recipient() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

      activity.emit(event(1_000, vec!["alice", "bob"], Some(42))).await.unwrap();

      let alice_rows = activity.list("alice", None, 100).await.unwrap();
      let bob_rows   = activity.list("bob",   None, 100).await.unwrap();
      assert_eq!(alice_rows.len(), 1);
      assert_eq!(bob_rows.len(),   1);
      assert_eq!(alice_rows[0].count, 1);
      assert_eq!(alice_rows[0].subject_id, "file_updated_you");
      assert_eq!(bob_rows[0].subject_id,   "file_updated_by");
  }

  #[tokio::test]
  async fn emit_coalesces_within_window() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

      activity.emit(event(1_000, vec!["alice"], Some(42))).await.unwrap();
      activity.emit(event(1_100, vec!["alice"], Some(42))).await.unwrap();
      activity.emit(event(1_200, vec!["alice"], Some(42))).await.unwrap();

      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 1, "three emits within window should coalesce into one row");
      assert_eq!(rows[0].count, 3);
      assert_eq!(rows[0].last_seen_at, 1_200);
      assert_eq!(rows[0].occurred_at,  1_000);
  }

  #[tokio::test]
  async fn emit_outside_window_inserts_new() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

      activity.emit(event(1_000, vec!["alice"], Some(42))).await.unwrap();
      activity.emit(event(2_000, vec!["alice"], Some(42))).await.unwrap();  // 1000s later

      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 2);
      assert!(rows[0].id > rows[1].id, "DESC order");
      assert_eq!(rows[0].count, 1);
      assert_eq!(rows[1].count, 1);
  }

  #[tokio::test]
  async fn emit_skips_recipient_with_stream_disabled_unless_actor() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      // Bob has opted out of file_updated stream entries.
      settings.set("bob", "file_updated", /*stream*/ false).await.unwrap();
      // Alice (the actor) also opted out — but the actor row is exempt.
      settings.set("alice", "file_updated", false).await.unwrap();

      let activity = Activity::new(pool.clone(), settings, 600);
      activity.emit(event(1_000, vec!["alice", "bob"], Some(42))).await.unwrap();

      assert_eq!(activity.list("alice", None, 100).await.unwrap().len(), 1,
                 "actor row is exempt from opt-out");
      assert_eq!(activity.list("bob",   None, 100).await.unwrap().len(), 0,
                 "non-actor opt-out skips the row");
  }

  #[tokio::test]
  async fn list_paginates_by_id_descending() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, 0); // disable coalesce

      for i in 0..5 {
          activity.emit(event(1_000 + i * 1000, vec!["alice"], Some(100 + i))).await.unwrap();
      }
      let page1 = activity.list("alice", None, 2).await.unwrap();
      assert_eq!(page1.len(), 2);
      let cursor = page1.last().unwrap().id;
      let page2 = activity.list("alice", Some(cursor), 2).await.unwrap();
      assert_eq!(page2.len(), 2);
      assert!(page2[0].id < cursor, "page2 starts strictly before cursor");
  }

  #[tokio::test]
  async fn sweep_expired_deletes_old_rows() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, 0); // disable coalesce

      activity.emit(event(1_000, vec!["alice"], Some(1))).await.unwrap();  // old
      activity.emit(event(9_000, vec!["alice"], Some(2))).await.unwrap();  // new
      let n = activity.sweep_expired(5_000).await.unwrap();
      assert_eq!(n, 1);
      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].object_id, Some(2));
  }

  #[tokio::test]
  async fn settings_get_all_returns_set_values() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      settings.set("alice", "file_updated",  false).await.unwrap();
      settings.set("alice", "share_created", true).await.unwrap();
      let mut rows = settings.get_all_for_user("alice").await.unwrap();
      rows.sort_by(|a, b| a.event_type.cmp(&b.event_type));
      assert_eq!(rows.len(), 2);
      assert_eq!(rows[0].event_type, "file_updated");
      assert_eq!(rows[0].stream, false);
      assert_eq!(rows[1].event_type, "share_created");
      assert_eq!(rows[1].stream, true);
  }

  #[tokio::test]
  async fn settings_upsert_updates_existing() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      settings.set("alice", "file_updated", true).await.unwrap();
      settings.set("alice", "file_updated", false).await.unwrap();
      let s = settings.stream_enabled("alice", "file_updated").await.unwrap();
      assert_eq!(s, false);
  }

  #[tokio::test]
  async fn settings_default_true_when_no_row() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let s = settings.stream_enabled("nobody_set_anything", "file_updated").await.unwrap();
      assert_eq!(s, true);
  }

  #[tokio::test]
  async fn emit_with_object_id_none_coalesces_correctly() {
      let (pool, _d) = setup().await;
      let settings = ActivitySettings::new(pool.clone());
      let activity = Activity::new(pool.clone(), settings, 600);
      activity.emit(event(1_000, vec!["alice"], None)).await.unwrap();
      activity.emit(event(1_100, vec!["alice"], None)).await.unwrap();
      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].count, 2);
  }
  ```

- [ ] **Step 2: Run the test (RED)**

  ```bash
  cargo test -p crabcloud-activity --test activity_e2e
  ```

  Expected: compile failures + assertion failures across all tests.

- [ ] **Step 3: Implement `src/service.rs`**

  Replace the stub from A2 with the full service. Follow the `crabcloud-versions/src/service.rs` pattern for per-dialect row decode.

  ```rust
  //! `Activity` — emit/list/sweep + impl ActivityEmitter.
  //!
  //! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
  //! Coalesce: same (recipient, actor, event_type, object_id) within
  //! `coalesce_window_secs` UPDATEs `count + last_seen_at`; otherwise
  //! INSERTs a fresh row.

  use crate::emitter::ActivityEmitter;
  use crate::error::{ActivityEmitError, ActivityError};
  use crate::settings::ActivitySettings;
  use crate::sql;
  use crate::types::{ActivityEvent, ActivityRow};
  use async_trait::async_trait;
  use crabcloud_db::DbPool;
  use sqlx::Row as _;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct Activity {
      pool: Arc<DbPool>,
      settings: ActivitySettings,
      coalesce_window_secs: i64,
  }

  impl Activity {
      pub fn new(pool: Arc<DbPool>, settings: ActivitySettings, coalesce_window_secs: i64) -> Self {
          Self { pool, settings, coalesce_window_secs }
      }

      pub async fn list(
          &self,
          affected_user: &str,
          since: Option<i64>,
          limit: i64,
      ) -> Result<Vec<ActivityRow>, ActivityError> {
          let since_v = since.unwrap_or(0);
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::LIST_QM)
                  .bind(affected_user).bind(since_v).bind(since_v).bind(limit)
                  .fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::LIST_QM)
                  .bind(affected_user).bind(since_v).bind(since_v).bind(limit)
                  .fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::LIST_PG)
                  .bind(affected_user).bind(since_v).bind(limit)
                  .fetch_all(p).await?,
          };
          rows.iter().map(row_to_activity).collect()
      }

      pub async fn sweep_expired(&self, cutoff: i64) -> Result<u64, ActivityError> {
          let n = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::DELETE_EXPIRED_QM).bind(cutoff).execute(p).await?.rows_affected(),
              DbPool::MySql(p) => sqlx::query(sql::DELETE_EXPIRED_QM).bind(cutoff).execute(p).await?.rows_affected(),
              DbPool::Postgres(p) => sqlx::query(sql::DELETE_EXPIRED_PG).bind(cutoff).execute(p).await?.rows_affected(),
          };
          Ok(n)
      }

      async fn coalesce_probe(
          &self,
          affected_user: &str,
          actor: &str,
          event_type: &str,
          object_id: Option<i64>,
          cutoff_ts: i64,
      ) -> Result<Option<i64>, ActivityError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::COALESCE_PROBE_QM)
                  .bind(affected_user).bind(actor).bind(event_type)
                  .bind(object_id).bind(object_id).bind(cutoff_ts)
                  .fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::COALESCE_PROBE_QM)
                  .bind(affected_user).bind(actor).bind(event_type)
                  .bind(object_id).bind(object_id).bind(cutoff_ts)
                  .fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::COALESCE_PROBE_PG)
                  .bind(affected_user).bind(actor).bind(event_type)
                  .bind(object_id).bind(cutoff_ts)
                  .fetch_optional(p).await?,
          };
          Ok(row.map(|r| r.try_get::<i64, _>("id")).transpose()?)
      }

      async fn coalesce_update(
          &self,
          id: i64,
          last_seen_at: i64,
          subject_params_json: &str,
      ) -> Result<(), ActivityError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => { sqlx::query(sql::COALESCE_UPDATE_QM).bind(last_seen_at).bind(subject_params_json).bind(id).execute(p).await?; }
              DbPool::MySql(p) => { sqlx::query(sql::COALESCE_UPDATE_QM).bind(last_seen_at).bind(subject_params_json).bind(id).execute(p).await?; }
              DbPool::Postgres(p) => { sqlx::query(sql::COALESCE_UPDATE_PG).bind(last_seen_at).bind(subject_params_json).bind(id).execute(p).await?; }
          }
          Ok(())
      }

      async fn insert_row(
          &self,
          affected_user: &str,
          actor: &str,
          event_type: &str,
          subject_id: &str,
          subject_params_json: &str,
          object_type: &str,
          object_id: Option<i64>,
          occurred_at: i64,
      ) -> Result<i64, ActivityError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(affected_user).bind(actor).bind(event_type)
                      .bind(subject_id).bind(subject_params_json)
                      .bind(object_type).bind(object_id)
                      .bind(occurred_at).bind(occurred_at)
                      .execute(p).await?;
                  Ok(r.last_insert_rowid())
              }
              DbPool::MySql(p) => {
                  let r = sqlx::query(sql::INSERT_QM)
                      .bind(affected_user).bind(actor).bind(event_type)
                      .bind(subject_id).bind(subject_params_json)
                      .bind(object_type).bind(object_id)
                      .bind(occurred_at).bind(occurred_at)
                      .execute(p).await?;
                  Ok(r.last_insert_id() as i64)
              }
              DbPool::Postgres(p) => {
                  let row = sqlx::query(sql::INSERT_PG)
                      .bind(affected_user).bind(actor).bind(event_type)
                      .bind(subject_id).bind(subject_params_json)
                      .bind(object_type).bind(object_id)
                      .bind(occurred_at).bind(occurred_at)
                      .fetch_one(p).await?;
                  Ok(row.try_get::<i64, _>("id")?)
              }
          }
      }
  }

  #[async_trait]
  impl ActivityEmitter for Activity {
      async fn emit(&self, event: ActivityEvent) -> Result<(), ActivityEmitError> {
          // De-dupe recipients in case a caller composes the list naively.
          let mut seen = std::collections::HashSet::new();
          let unique_recipients: Vec<_> = event
              .recipients
              .into_iter()
              .filter(|u| seen.insert(u.as_str().to_string()))
              .collect();

          let subject_params_json = serde_json::to_string(&event.subject_params)
              .map_err(|e| ActivityEmitError(format!("subject_params serialize: {e}")))?;
          let event_type_str = event.event_type.as_str();
          let object_type_str = event.object_type.as_str();
          let cutoff_ts = event.occurred_at - self.coalesce_window_secs;

          for recipient in unique_recipients {
              let is_actor = recipient.as_str() == event.actor;
              if !is_actor {
                  // Stream opt-out check (actor row exempt).
                  let stream = self
                      .settings
                      .stream_enabled(recipient.as_str(), event_type_str)
                      .await
                      .map_err(ActivityEmitError::from)?;
                  if !stream {
                      continue;
                  }
              }

              let subject_id = if is_actor {
                  &event.subject_id_actor
              } else {
                  &event.subject_id_recipient
              };

              if self.coalesce_window_secs > 0 {
                  if let Some(id) = self
                      .coalesce_probe(
                          recipient.as_str(),
                          &event.actor,
                          event_type_str,
                          event.object_id,
                          cutoff_ts,
                      )
                      .await
                      .map_err(ActivityEmitError::from)?
                  {
                      self.coalesce_update(id, event.occurred_at, &subject_params_json)
                          .await
                          .map_err(ActivityEmitError::from)?;
                      continue;
                  }
              }

              self.insert_row(
                  recipient.as_str(),
                  &event.actor,
                  event_type_str,
                  subject_id,
                  &subject_params_json,
                  object_type_str,
                  event.object_id,
                  event.occurred_at,
              )
              .await
              .map_err(ActivityEmitError::from)?;
          }

          Ok(())
      }
  }

  fn row_to_activity<R>(r: &R) -> Result<ActivityRow, ActivityError>
  where
      R: sqlx::Row,
      i64: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      String: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      Option<i64>: sqlx::Type<R::Database>
          + for<'q> sqlx::Decode<'q, R::Database>,
      for<'a> &'a str: sqlx::ColumnIndex<R>,
  {
      let subject_params_str: String = r.try_get("subject_params")?;
      let subject_params: serde_json::Value = serde_json::from_str(&subject_params_str)
          .unwrap_or(serde_json::Value::Null);
      Ok(ActivityRow {
          id: r.try_get("id")?,
          affected_user: r.try_get("affected_user")?,
          actor: r.try_get("actor")?,
          event_type: r.try_get("event_type")?,
          subject_id: r.try_get("subject_id")?,
          subject_params,
          object_type: r.try_get("object_type")?,
          object_id: r.try_get("object_id")?,
          occurred_at: r.try_get("occurred_at")?,
          last_seen_at: r.try_get("last_seen_at")?,
          count: r.try_get::<i32, _>("count")?,
      })
  }
  ```

  **Note on `row_to_activity`:** if the generic trait bounds prove unwieldy across the three dialects (sqlx 0.8 can be finicky), fall back to the per-dialect inline pattern used in `crates/crabcloud-versions/src/service.rs` and `crates/crabcloud-trash/src/service.rs` (look for `row_from_sqlite` / `row_from_mysql` / `row_from_postgres` helpers and mirror their exact shape).

- [ ] **Step 4: Implement `src/settings.rs`**

  ```rust
  //! `ActivitySettings` — per-user-per-event stream toggle storage.
  //!
  //! Default `stream = true` when no row exists. Get is one row;
  //! `get_all_for_user` returns every set toggle. Set is an upsert.

  use crate::error::ActivityError;
  use crate::sql;
  use crate::types::ActivitySetting;
  use crabcloud_db::DbPool;
  use sqlx::Row as _;
  use std::sync::Arc;

  #[derive(Clone)]
  pub struct ActivitySettings {
      pool: Arc<DbPool>,
  }

  impl ActivitySettings {
      pub fn new(pool: Arc<DbPool>) -> Self {
          Self { pool }
      }

      /// Get the toggle for one (user, event_type). Missing row → true.
      pub async fn stream_enabled(&self, user_id: &str, event_type: &str) -> Result<bool, ActivityError> {
          let row = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::SETTINGS_GET_QM)
                  .bind(user_id).bind(event_type).fetch_optional(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::SETTINGS_GET_QM)
                  .bind(user_id).bind(event_type).fetch_optional(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::SETTINGS_GET_PG)
                  .bind(user_id).bind(event_type).fetch_optional(p).await?,
          };
          match row {
              Some(r) => Ok(r.try_get::<bool, _>("stream")?),
              None => Ok(true),
          }
      }

      pub async fn get_all_for_user(&self, user_id: &str) -> Result<Vec<ActivitySetting>, ActivityError> {
          let rows = match self.pool.as_ref() {
              DbPool::Sqlite(p) => sqlx::query(sql::SETTINGS_GET_ALL_QM).bind(user_id).fetch_all(p).await?,
              DbPool::MySql(p) => sqlx::query(sql::SETTINGS_GET_ALL_QM).bind(user_id).fetch_all(p).await?,
              DbPool::Postgres(p) => sqlx::query(sql::SETTINGS_GET_ALL_PG).bind(user_id).fetch_all(p).await?,
          };
          rows.iter().map(|r| -> Result<_, ActivityError> {
              Ok(ActivitySetting {
                  event_type: r.try_get("event_type")?,
                  stream: r.try_get::<bool, _>("stream")?,
              })
          }).collect()
      }

      pub async fn set(&self, user_id: &str, event_type: &str, stream: bool) -> Result<(), ActivityError> {
          match self.pool.as_ref() {
              DbPool::Sqlite(p) => {
                  sqlx::query(sql::SETTINGS_UPSERT_SQLITE)
                      .bind(user_id).bind(event_type).bind(stream)
                      .execute(p).await?;
              }
              DbPool::MySql(p) => {
                  sqlx::query(sql::SETTINGS_UPSERT_MYSQL)
                      .bind(user_id).bind(event_type).bind(stream)
                      .execute(p).await?;
              }
              DbPool::Postgres(p) => {
                  sqlx::query(sql::SETTINGS_UPSERT_PG)
                      .bind(user_id).bind(event_type).bind(stream)
                      .execute(p).await?;
              }
          }
          Ok(())
      }
  }
  ```

  **Note on the get_all_for_user iterator pattern:** if the generic trait bounds in `row_to_activity` are dropped in favor of per-dialect inline, do the same here — match on the pool, decode rows in each arm.

- [ ] **Step 5: Iterate against the e2e until GREEN**

  ```bash
  cargo test -p crabcloud-activity --test activity_e2e
  ```

  All 10 tests must pass. Likely sticking points:
  - sqlx generic trait bounds for `row_to_activity` / `get_all_for_user`. Drop to per-dialect inline if needed (mirror `crates/crabcloud-versions/src/service.rs`).
  - The `(? = 0 OR id < ?)` cursor pattern for sqlite/mysql binds `since` twice; postgres binds `$2` twice as the same param.
  - `last_insert_rowid()` (sqlite) is the right shape; mysql uses `last_insert_id()` cast to i64; pg uses `RETURNING id`.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-activity/src/service.rs \
          crates/crabcloud-activity/src/settings.rs \
          crates/crabcloud-activity/tests/activity_e2e.rs
  git commit -m "activity: Activity service + ActivitySettings (emit / list / coalesce / sweep / settings)"
  ```

### Task A5: Subject template renderer

**Files:**
- Modify: `crates/crabcloud-activity/src/subjects.rs`

- [ ] **Step 1: Write the renderer + template map**

  ```rust
  //! Subject template rendering.
  //!
  //! Each `subject_id` maps to an English template with `{key}` placeholders.
  //! Unknown subject_ids fall back to the id verbatim with a tracing::warn.

  use serde_json::Value;

  /// Render a subject_id + params into an English string.
  pub fn render_subject(subject_id: &str, params: &Value) -> String {
      let template = match template_for(subject_id) {
          Some(t) => t,
          None => {
              tracing::warn!(subject_id, "activity: subject_id has no template; returning verbatim");
              return subject_id.to_string();
          }
      };
      interpolate(template, params)
  }

  fn template_for(subject_id: &str) -> Option<&'static str> {
      Some(match subject_id {
          "file_created_by"      => "{actor} created {file}",
          "file_created_you"     => "You created {file}",
          "file_updated_by"      => "{actor} updated {file}",
          "file_updated_you"     => "You updated {file}",
          "file_deleted_by"      => "{actor} deleted {file}",
          "file_deleted_you"     => "You deleted {file}",
          "file_renamed_by"      => "{actor} renamed {old} to {file}",
          "file_renamed_you"     => "You renamed {old} to {file}",
          "file_restored_by"     => "{actor} restored {file} from the trash",
          "file_restored_you"    => "You restored {file} from the trash",
          "share_created_by"     => "{actor} shared {file} with you",
          "share_created_you"    => "You shared {file} with {recipient}",
          "share_deleted_by"     => "{actor} unshared {file} from you",
          "share_deleted_you"    => "You unshared {file} from {recipient}",
          "version_restored_by"  => "{actor} restored a previous version of {file}",
          "version_restored_you" => "You restored a previous version of {file}",
          // Public-link variants (actor = "")
          "file_created_link"    => "Someone created {file} via a shared link",
          "file_updated_link"    => "Someone updated {file} via a shared link",
          "file_deleted_link"    => "Someone deleted {file} via a shared link",
          _ => return None,
      })
  }

  /// Replace every `{key}` placeholder with the corresponding string from
  /// `params` (JSON object). Missing keys → empty string.
  fn interpolate(template: &str, params: &Value) -> String {
      let mut out = String::with_capacity(template.len());
      let mut chars = template.chars().peekable();
      while let Some(ch) = chars.next() {
          if ch != '{' {
              out.push(ch);
              continue;
          }
          // Collect the key until '}'.
          let mut key = String::new();
          while let Some(&c) = chars.peek() {
              if c == '}' {
                  chars.next();
                  break;
              }
              key.push(c);
              chars.next();
          }
          let value = params.get(&key).and_then(|v| v.as_str()).unwrap_or("");
          out.push_str(value);
      }
      out
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use serde_json::json;

      #[test]
      fn interpolate_simple() {
          assert_eq!(interpolate("hello {name}", &json!({"name": "alice"})), "hello alice");
      }

      #[test]
      fn interpolate_missing_key_is_empty() {
          assert_eq!(interpolate("{a} {b}", &json!({"a": "x"})), "x ");
      }

      #[test]
      fn render_known_subject() {
          assert_eq!(
              render_subject("file_updated_by", &json!({"actor": "alice", "file": "x.txt"})),
              "alice updated x.txt"
          );
      }

      #[test]
      fn render_unknown_returns_verbatim() {
          let r = render_subject("unknown_template", &json!({}));
          assert_eq!(r, "unknown_template");
      }
  }
  ```

- [ ] **Step 2: Run tests**

  ```bash
  cargo test -p crabcloud-activity subjects
  ```

  Expected: 4 unit tests pass.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/crabcloud-activity/src/subjects.rs
  git commit -m "activity: subject template renderer + English templates"
  ```

### Task A6: `ActivitySweeper` background task

**Files:**
- Create: `crates/crabcloud-core/src/activity_sweeper.rs`
- Modify: `crates/crabcloud-core/src/lib.rs`
- Modify: `crates/crabcloud-core/Cargo.toml`

- [ ] **Step 1: Add `crabcloud-activity` dep to `crabcloud-core`**

  In `crates/crabcloud-core/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

- [ ] **Step 2: Write `src/activity_sweeper.rs`**

  Mirror `versions_sweeper.rs` shape.

  ```rust
  //! Background task: daily age-based sweep of `oc_activity`. Mirrors the
  //! `VersionsSweeper` / `TrashSweeper` shape: cooperative shutdown via
  //! `Arc<Notify>`, `sweep_once()` for sync test drive.

  use crabcloud_activity::Activity;
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::sync::Notify;

  const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

  #[derive(Clone)]
  pub struct ActivitySweeper {
      activity: Arc<Activity>,
      retention: chrono::Duration,
      shutdown: Arc<Notify>,
  }

  impl ActivitySweeper {
      pub fn new(activity: Arc<Activity>, retention_days: u32) -> (Self, Arc<Notify>) {
          let shutdown = Arc::new(Notify::new());
          (
              Self {
                  activity,
                  retention: chrono::Duration::seconds(retention_days as i64 * 86_400),
                  shutdown: shutdown.clone(),
              },
              shutdown,
          )
      }

      pub async fn run(self) {
          loop {
              if let Err(e) = self.sweep_once().await {
                  tracing::warn!(error = %e, "activity sweeper: sweep_once failed");
              }
              tokio::select! {
                  _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                  _ = self.shutdown.notified() => return,
              }
          }
      }

      pub async fn sweep_once(&self) -> Result<u64, crabcloud_activity::ActivityError> {
          let secs = self.retention.num_seconds();
          if secs <= 0 {
              return Ok(0);
          }
          let cutoff = chrono::Utc::now().timestamp() - secs;
          self.activity.sweep_expired(cutoff).await
      }
  }
  ```

- [ ] **Step 3: Wire into `lib.rs`**

  ```rust
  mod activity_sweeper;
  pub use activity_sweeper::ActivitySweeper;
  ```

- [ ] **Step 4: Add unit test inside `activity_sweeper.rs`**

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crabcloud_activity::{Activity, ActivitySettings};
      use crabcloud_config::test_support::minimal_sqlite_config;
      use crabcloud_db::{core_set, DbPool, MigrationRunner};
      use tempfile::TempDir;

      async fn setup_activity() -> (Arc<Activity>, TempDir) {
          let db = TempDir::new().unwrap();
          let cfg = minimal_sqlite_config(db.path().join("t.db"));
          let pool = DbPool::connect(&cfg).await.unwrap();
          let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
          runner.register(core_set());
          runner.run().await.unwrap();
          let pool = Arc::new(pool);
          let settings = ActivitySettings::new(pool.clone());
          (Arc::new(Activity::new(pool, settings, 0)), db)
      }

      #[tokio::test]
      async fn sweep_once_disabled_returns_zero() {
          let (activity, _d) = setup_activity().await;
          let (sw, _) = ActivitySweeper::new(activity, /*retention_days*/ 0);
          assert_eq!(sw.sweep_once().await.unwrap(), 0);
      }
  }
  ```

- [ ] **Step 5: Build + test**

  ```bash
  cargo build -p crabcloud-core
  cargo test -p crabcloud-core activity_sweeper
  ```

  Expected: passes.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-core/Cargo.toml crates/crabcloud-core/src/activity_sweeper.rs crates/crabcloud-core/src/lib.rs
  git commit -m "activity: ActivitySweeper background task (daily age-based purge)"
  ```

### Task A7: Config knobs

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`
- Modify: `crates/crabcloud-config/src/test_support.rs`

- [ ] **Step 1: Add fields + defaults**

  In `types.rs`, add after the `versions_*` fields on `FileConfig`:

  ```rust
  /// How many days to keep activity feed rows before the daily sweeper
  /// purges them. `0` disables sweeping (compliance retain-forever
  /// escape hatch). Default 365 (matches Nextcloud).
  #[serde(default = "default_activity_retention_days")]
  pub activity_retention_days: u32,

  /// Coalesce window for the activity feed. Successive same-
  /// (recipient, actor, event_type, object_id) emits within this many
  /// seconds bump the existing row's count + last_seen_at instead of
  /// inserting. `0` disables coalesce. Default 600 (10 minutes).
  #[serde(default = "default_activity_coalesce_window_secs")]
  pub activity_coalesce_window_secs: u32,
  ```

  And defaults at the bottom:

  ```rust
  fn default_activity_retention_days() -> u32 { 365 }
  fn default_activity_coalesce_window_secs() -> u32 { 600 }
  ```

  Update `FileConfig::default()` to include both.

- [ ] **Step 2: Update `test_support.rs::minimal_sqlite_config`**

  Add:
  ```rust
  activity_retention_days: 365,
  activity_coalesce_window_secs: 600,
  ```

- [ ] **Step 3: Build + test**

  ```bash
  cargo test -p crabcloud-config
  ```

  Expected: passes.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/crabcloud-config/src/types.rs crates/crabcloud-config/src/test_support.rs
  git commit -m "activity: activity_retention_days + activity_coalesce_window_secs config knobs"
  ```

### Task A8: Wire `Activity` + `ActivitySettings` + sweeper into `AppState`

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Add fields**

  At the top: `use crabcloud_activity::{Activity, ActivitySettings};`

  Inside `AppState`:
  ```rust
  /// Activity service. Cheap to clone.
  pub activity: Arc<crabcloud_activity::Activity>,
  /// Activity settings (per-user-per-event stream toggles).
  pub activity_settings: crabcloud_activity::ActivitySettings,
  /// Activity sweeper shutdown handle. Always present; spawned
  /// unconditionally in `AppStateBuilder::build`.
  pub activity_sweeper_shutdown: Arc<tokio::sync::Notify>,
  ```

- [ ] **Step 2: Construct in `AppStateBuilder::build`**

  Before the existing trash construction block (so trash + versions can be wired to use the activity emitter — Task A9 etc.):

  ```rust
  let activity_settings = crabcloud_activity::ActivitySettings::new(Arc::new(pool.clone()));
  let activity = Arc::new(crabcloud_activity::Activity::new(
      Arc::new(pool.clone()),
      activity_settings.clone(),
      self.config.activity_coalesce_window_secs as i64,
  ));
  let (activity_sweeper, activity_sweeper_shutdown) =
      crate::activity_sweeper::ActivitySweeper::new(
          activity.clone(),
          self.config.activity_retention_days,
      );
  std::mem::drop(tokio::spawn(async move { activity_sweeper.run().await }));
  ```

- [ ] **Step 3: Add to the AppState literal**

  ```rust
  activity,
  activity_settings,
  activity_sweeper_shutdown,
  ```

- [ ] **Step 4: Build + state tests**

  ```bash
  cargo build --workspace
  cargo test -p crabcloud-core state
  ```

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-core/src/state.rs
  git commit -m "activity: wire Activity + ActivitySettings + sweeper into AppState"
  ```

### Task A9: Emit hook in `Versions::restore`

**Files:**
- Modify: `crates/crabcloud-versions/Cargo.toml`
- Modify: `crates/crabcloud-versions/src/service.rs`
- Modify: `crates/crabcloud-core/src/state.rs` (pass `activity` into `Versions::new`)

Start with versions because it's the simplest emitter (one recipient: the owner).

- [ ] **Step 1: Add dep**

  In `crates/crabcloud-versions/Cargo.toml` `[dependencies]`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

- [ ] **Step 2: Add field to `Versions`**

  ```rust
  pub struct Versions {
      pool: Arc<DbPool>,
      datadir: PathBuf,
      activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
  }
  ```

  Update `Versions::new`:
  ```rust
  pub fn new(
      pool: Arc<DbPool>,
      datadir: PathBuf,
      activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
  ) -> Self {
      Self { pool, datadir, activity }
  }
  ```

- [ ] **Step 3: Emit in `restore`**

  After the existing successful body of `Versions::restore`, before the final `Ok(())`:
  ```rust
  // Best-effort activity emit. Failure does not roll back the restore.
  let event = crabcloud_activity::ActivityEvent {
      actor: uid.to_string(),
      event_type: crabcloud_activity::EventType::VersionRestored,
      subject_id_actor: "version_restored_you".into(),
      subject_id_recipient: "version_restored_by".into(),
      subject_params: serde_json::json!({
          "actor": uid,
          "file": std::path::Path::new(&entry.path)
              .file_name()
              .and_then(|s| s.to_str())
              .unwrap_or(&entry.path),
      }),
      object_type: crabcloud_activity::ObjectType::Version,
      object_id: Some(entry.id),
      recipients: vec![crabcloud_users::UserId::new(uid)
          .map_err(|e| VersionsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?],
      occurred_at: chrono::Utc::now().timestamp(),
  };
  if let Err(e) = self.activity.emit(event).await {
      tracing::warn!(error = %e, uid, version_id = entry.id, "versions: activity emit failed");
  }
  ```

- [ ] **Step 4: Update `AppState` to pass activity into Versions**

  In `state.rs`, the `Versions::new` call:
  ```rust
  let versions = Arc::new(crabcloud_versions::Versions::new(
      Arc::new(pool.clone()),
      self.config.datadirectory.clone(),
      activity.clone(),  // NEW
  ));
  ```

- [ ] **Step 5: Update test fixtures that build `Versions::new` directly**

  Grep `Versions::new(` and update each call site. For tests that don't care about activity, pass `Arc::new(crabcloud_activity::NoopEmitter) as Arc<dyn crabcloud_activity::ActivityEmitter>`.

- [ ] **Step 6: Add an integration test**

  In `crates/crabcloud-versions/tests/versions_e2e.rs`, add:
  ```rust
  #[tokio::test]
  async fn restore_emits_activity_for_owner() {
      // Build a real Activity (not Noop) so the emit is observable.
      let (pool, datadir, _d, _dd) = setup().await;
      let settings = crabcloud_activity::ActivitySettings::new(pool.clone());
      let activity = std::sync::Arc::new(crabcloud_activity::Activity::new(pool.clone(), settings, 0));
      let versions = crabcloud_versions::Versions::new(
          pool.clone(), datadir.clone(),
          activity.clone() as std::sync::Arc<dyn crabcloud_activity::ActivityEmitter>,
      );
      write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
      let id = versions
          .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
          .await.unwrap().expect("snapshot");
      write_user_file(&datadir, "alice", "/x.txt", b"v2").await;
      versions.restore("alice", id, 8, 2_000, 2, 1024).await.unwrap();

      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].event_type, "version_restored");
      assert_eq!(rows[0].actor, "alice");
      assert_eq!(rows[0].subject_id, "version_restored_you");
  }
  ```

- [ ] **Step 7: Run + commit**

  ```bash
  cargo test -p crabcloud-versions --test versions_e2e
  cargo test --workspace
  git add crates/crabcloud-versions/ crates/crabcloud-core/src/state.rs
  git commit -m "activity: emit version_restored from Versions::restore"
  ```

### Task A10: Emit hooks in `Trash::restore`

**Files:**
- Modify: `crates/crabcloud-trash/Cargo.toml`
- Modify: `crates/crabcloud-trash/src/service.rs`
- Modify: `crates/crabcloud-core/src/state.rs` (pass `activity` into `Trash::new`)

- [ ] **Step 1: Add dep + field**

  In `crates/crabcloud-trash/Cargo.toml`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

  In `Trash`:
  ```rust
  pub struct Trash {
      pool: Arc<DbPool>,
      datadir: PathBuf,
      versions: Arc<crabcloud_versions::Versions>,
      activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
  }
  ```

  Update `Trash::new` signature and the AppState call site.

- [ ] **Step 2: Emit in `restore`**

  After the existing successful body of `Trash::restore`, before the final `Ok(RestoredTo { ... })`:

  ```rust
  let event = crabcloud_activity::ActivityEvent {
      actor: uid.to_string(),
      event_type: crabcloud_activity::EventType::FileRestored,
      subject_id_actor: "file_restored_you".into(),
      subject_id_recipient: "file_restored_by".into(),
      subject_params: serde_json::json!({
          "actor": uid,
          "file": restored_to.path.clone(),
      }),
      object_type: crabcloud_activity::ObjectType::File,
      object_id: entry.fileid_legacy,
      recipients: vec![crabcloud_users::UserId::new(uid).map_err(|e| TrashError::Trash(e.to_string()))?],
      occurred_at: chrono::Utc::now().timestamp(),
  };
  if let Err(e) = self.activity.emit(event).await {
      tracing::warn!(error = %e, uid, "trash: activity emit failed");
  }
  ```

- [ ] **Step 3: Update `AppState` to pass activity into Trash**

  ```rust
  let trash = Arc::new(crabcloud_trash::Trash::new(
      Arc::new(pool.clone()),
      self.config.datadirectory.clone(),
      versions.clone(),
      activity.clone(),  // NEW
  ));
  ```

- [ ] **Step 4: Update test fixtures**

  Grep `Trash::new(` and pass `Arc::new(NoopEmitter)` in test sites.

- [ ] **Step 5: Add an integration test**

  In `crates/crabcloud-trash/tests/trash_e2e.rs`:
  ```rust
  #[tokio::test]
  async fn restore_emits_file_restored() {
      let (pool, datadir, _d, _dd) = setup().await;
      let settings = crabcloud_activity::ActivitySettings::new(pool.clone());
      let activity = std::sync::Arc::new(crabcloud_activity::Activity::new(pool.clone(), settings, 0));
      let versions = std::sync::Arc::new(crabcloud_versions::Versions::new(
          pool.clone(), datadir.clone(),
          std::sync::Arc::new(crabcloud_activity::NoopEmitter) as _,
      ));
      let trash = Trash::new(pool.clone(), datadir.clone(), versions, activity.clone() as _);

      write_user_file(&datadir, "alice", "/x.txt", b"hi").await;
      let id = trash.soft_delete("alice", "/x.txt", TrashType::File, Some(100)).await.unwrap();
      trash.restore("alice", id, None).await.unwrap();

      let rows = activity.list("alice", None, 100).await.unwrap();
      assert_eq!(rows.len(), 1);
      assert_eq!(rows[0].event_type, "file_restored");
  }
  ```

- [ ] **Step 6: Run + commit**

  ```bash
  cargo test -p crabcloud-trash --test trash_e2e
  cargo test --workspace
  git add crates/crabcloud-trash/ crates/crabcloud-core/src/state.rs
  git commit -m "activity: emit file_restored from Trash::restore"
  ```

### Task A11: Emit hooks in `Shares::create` and `Shares::delete`

**Files:**
- Modify: `crates/crabcloud-sharing/Cargo.toml`
- Modify: `crates/crabcloud-sharing/src/service.rs` (or wherever `Shares::create` / `Shares::delete` live — grep `pub async fn create` / `pub async fn delete` inside the sharing crate)
- Modify: `crates/crabcloud-core/src/state.rs` (pass `activity` into `SharesConfig`)

- [ ] **Step 1: Add dep + field**

  In `crates/crabcloud-sharing/Cargo.toml`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

  Find `SharesConfig` (or the equivalent constructor input — SP12 polish C introduced this struct). Add:
  ```rust
  pub activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
  ```

  Pass through to the `Shares` struct.

- [ ] **Step 2: Emit in `create`**

  After a successful share row insert, compose recipients:
  - User share (`share_type=0`): recipients = [actor, share_with]
  - Group share (`share_type=1`): recipients = [actor, ...group members resolved via `users.group_members(group_id)`]
  - Link share / Email share: recipients = [actor]

  Emit `share_created` with the resolved recipient list:
  ```rust
  let recipients = match req.share_type {
      ShareType::User => vec![req.requester.clone(), req.share_with_user_id()?],
      ShareType::Group => {
          let mut r = vec![req.requester.clone()];
          r.extend(self.users.group_members(&req.share_with_group_id()?).await.unwrap_or_default());
          r
      }
      ShareType::Link | ShareType::Email => vec![req.requester.clone()],
  };
  // De-dupe + map to UserId
  let mut seen = std::collections::HashSet::new();
  let recipients: Vec<_> = recipients
      .into_iter()
      .filter_map(|s| crabcloud_users::UserId::new(s).ok())
      .filter(|u| seen.insert(u.as_str().to_string()))
      .collect();

  let event = crabcloud_activity::ActivityEvent {
      actor: req.requester.clone(),
      event_type: crabcloud_activity::EventType::ShareCreated,
      subject_id_actor: "share_created_you".into(),
      subject_id_recipient: "share_created_by".into(),
      subject_params: serde_json::json!({
          "actor": req.requester,
          "file": req.path.clone(),
          "recipient": req.share_with.clone(),
      }),
      object_type: crabcloud_activity::ObjectType::Share,
      object_id: Some(share_id),
      recipients,
      occurred_at: chrono::Utc::now().timestamp(),
  };
  if let Err(e) = self.activity.emit(event).await {
      tracing::warn!(error = %e, share_id, "sharing: activity emit failed");
  }
  ```

  Adjust the field names (`req.share_with_user_id()` etc.) to the actual `CreateShareRequest` shape in the codebase. Read `crates/crabcloud-sharing/src/types.rs` first.

- [ ] **Step 3: Emit in `delete`**

  Symmetric to `create`. Resolve recipients from the share row's current state (read the row before deleting it so the recipients are still computable):
  ```rust
  let row = self.read_row(share_id).await?;  // existing or new helper
  // Then compute recipients from row.share_type + row.share_with the same way as create.
  // Emit ShareDeleted with subject_id_*_deleted_*.
  ```

- [ ] **Step 4: Update `AppState` to pass activity into `SharesConfig`**

  Add `activity: activity.clone(),` to the `SharesConfig { ... }` literal in `AppStateBuilder::build`.

- [ ] **Step 5: Update test fixtures**

  Grep `SharesConfig {` and `Shares::new(` in tests. For tests that don't care, pass `Arc::new(NoopEmitter)`.

- [ ] **Step 6: Add integration tests**

  In `crates/crabcloud-sharing/tests/`:
  - User share create emits to actor + target.
  - Group share create emits to actor + every group member.
  - Share delete emits to actor + target.

- [ ] **Step 7: Run + commit**

  ```bash
  cargo test -p crabcloud-sharing
  cargo test --workspace
  git add crates/crabcloud-sharing/ crates/crabcloud-core/src/state.rs
  git commit -m "activity: emit share_created + share_deleted from Shares::{create, delete}"
  ```

### Task A12: Emit hooks in `View::{write_file, delete, hard_delete, rename, rename_force_overwrite}`

**Files:**
- Modify: `crates/crabcloud-fs/Cargo.toml`
- Modify: `crates/crabcloud-fs/src/view.rs` (and any sibling files holding the write paths)

- [ ] **Step 1: Add dep + field**

  In `crates/crabcloud-fs/Cargo.toml`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

  In `View`, add an `activity: Arc<dyn crabcloud_activity::ActivityEmitter>` field. Extend `View::new` to take it. Update every `View::new(` call site (grep — same drill as the SP13 Batch A `VersionsHooks` ripple; 7-8 sites).

- [ ] **Step 2: Helper for resolving recipients**

  Inside `view.rs`, add a private async helper that, given a mount + path, resolves the recipient list:
  - Home mount: recipients = [self.uid] (actor only).
  - Share mount: recipients = [self.uid (the deleter/writer), mount.metadata.owner_uid]. Plus any other recipients of the share (if the share grants update permission to a group, fan-out to group members).

  Start with the actor + owner_uid case for share mounts. Group fan-out for shared-resource activity can come in a follow-up — flag it in a comment.

- [ ] **Step 3: Emit in `write_file`**

  After the successful write (so we don't emit on a failed write), but BEFORE returning `Ok`:

  ```rust
  let recipients = self.resolve_recipients(&mount, &path).await;
  let event_type = if pre_write_existed {
      crabcloud_activity::EventType::FileUpdated
  } else {
      crabcloud_activity::EventType::FileCreated
  };
  let event = crabcloud_activity::ActivityEvent {
      actor: self.uid.as_str().to_string(),
      event_type,
      subject_id_actor: if pre_write_existed { "file_updated_you" } else { "file_created_you" }.into(),
      subject_id_recipient: if pre_write_existed { "file_updated_by" } else { "file_created_by" }.into(),
      subject_params: serde_json::json!({
          "actor": self.uid.as_str(),
          "file": path.as_str(),
      }),
      object_type: crabcloud_activity::ObjectType::File,
      object_id: post_write_fileid,  // available from the filecache lookup the write just did
      recipients,
      occurred_at: chrono::Utc::now().timestamp(),
  };
  if let Err(e) = self.activity.emit(event).await {
      tracing::warn!(error = %e, uid = self.uid.as_str(), "view: activity emit failed (write_file)");
  }
  ```

- [ ] **Step 4: Emit in `delete` / `hard_delete`**

  Same shape. `FileDeleted` event. Recipients resolved the same way. For public-link DELETE → actor = "", subject_id_recipient = "file_deleted_link".

- [ ] **Step 5: Emit in `rename` / `rename_force_overwrite`**

  `FileRenamed` event. `subject_params` includes both `old` and `file` (the new name). Recipients resolved from the destination mount (where the file lives after the rename).

- [ ] **Step 6: Integration tests**

  In `crates/crabcloud-fs/tests/view_activity.rs` (new):
  - `View::write_file` emits `file_updated` to actor when overwriting; `file_created` on fresh write.
  - Shared-mount: Bob's write emits to Bob + Alice (owner).
  - `View::delete` emits `file_deleted` to actor (+ owner on share mount).
  - `View::rename` emits `file_renamed` with old + new params.
  - Public-link `View::hard_delete` emits with `actor = ""`.

- [ ] **Step 7: Run + commit**

  ```bash
  cargo test -p crabcloud-fs
  cargo test --workspace
  git add crates/crabcloud-fs/ crates/crabcloud-core/src/state.rs
  git commit -m "activity: emit file CRUD events from View"
  ```

### Task A13: Batch A pre-PR

- [ ] **Step 1: Pre-flight**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- [ ] **Step 2: Push and open PR**

  ```bash
  git push -u origin sp14/a-activity-crate
  gh pr create --title "sp14(a): crabcloud-activity crate + emit hooks + sweeper" \
    --body "Batch A of SP14 activity feed. New crabcloud-activity crate (Activity + ActivitySettings + ActivityEmitter trait), 0011_activity_and_settings migration, emit hooks in View (file CRUD) / Trash::restore / Shares::{create, delete} / Versions::restore, ActivitySweeper background task, two new config knobs (activity_retention_days default 365, activity_coalesce_window_secs default 600), AppState wiring. Spec: docs/superpowers/specs/2026-05-17-activity-feed-design.md."
  ```

---

# Batch B — OCS REST surface

**Branch:** `sp14/b-ocs` (off the merged Batch A master)

**Goal:** Add the Nextcloud-shape OCS endpoints at `/ocs/v2.php/apps/activity/api/v2/activity` for the feed list + settings.

### Task B1: OCS endpoints

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/activity.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`
- Modify: `crates/crabcloud-http/Cargo.toml`
- Create: `crates/crabcloud-http/tests/ocs_activity.rs`

- [ ] **Step 1: Add dep**

  In `crates/crabcloud-http/Cargo.toml`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

- [ ] **Step 2: Write `activity.rs`**

  Mirror `routes/ocs/files_versions.rs`. Use the shared `super::envelope::*` helpers (extracted in SP12 polish G).

  ```rust
  //! OCS endpoints for the activity feed.
  //!
  //! /ocs/v2.php/apps/activity/api/v2/
  //!   GET    /activity?since=<id>&limit=<N>          — list feed
  //!   GET    /activity/settings                       — list per-event toggles
  //!   PUT    /activity/settings                       — upsert one toggle

  use axum::extract::{Extension, Query, State};
  use axum::routing::{get, put};
  use axum::{Json, Router};
  use crabcloud_core::AppState;
  use crabcloud_activity::{render_subject, ActivityError, ActivityRow, ActivitySetting};
  use serde::{Deserialize, Serialize};

  pub fn router() -> Router<AppState> {
      Router::new()
          .route("/activity", get(list_handler))
          .route("/activity/settings", get(get_settings).put(put_setting))
  }

  #[derive(Deserialize, Default)]
  struct ListQuery {
      since: Option<i64>,
      limit: Option<i64>,
  }

  #[derive(Serialize)]
  pub struct ActivityRowDto {
      pub id: i64,
      pub actor: String,
      pub event_type: String,
      pub subject_id: String,
      pub subject_params: serde_json::Value,
      pub subject: String,
      pub object_type: String,
      pub object_id: Option<i64>,
      pub occurred_at: i64,
      pub last_seen_at: i64,
      pub count: i32,
  }

  impl From<ActivityRow> for ActivityRowDto {
      fn from(r: ActivityRow) -> Self {
          let subject = render_subject(&r.subject_id, &r.subject_params);
          Self {
              id: r.id, actor: r.actor, event_type: r.event_type,
              subject_id: r.subject_id, subject_params: r.subject_params,
              subject,
              object_type: r.object_type, object_id: r.object_id,
              occurred_at: r.occurred_at, last_seen_at: r.last_seen_at,
              count: r.count,
          }
      }
  }

  #[derive(Serialize)]
  pub struct ListResponseDto {
      pub items: Vec<ActivityRowDto>,
      pub next_since: Option<i64>,
  }

  async fn list_handler(
      State(state): State<AppState>,
      Extension(ctx): Extension<crate::middleware::auth::AuthContext>,
      Query(q): Query<ListQuery>,
  ) -> impl axum::response::IntoResponse {
      let limit = q.limit.unwrap_or(30).clamp(1, 100);
      let rows = match state.activity.list(&ctx.user_id, q.since, limit).await {
          Ok(r) => r,
          Err(e) => return from_activity_error(e),
      };
      let next_since = rows.last().map(|r| r.id);
      let items: Vec<ActivityRowDto> = rows.into_iter().map(ActivityRowDto::from).collect();
      super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({ "items": items, "next_since": next_since }))
  }

  async fn get_settings(
      State(state): State<AppState>,
      Extension(ctx): Extension<crate::middleware::auth::AuthContext>,
  ) -> impl axum::response::IntoResponse {
      let rows = match state.activity_settings.get_all_for_user(&ctx.user_id).await {
          Ok(r) => r,
          Err(e) => return from_activity_error(e),
      };
      super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({ "settings": rows }))
  }

  #[derive(Deserialize)]
  struct PutSettingBody {
      event_type: String,
      stream: bool,
  }

  async fn put_setting(
      State(state): State<AppState>,
      Extension(ctx): Extension<crate::middleware::auth::AuthContext>,
      Json(body): Json<PutSettingBody>,
  ) -> impl axum::response::IntoResponse {
      match state.activity_settings.set(&ctx.user_id, &body.event_type, body.stream).await {
          Ok(()) => super::envelope::ocs_envelope(200, "OK".into(), serde_json::json!({})),
          Err(e) => from_activity_error(e),
      }
  }

  fn from_activity_error(e: ActivityError) -> impl axum::response::IntoResponse {
      tracing::error!(error = %e, "activity OCS: unhandled error");
      super::envelope::ocs_envelope(500, e.to_string(), serde_json::json!({}))
  }
  ```

  Adjust the `Extension<crate::middleware::auth::AuthContext>` import path to match the actual extractor in the codebase (look at how `files_versions.rs` does it).

- [ ] **Step 3: Mount in `routes/ocs/mod.rs`**

  Add `pub mod activity;` and:
  ```rust
  .nest(
      "/v2.php/apps/activity/api/v2",
      activity::router().with_state(state.clone()),
  )
  ```

- [ ] **Step 4: E2E test**

  Create `crates/crabcloud-http/tests/ocs_activity.rs`. Cover:
  - GET empty list → 200 with `items: []`.
  - Seed 3 activity rows via `state.activity.emit(...)`; GET → 3 items, descending id.
  - GET with `?since=<id>&limit=1` → 1 item, id < since.
  - GET /settings → array; PUT /settings + re-GET → upsert visible.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-http --test ocs_activity
  git add crates/crabcloud-http/Cargo.toml crates/crabcloud-http/src/routes/ocs/ crates/crabcloud-http/tests/ocs_activity.rs
  git commit -m "activity ocs: /apps/activity/api/v2/{activity, settings} endpoints"
  ```

### Task B2: Batch B pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp14/b-ocs
  gh pr create --title "sp14(b): OCS /apps/activity/api/v2 endpoints" \
    --body "Batch B of SP14 activity feed."
  ```

---

# Batch C — Server fns

**Branch:** `sp14/c-server-fns` (off the merged Batch B master)

### Task C1: Server fns

**Files:**
- Create: `crates/crabcloud-app/src/server_fns/activity.rs`
- Modify: `crates/crabcloud-app/src/server_fns/mod.rs`
- Modify: `crates/crabcloud-app/Cargo.toml`
- Create: `crates/crabcloud-app/tests/server_fns_activity.rs`

- [ ] **Step 1: Add dep**

  In `crates/crabcloud-app/Cargo.toml`:
  ```toml
  crabcloud-activity = { workspace = true }
  ```

  Add `crabcloud-activity` to dev-deps too if the integration test needs it (it will).

- [ ] **Step 2: Write `activity.rs`**

  Mirror `server_fns/versions.rs` shape.

  ```rust
  //! Activity feed server fns.
  //!
  //! /api/files/activity/{list,settings,settings/put}

  use dioxus::prelude::*;
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ActivityRowDto {
      pub id: i64,
      pub actor: String,
      pub event_type: String,
      pub subject_id: String,
      pub subject_params: serde_json::Value,
      pub subject: String,
      pub object_type: String,
      pub object_id: Option<i64>,
      pub occurred_at: i64,
      pub last_seen_at: i64,
      pub count: i32,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ListActivityResponse {
      pub items: Vec<ActivityRowDto>,
      pub next_since: Option<i64>,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ActivitySettingDto {
      pub event_type: String,
      pub stream: bool,
  }

  #[server(endpoint = "/api/files/activity/list")]
  pub async fn list_activity(
      since: Option<i64>,
      limit: Option<i64>,
  ) -> Result<ListActivityResponse, ServerFnError> {
      use crate::server_fns::require_user;
      use crabcloud_activity::render_subject;
      let (state, ctx) = require_user()?;
      let limit = limit.unwrap_or(30).clamp(1, 100);
      let rows = state
          .activity
          .list(&ctx.user_id, since, limit)
          .await
          .map_err(|e| {
              tracing::error!(error = %e, "activity server fn list failed");
              ServerFnError::new(format!("activity list: {e}"))
          })?;
      let next_since = rows.last().map(|r| r.id);
      Ok(ListActivityResponse {
          items: rows.into_iter().map(|r| {
              let subject = render_subject(&r.subject_id, &r.subject_params);
              ActivityRowDto {
                  id: r.id, actor: r.actor, event_type: r.event_type,
                  subject_id: r.subject_id, subject_params: r.subject_params,
                  subject,
                  object_type: r.object_type, object_id: r.object_id,
                  occurred_at: r.occurred_at, last_seen_at: r.last_seen_at,
                  count: r.count,
              }
          }).collect(),
          next_since,
      })
  }

  #[server(endpoint = "/api/files/activity/settings")]
  pub async fn get_activity_settings() -> Result<Vec<ActivitySettingDto>, ServerFnError> {
      use crate::server_fns::require_user;
      let (state, ctx) = require_user()?;
      state
          .activity_settings
          .get_all_for_user(&ctx.user_id)
          .await
          .map(|rows| {
              rows.into_iter()
                  .map(|s| ActivitySettingDto { event_type: s.event_type, stream: s.stream })
                  .collect()
          })
          .map_err(|e| {
              tracing::error!(error = %e, "activity settings GET failed");
              ServerFnError::new(format!("activity settings: {e}"))
          })
  }

  #[server(endpoint = "/api/files/activity/settings/put")]
  pub async fn set_activity_setting(event_type: String, stream: bool) -> Result<(), ServerFnError> {
      use crate::server_fns::require_user;
      let (state, ctx) = require_user()?;
      state
          .activity_settings
          .set(&ctx.user_id, &event_type, stream)
          .await
          .map_err(|e| {
              tracing::error!(error = %e, event_type, "activity settings PUT failed");
              ServerFnError::new(format!("activity settings set: {e}"))
          })
  }
  ```

  Adjust `require_user()` to match the actual sibling pattern (look at `server_fns/versions.rs`).

- [ ] **Step 3: Mount in `server_fns/mod.rs`**

  ```rust
  pub mod activity;
  ```

  + re-export DTOs at the lib level if the UI batch will need them (check the SP13 lib.rs precedent).

- [ ] **Step 4: Integration test**

  Mirror `crates/crabcloud-app/tests/server_fns_versions.rs`. Cover:
  - `list_activity` empty.
  - `list_activity` returns seeded rows.
  - `set_activity_setting` + `get_activity_settings` round-trip.
  - Unauthenticated requests fail.

- [ ] **Step 5: Run + commit**

  ```bash
  cargo test -p crabcloud-app --test server_fns_activity
  git add crates/crabcloud-app/
  git commit -m "activity: server fns (list / get_settings / set_setting)"
  ```

### Task C2: Batch C pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp14/c-server-fns
  gh pr create --title "sp14(c): activity server fns" --body "Batch C of SP14 activity feed."
  ```

---

# Batch D — Dioxus UI

**Branch:** `sp14/d-ui` (off the merged Batch C master)

### Task D1: Sidebar entry

**Files:**
- Modify: `crates/crabcloud-app/src/pages/files/chrome.rs`

- [ ] **Step 1: Find the sibling sidebar entries**

  Read `chrome.rs`. Find "All files" + "Deleted files" entries. Add a third "Activity" entry pointing at `/activity` in the same style.

  ```rust
  Link {
      to: "/activity",
      class: "sidebar-item sidebar-link",
      span { class: "sidebar-item-icon", aria_hidden: "true", "📰" }
      span { class: "sidebar-item-label", "Activity" }
  }
  ```

- [ ] **Step 2: Commit**

  ```bash
  git add crates/crabcloud-app/src/pages/files/chrome.rs
  git commit -m "activity ui: 'Activity' sidebar entry"
  ```

### Task D2: Activity page

**Files:**
- Create: `crates/crabcloud-app/src/pages/activity.rs`
- Modify: `crates/crabcloud-app/src/pages/mod.rs`
- Modify: `crates/crabcloud-app/src/app.rs`

- [ ] **Step 1: Write `activity.rs`**

  Mirror `pages/trash.rs` for the page chrome (TopBar + Sidebar + error banner). The list itself is read-only — no per-row mutations — so the in-flight tracking the trash/versions pages have isn't needed here.

  ```rust
  //! Activity feed page.
  //!
  //! Lists recent activity for the authed user. Cursor pagination via a
  //! "Load more" button. Each row shows actor + subject string + relative
  //! timestamp + count badge when > 1.

  use crate::pages::files::chrome::{Sidebar, TopBar};
  use crate::server_fns::activity::{list_activity, ActivityRowDto, ListActivityResponse};
  use chrono::{TimeZone, Utc};
  use dioxus::prelude::*;

  #[component]
  pub fn ActivityPage() -> Element {
      let mut entries = use_signal::<Vec<ActivityRowDto>>(Vec::new);
      let mut next_since = use_signal::<Option<i64>>(|| None);
      let mut loading = use_signal::<bool>(|| true);
      let mut last_error = use_signal::<Option<String>>(|| None);

      // Initial load.
      use_effect(move || {
          spawn(async move {
              match list_activity(None, Some(30)).await {
                  Ok(ListActivityResponse { items, next_since: ns }) => {
                      entries.set(items);
                      next_since.set(ns);
                  }
                  Err(e) => last_error.set(Some(format!("Couldn't load activity: {e}"))),
              }
              loading.set(false);
          });
      });

      let on_load_more = move |_evt: MouseEvent| {
          let since = next_since();
          spawn(async move {
              match list_activity(since, Some(30)).await {
                  Ok(ListActivityResponse { items, next_since: ns }) => {
                      entries.with_mut(|v| v.extend(items));
                      next_since.set(ns);
                  }
                  Err(e) => last_error.set(Some(format!("Couldn't load more: {e}"))),
              }
          });
      };

      rsx! {
          div { class: "files-page",
              TopBar {}
              div { class: "files-body",
                  Sidebar {}
                  main { class: "activity-page",
                      div { class: "activity-header",
                          h2 { "Activity" }
                      }
                      if let Some(err) = last_error() {
                          ActivityBanner { msg: err, on_close: move |_| last_error.set(None) }
                      }
                      if loading() {
                          p { class: "activity-loading", "Loading..." }
                      } else if entries.read().is_empty() {
                          p { class: "activity-empty", "Nothing here yet." }
                      } else {
                          ul { class: "activity-list",
                              for entry in entries().iter() {
                                  ActivityRowView { key: "{entry.id}", entry: entry.clone() }
                              }
                          }
                          if next_since().is_some() {
                              button {
                                  r#type: "button",
                                  class: "activity-load-more",
                                  onclick: on_load_more,
                                  "Load more"
                              }
                          }
                      }
                  }
              }
          }
      }
  }

  #[derive(Props, Clone, PartialEq)]
  struct ActivityRowProps {
      entry: ActivityRowDto,
  }

  #[component]
  fn ActivityRowView(props: ActivityRowProps) -> Element {
      let when = format_when(props.entry.last_seen_at);
      let count_badge = if props.entry.count > 1 {
          rsx! { span { class: "activity-row-count", " +{props.entry.count - 1} more" } }
      } else {
          rsx! {}
      };
      rsx! {
          li { class: "activity-row",
              span { class: "activity-row-icon", aria_hidden: "true", "{icon_for(&props.entry.event_type)}" }
              span { class: "activity-row-subject", "{props.entry.subject}" }
              {count_badge}
              span { class: "activity-row-when", "{when}" }
          }
      }
  }

  fn icon_for(event_type: &str) -> &'static str {
      match event_type {
          "file_created" => "📄",
          "file_updated" => "✏",
          "file_deleted" => "🗑",
          "file_renamed" => "🏷",
          "file_restored" => "♻",
          "share_created" => "🔗",
          "share_deleted" => "✂",
          "version_restored" => "🕘",
          _ => "•",
      }
  }

  fn format_when(unix_secs: i64) -> String {
      Utc.timestamp_opt(unix_secs.max(0), 0)
          .single()
          .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
          .unwrap_or_else(|| unix_secs.to_string())
  }

  #[derive(Props, Clone, PartialEq)]
  struct BannerProps {
      msg: String,
      on_close: EventHandler<()>,
  }

  #[component]
  fn ActivityBanner(props: BannerProps) -> Element {
      rsx! {
          div { class: "activity-banner activity-banner-error", role: "alert",
              span { class: "activity-banner-msg", "{props.msg}" }
              button {
                  r#type: "button",
                  class: "activity-banner-close",
                  aria_label: "Dismiss",
                  onclick: move |_| props.on_close.call(()),
                  "×"
              }
          }
      }
  }
  ```

- [ ] **Step 2: Wire `pages/mod.rs` and `app.rs`**

  In `pages/mod.rs`: `pub mod activity;`.

  In `app.rs`, register the route — mirror the `/trash` route added in SP12. The exact macro / enum variant depends on dioxus-router 0.7's idiom; copy the shape of the `TrashRoute` variant.

- [ ] **Step 3: SSR snapshot tests**

  Inside `activity.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      // Mirror trash.rs tests: hermetic SSR snapshot via inner Wrapper
      // pattern. Cover: empty state, two-rows-with-icons-and-when,
      // coalesced-row-shows-count-badge, error-banner-renders.
  }
  ```

  Look at `pages/trash.rs::tests` for the exact Wrapper pattern; replicate.

- [ ] **Step 4: CSS**

  In `assets/app.css`, add ~50 lines of `.activity-*` styles mirroring `.trash-*` palette (`#0082c9` accent, `#666/#888` muted text, hover background `#f5fafd`). Cover: `.activity-page`, `.activity-header`, `.activity-list`, `.activity-row`, `.activity-row-icon`, `.activity-row-subject`, `.activity-row-count`, `.activity-row-when`, `.activity-load-more`, `.activity-empty`, `.activity-loading`, `.activity-banner`, `.activity-banner-error`, `.activity-banner-msg`, `.activity-banner-close`.

- [ ] **Step 5: Build + test + WASM build**

  ```bash
  cargo test -p crabcloud-app
  cargo test --workspace
  cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add crates/crabcloud-app/src/pages/ crates/crabcloud-app/src/app.rs crates/crabcloud-app/assets/app.css
  git commit -m "activity ui: /activity page with load-more pagination + CSS"
  ```

### Task D3: Settings page

**Files:**
- Create: `crates/crabcloud-app/src/pages/activity_settings.rs`
- Modify: `crates/crabcloud-app/src/pages/mod.rs`
- Modify: `crates/crabcloud-app/src/app.rs`

- [ ] **Step 1: Write `activity_settings.rs`**

  A simple settings page: fetch `get_activity_settings()` → render one toggle per event type → on toggle, call `set_activity_setting()`. Use the same in-flight + error banner patterns as the trash/versions polish.

  The list of event types is the 8 from `EventType::as_str()`. Render each as a labeled checkbox. Default-true semantics — if no row exists in the response, render the checkbox as checked.

  Skeleton:
  ```rust
  use crate::pages::files::chrome::{Sidebar, TopBar};
  use crate::server_fns::activity::{get_activity_settings, set_activity_setting, ActivitySettingDto};
  use dioxus::prelude::*;
  use std::collections::HashMap;

  const EVENT_TYPES: &[(&str, &str)] = &[
      ("file_created",     "File created"),
      ("file_updated",     "File updated"),
      ("file_deleted",     "File deleted"),
      ("file_renamed",     "File renamed"),
      ("file_restored",    "File restored from trash"),
      ("share_created",    "Share created"),
      ("share_deleted",    "Share removed"),
      ("version_restored", "Version restored"),
  ];

  #[component]
  pub fn ActivitySettingsPage() -> Element {
      let mut settings = use_signal::<HashMap<String, bool>>(HashMap::new);
      let mut loaded = use_signal::<bool>(|| false);
      let mut last_error = use_signal::<Option<String>>(|| None);

      use_effect(move || {
          spawn(async move {
              match get_activity_settings().await {
                  Ok(rows) => {
                      let mut m = HashMap::new();
                      for r in rows { m.insert(r.event_type, r.stream); }
                      settings.set(m);
                  }
                  Err(e) => last_error.set(Some(format!("Couldn't load settings: {e}"))),
              }
              loaded.set(true);
          });
      });

      let on_toggle = move |(event_type, new_value): (String, bool)| {
          settings.with_mut(|m| { m.insert(event_type.clone(), new_value); });
          spawn(async move {
              if let Err(e) = set_activity_setting(event_type.clone(), new_value).await {
                  last_error.set(Some(format!("Couldn't update {event_type}: {e}")));
              }
          });
      };

      rsx! {
          div { class: "files-page",
              TopBar {}
              div { class: "files-body",
                  Sidebar {}
                  main { class: "activity-settings",
                      h2 { "Activity settings" }
                      if let Some(err) = last_error() {
                          p { class: "activity-banner activity-banner-error", role: "alert", "{err}" }
                      }
                      if !loaded() {
                          p { "Loading..." }
                      } else {
                          ul { class: "activity-settings-list",
                              for (event_type, label) in EVENT_TYPES.iter() {
                                  {
                                      let event_type = event_type.to_string();
                                      let checked = *settings.read().get(&event_type).unwrap_or(&true);
                                      let on_toggle = on_toggle.clone();
                                      let event_type_for_handler = event_type.clone();
                                      rsx! {
                                          li { key: "{event_type}", class: "activity-settings-row",
                                              label {
                                                  input {
                                                      r#type: "checkbox",
                                                      checked: checked,
                                                      onchange: move |e| {
                                                          let new_val = e.checked();
                                                          on_toggle((event_type_for_handler.clone(), new_val));
                                                      }
                                                  }
                                                  span { class: "activity-settings-label", "{label}" }
                                              }
                                          }
                                      }
                                  }
                              }
                          }
                      }
                  }
              }
          }
      }
  }
  ```

- [ ] **Step 2: Route + mod registration**

  In `pages/mod.rs`: `pub mod activity_settings;`.

  In `app.rs`: register `/activity/settings` route the same way `/activity` was wired.

- [ ] **Step 3: SSR snapshot test**

  ```rust
  #[cfg(test)]
  mod tests {
      // Mirror the trash settings SSR pattern. Cover:
      //  - All event-type rows render (8 of them).
      //  - Default-true semantics: when get_activity_settings returns
      //    empty, every checkbox is checked.
      //  - Returned override (e.g. file_updated=false) renders unchecked.
  }
  ```

- [ ] **Step 4: Build + test**

  ```bash
  cargo test -p crabcloud-app
  cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
  ```

- [ ] **Step 5: Commit**

  ```bash
  git add crates/crabcloud-app/
  git commit -m "activity ui: /activity/settings page with per-event-type toggles"
  ```

### Task D4: Batch D pre-PR

- [ ] **Pre-flight + push + PR**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  git push -u origin sp14/d-ui
  gh pr create --title "sp14(d): activity UI (sidebar + page + settings)" \
    --body "Final batch of SP14 activity feed. 'Activity' sidebar entry alongside 'Deleted files'; /activity page with load-more pagination; /activity/settings page with per-event-type stream toggles."
  ```

---

## Self-review notes

- **Spec coverage:** §1 goal → all batches. §2 decisions → A1–A13 (1–10), B (11–12), C (13), D (14). §3 architecture → A2–A8. §4 schema → A1. §5 surfaces → B (5.1) + C (5.2). §6 edge cases → A4 e2e (coalesce + opt-out + race tolerance), A9–A12 (per-emitter recipient resolution, including public-link actor=""), D4 (settings UI for opt-out). §7 testing list → e2e + unit + integration at every layer. §8 batches → 4 batches.

- **Placeholder scan:** A few honest "look at the sibling pattern in X" instructions — these point at established workspace conventions (`require_user`, OCS envelope helpers, dispatch-by-method, the Wrapper SSR-test pattern). The `row_to_activity` generic-trait shape is the one spot most likely to need per-dialect inlining at impl time; called out with the SP13 precedent. The `SharesConfig` field names in Task A11 step 2 say "adjust to actual `CreateShareRequest` shape" — that's irreducibly codebase-specific.

- **Type consistency:** `ActivityEvent` shape consistent A2→A4→A9–A12. `ActivityRow` consistent A2→A4→B→C. `EventType::as_str()` values consistent A2→A4→A9–A12→subject templates A5→UI D3. `ActivitySetting` consistent A2→A4→settings.rs→B→C→D3.

- **Known underspecified spots** the implementer must resolve from the codebase:
  - The exact `AuthContext` extension field name (`user_id`?) — mirror sibling OCS handlers.
  - The exact `require_user()` extractor signature — mirror SP13 `server_fns/versions.rs`.
  - The exact `SharesConfig` / `CreateShareRequest` field names (`requester`, `share_with`, `share_type`, `path`) — read `crates/crabcloud-sharing/src/types.rs` first.
  - The exact `View::new` call-site ripple (Task A12) — grep + update each; mirrors SP13 Batch A's ripple count (≈8 sites).
  - The exact `pre_write_existed` / `post_write_fileid` shape (Task A12) — depends on whether `View::write_file` already does a pre-write filecache lookup that A12 can piggyback on.
  - Group expansion in Task A11 step 2: the `users.group_members(group_id)` call signature is codebase-specific. If `crabcloud-users::UsersService` exposes a different method (e.g. `list_group_members` or via the `GroupStore` trait), use that.
