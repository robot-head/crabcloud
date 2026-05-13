# Sharing (user + group) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship outgoing user-and-group shares in Crabcloud, with the recipient seeing each accepted share as a virtual mount at their filesystem root (visible in both the web UI and over WebDAV / desktop clients).

**Architecture:** Owner creates shares via Nextcloud-compatible OCS `/ocs/v2.php/apps/files_sharing/api/v1/shares`. A new `crabcloud-sharing` crate owns CRUD + group-aware lookup against a new `oc_share` table (migration `0006_shares`). A new `SharedSubrootStorage` wrapper in `crabcloud-fs` subroots + permission-filters the owner's storage; a new `ShareMountResolver` returns the home mount plus one share mount per accepted incoming share. The Files UI gets a Share button (third entry in the row ⋯ menu), a `ShareModal`, a sidebar "Shared with you" chip, and shared-by / share-count badges on file rows.

**Tech Stack:** Rust 1.95, sqlx 0.8 (sqlite/mysql/postgres), axum 0.8, Dioxus 0.7 fullstack, Playwright. Builds on the existing `crabcloud-fs` (`View`, `MountResolver`, `Storage`), `crabcloud-users` (`Users`, `Groups`), `crabcloud-filecache` (`Filecache`), `crabcloud-http` (`build_router`, OCS subrouter, `AuthLayer`/`AuthContext`).

**Spec:** `docs/superpowers/specs/2026-05-13-sharing-user-group-and-virtual-mount-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`. At the start of each batch:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp7/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-schema`, `b-shares-service`, `c-mount-wrapper`, `d-ocs-api`, `e-ui`, `f-tests-polish`.
- **Commit cadence:** Commit at every "Commit" step. Frequent, focused commits.
- **Pre-PR check:** Before opening the PR, run:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass locally.
- **Open the PR:**
  ```bash
  git push -u origin sp7/<batch-letter>-<slug>
  gh pr create --title "sp7: batch <X> — <topic>" --body "$(cat <<'EOF'
  ## Summary
  - <one-line bullets>

  ## Test plan
  - [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
  - [ ] <batch-specific manual checks>

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```
- **Merge:** After all checks pass: `gh pr merge --squash --delete-branch`.
- **Established workaround:** Whenever a test builds `AppState`, set `cfg.filecache.enabled = false` before `AppStateBuilder::new(cfg).build()` to avoid the scanner-race that's hit other batches. See `crates/crabcloud-http/tests/dav_basic.rs:16-37` for the pattern.
- **Pre-existing patterns to mirror:**
  - **Per-dialect SQL migration triplet**: `migrations/core/0005_webdav_props_and_locks/{sqlite,mysql,postgres}.sql`.
  - **Service crate shape**: `crates/crabcloud-users` (split into `lib.rs`, `types.rs`, `users.rs`, `groups.rs`, etc.).
  - **Per-dialect integration test harness**: `crates/crabcloud-db/tests/migrate_end_to_end.rs` (uses `testcontainers` + `crabcloud-config::test_support`).
  - **OCS handler shape**: `crates/crabcloud-http/src/routes/ocs/admin_users.rs` (envelope + format negotiation + `AuthContext` + form-encoded body extraction).
  - **Server function shape**: `crates/crabcloud-ui/src/server_fns/files.rs` (POST + JSON body + `FullstackContext::extension::<AppState>` + `require_user`).
  - **Page component shape**: `crates/crabcloud-ui/src/pages/files/delete_modal.rs` (modal chrome to reuse for `ShareModal`).
  - **Playwright shape**: `e2e/tests/files.spec.ts` (cookie login + hydration wait + locator assertions).

---

## File-by-file map

### New crate: `crabcloud-sharing`

```
crates/crabcloud-sharing/
├── Cargo.toml
├── src/
│   ├── lib.rs               — re-exports
│   ├── error.rs             — ShareError
│   ├── permissions.rs       — SharePermissions (u8 bitmask wrapper)
│   ├── types.rs             — ShareType, ItemType, ShareRow, CreateShareRequest, UpdateShareFields
│   ├── service.rs           — Shares struct + impl (create/get/list/update/delete)
│   └── sql.rs               — SQL constants + Row deserialization
└── tests/
    └── sharing_e2e.rs       — per-dialect integration tests (testcontainers harness)
```

### New migration

```
migrations/core/0006_shares/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### New / modified `crabcloud-fs`

```
crates/crabcloud-fs/src/
├── mount.rs                 — MODIFY: add Mount.metadata + MountMetadata + MountKind
├── storage/
│   └── share_subroot.rs     — NEW: SharedSubrootStorage
└── resolver/
    └── share.rs             — NEW: ShareMountResolver
```

(Where `storage/` doesn't exist yet — we create the module under `mod.rs` to host the wrapper.)

### Modified `crabcloud-core`

- `crates/crabcloud-core/src/state.rs` — wire `Shares` into `AppState` + `AppStateBuilder`.

### New / modified `crabcloud-http`

```
crates/crabcloud-http/src/routes/ocs/
├── files_sharing.rs         — NEW: 5 OCS endpoints
└── mod.rs                   — MODIFY: nest files_sharing router

crates/crabcloud-http/tests/
└── files_sharing_e2e.rs     — NEW: handler-level tests
```

### Modified `crabcloud-server`

- `crates/crabcloud-server/src/main.rs` — swap `HomeMountResolver` for `ShareMountResolver` in the AppState wiring.

### New / modified `crabcloud-ui`

```
crates/crabcloud-ui/src/
├── pages/files/
│   ├── row.rs              — MODIFY: add Share entry to ⋯ menu
│   ├── list.rs             — MODIFY: render shared_by + share_count badges
│   ├── chrome.rs           — MODIFY: add "Shared with you" sidebar chip
│   └── share_modal.rs      — NEW: ShareModal + recipient picker + permission toggles
└── server_fns/
    └── files.rs            — MODIFY: extend FileEntry DTO; add share_recipient_search server fn
```

### Modified `Cargo.toml` (workspace root)

- Add `crates/crabcloud-sharing` to `members`.
- Add `crabcloud-sharing` to `[workspace.dependencies]`.
- Add `chrono` workspace dep for `DateTime<Utc>` in `ShareRow.expiration`.

### Modified crate `Cargo.toml`s

- `crates/crabcloud-sharing/Cargo.toml` — new.
- `crates/crabcloud-fs/Cargo.toml` — add `crabcloud-sharing`.
- `crates/crabcloud-core/Cargo.toml` — add `crabcloud-sharing`.
- `crates/crabcloud-http/Cargo.toml` — add `crabcloud-sharing` (for OCS handlers).
- `crates/crabcloud-server/Cargo.toml` — add `crabcloud-sharing` (factory wiring).
- `crates/crabcloud-ui/Cargo.toml` — no new deps (DTO change is plain serde).

### New e2e

- `e2e/tests/sharing.spec.ts` — sharing scenarios.

---

## Batch A — Schema + crate skeleton

**Branch:** `sp7/a-schema` off `origin/master`.
**Goal:** Migration `0006_shares` lands on all three dialects. `crabcloud-sharing` crate exists with types, error, permissions, and the empty `Shares` service struct. No CRUD logic yet — that's Batch B. All existing tests still pass; migration-end-to-end test recognizes the new migration.

### Task A1: Create the migration triplet

**Files:**
- Create: `migrations/core/0006_shares/sqlite.sql`
- Create: `migrations/core/0006_shares/mysql.sql`
- Create: `migrations/core/0006_shares/postgres.sql`

- [ ] **Step 1: Create the sqlite migration**

Create `migrations/core/0006_shares/sqlite.sql`:
```sql
CREATE TABLE oc_share (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    share_type    SMALLINT     NOT NULL,
    share_with    VARCHAR(255) NULL,
    uid_owner     VARCHAR(64)  NOT NULL,
    uid_initiator VARCHAR(64)  NOT NULL,
    parent        BIGINT       NULL,
    item_type     VARCHAR(64)  NOT NULL,
    item_source   BIGINT       NOT NULL,
    file_source   BIGINT       NOT NULL,
    file_target   VARCHAR(512) NOT NULL,
    permissions   INTEGER      NOT NULL,
    stime         BIGINT       NOT NULL,
    accepted      SMALLINT     NOT NULL DEFAULT 1,
    expiration    TIMESTAMP    NULL,
    token         VARCHAR(32)  NULL,
    password      VARCHAR(255) NULL,
    mail_send     SMALLINT     NOT NULL DEFAULT 0
);

CREATE INDEX idx_share_with        ON oc_share (share_with, share_type);
CREATE INDEX idx_share_owner       ON oc_share (uid_owner);
CREATE INDEX idx_share_item_source ON oc_share (item_source);
CREATE UNIQUE INDEX idx_share_token ON oc_share (token) WHERE token IS NOT NULL;
```

- [ ] **Step 2: Create the postgres migration**

Create `migrations/core/0006_shares/postgres.sql`:
```sql
CREATE TABLE oc_share (
    id            BIGSERIAL    PRIMARY KEY,
    share_type    SMALLINT     NOT NULL,
    share_with    VARCHAR(255) NULL,
    uid_owner     VARCHAR(64)  NOT NULL,
    uid_initiator VARCHAR(64)  NOT NULL,
    parent        BIGINT       NULL,
    item_type     VARCHAR(64)  NOT NULL,
    item_source   BIGINT       NOT NULL,
    file_source   BIGINT       NOT NULL,
    file_target   VARCHAR(512) NOT NULL,
    permissions   INTEGER      NOT NULL,
    stime         BIGINT       NOT NULL,
    accepted      SMALLINT     NOT NULL DEFAULT 1,
    expiration    TIMESTAMP    NULL,
    token         VARCHAR(32)  NULL,
    password      VARCHAR(255) NULL,
    mail_send     SMALLINT     NOT NULL DEFAULT 0
);

CREATE INDEX idx_share_with        ON oc_share (share_with, share_type);
CREATE INDEX idx_share_owner       ON oc_share (uid_owner);
CREATE INDEX idx_share_item_source ON oc_share (item_source);
CREATE UNIQUE INDEX idx_share_token ON oc_share (token) WHERE token IS NOT NULL;
```

- [ ] **Step 3: Create the mysql migration**

Create `migrations/core/0006_shares/mysql.sql`. MySQL needs an explicit row format + charset, and its unique-index handles NULLs as distinct (so the partial-index dance isn't needed). Mirror `0003_auth_tokens/mysql.sql`'s style:
```sql
CREATE TABLE oc_share (
    id            BIGINT       NOT NULL AUTO_INCREMENT,
    share_type    SMALLINT     NOT NULL,
    share_with    VARCHAR(255) NULL,
    uid_owner     VARCHAR(64)  NOT NULL,
    uid_initiator VARCHAR(64)  NOT NULL,
    parent        BIGINT       NULL,
    item_type     VARCHAR(64)  NOT NULL,
    item_source   BIGINT       NOT NULL,
    file_source   BIGINT       NOT NULL,
    file_target   VARCHAR(512) NOT NULL,
    permissions   INTEGER      NOT NULL,
    stime         BIGINT       NOT NULL,
    accepted      SMALLINT     NOT NULL DEFAULT 1,
    expiration    TIMESTAMP    NULL,
    token         VARCHAR(32)  NULL,
    password      VARCHAR(255) NULL,
    mail_send     SMALLINT     NOT NULL DEFAULT 0,
    PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE INDEX idx_share_with        ON oc_share (share_with, share_type);
CREATE INDEX idx_share_owner       ON oc_share (uid_owner);
CREATE INDEX idx_share_item_source ON oc_share (item_source);
CREATE UNIQUE INDEX idx_share_token ON oc_share (token);
```

- [ ] **Step 4: Run the end-to-end migration test on sqlite**

The migration runner picks up new directories automatically (`crabcloud-db::migrate` enumerates `migrations/core/*`). Confirm:
```bash
cargo test -p crabcloud-db --test migrate_end_to_end
```
Expected: PASS. The test creates a fresh sqlite DB, runs every migration in order, then asserts the schema has all known tables. We need to update the assertion in the next step.

- [ ] **Step 5: Extend the schema-assertion test to expect `oc_share`**

Open `crates/crabcloud-db/tests/migrate_end_to_end.rs`. Find the list of expected table names (search for `oc_filecache` to locate it) and append `"oc_share"` to the list. Rerun the test:
```bash
cargo test -p crabcloud-db --test migrate_end_to_end
```
Expected: PASS on sqlite.

- [ ] **Step 6: Run the same test against mysql + postgres**

```bash
cargo test -p crabcloud-db --test migrate_end_to_end -- --include-ignored
```
This brings up testcontainers for both dialects. Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add migrations/core/0006_shares crates/crabcloud-db/tests/migrate_end_to_end.rs
git commit -m "sp7(a): migration 0006_shares — oc_share schema"
```

### Task A2: Scaffold `crabcloud-sharing` crate

**Files:**
- Create: `crates/crabcloud-sharing/Cargo.toml`
- Create: `crates/crabcloud-sharing/src/lib.rs`
- Modify: `Cargo.toml` (workspace) — add member + dep
- Modify: `Cargo.toml` (workspace) — add `chrono` workspace dep

- [ ] **Step 1: Add `chrono` workspace dep**

In the workspace root `Cargo.toml`, under `[workspace.dependencies]`, add:
```toml
chrono = { version = "0.4", default-features = false, features = ["serde", "clock"] }
```

- [ ] **Step 2: Add the new crate to the workspace**

In the workspace root `Cargo.toml`:
- Under `[workspace] members`, insert `"crates/crabcloud-sharing",` (alphabetical position, before `crabcloud-storage`).
- Under `[workspace.dependencies]` near the other `crabcloud-*` entries, add:
  ```toml
  crabcloud-sharing = { path = "crates/crabcloud-sharing" }
  ```

- [ ] **Step 3: Create the crate's `Cargo.toml`**

Create `crates/crabcloud-sharing/Cargo.toml`:
```toml
[package]
name = "crabcloud-sharing"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
chrono.workspace = true
crabcloud-db.workspace = true
crabcloud-filecache.workspace = true
crabcloud-fs.workspace = true
crabcloud-storage.workspace = true
crabcloud-users.workspace = true
serde.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["sync"] }
tracing.workspace = true

[dev-dependencies]
crabcloud-config = { workspace = true, features = ["test-support"] }
crabcloud-core.workspace = true
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

Note: `crabcloud-fs` is a dep only for `UserPath` / `StoragePath` types. We accept the dep direction `sharing → fs` (and add a NOTE later if `fs → sharing` is needed for `ShareMountResolver` — see Batch C, which constructs `ShareMountResolver` in `fs` but takes `Shares` as a runtime trait object via dyn dispatch to avoid a cycle).

Actually — to avoid the cycle, this plan defers the dep direction: `crabcloud-sharing` will use `crabcloud-storage` for `StoragePath` and a local-only `UserPath` re-export from `crabcloud-users`. Replace the `crabcloud-fs` dep with: nothing for now. Final `Cargo.toml`:

```toml
[package]
name = "crabcloud-sharing"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
chrono.workspace = true
crabcloud-db.workspace = true
crabcloud-filecache.workspace = true
crabcloud-storage.workspace = true
crabcloud-users.workspace = true
serde.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["sync"] }
tracing.workspace = true

[dev-dependencies]
crabcloud-config = { workspace = true, features = ["test-support"] }
crabcloud-core.workspace = true
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

Path inputs (`/Vacation Photos`) are accepted as plain `&str` by `Shares::create` and validated against `crabcloud_users::UserId` for uids/gids; we don't need a `UserPath` wrapper here.

- [ ] **Step 4: Create `src/lib.rs`**

Create `crates/crabcloud-sharing/src/lib.rs`:
```rust
//! User and group sharing for Crabcloud.
//!
//! Schema lives in `migrations/core/0006_shares`. Design spec:
//! `docs/superpowers/specs/2026-05-13-sharing-user-group-and-virtual-mount-design.md`.

mod error;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use permissions::SharePermissions;
pub use service::Shares;
pub use types::{
    CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields,
};
```

- [ ] **Step 5: Sanity-check the workspace still builds**

```bash
cargo check --workspace
```
Expected: FAIL — the modules `error`, `permissions`, etc. don't exist yet. That's intentional; the next tasks add them.

- [ ] **Step 6: Commit the scaffolding**

```bash
git add Cargo.toml crates/crabcloud-sharing/Cargo.toml crates/crabcloud-sharing/src/lib.rs
git commit -m "sp7(a): scaffold crabcloud-sharing crate"
```

### Task A3: `SharePermissions` bitmask

**Files:**
- Create: `crates/crabcloud-sharing/src/permissions.rs`

- [ ] **Step 1: Write the failing tests first**

Create `crates/crabcloud-sharing/src/permissions.rs`:
```rust
//! Permission bitmask wrapper for shares. Layout matches Nextcloud:
//! bit 1 = read, 2 = update, 4 = create, 8 = delete, 16 = share.
//!
//! SP7 invariant: stored values always have bit 16 cleared (no re-share).

use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SharePermissions(u8);

impl SharePermissions {
    pub const READ:   Self = Self(1);
    pub const UPDATE: Self = Self(2);
    pub const CREATE: Self = Self(4);
    pub const DELETE: Self = Self(8);
    pub const SHARE:  Self = Self(16);

    /// Mask off bits we don't track (≥ 32) and the SP7-prohibited share bit
    /// (16). The caller is responsible for asserting that bit 1 (read) is
    /// still set after this — see `Shares::create`.
    pub fn from_bitmask_strip_share(b: u32) -> Self {
        Self(((b & 0x1F) & !Self::SHARE.0 as u32) as u8)
    }

    pub fn bits(self) -> u8 { self.0 }
    pub fn bitmask(self) -> u32 { self.0 as u32 }

    pub fn contains_read(self)   -> bool { (self.0 & Self::READ.0)   != 0 }
    pub fn allows_write(self)    -> bool { (self.0 & (Self::UPDATE.0 | Self::CREATE.0)) != 0 }
    pub fn allows_update(self)   -> bool { (self.0 & Self::UPDATE.0) != 0 }
    pub fn allows_create(self)   -> bool { (self.0 & Self::CREATE.0) != 0 }
    pub fn allows_delete(self)   -> bool { (self.0 & Self::DELETE.0) != 0 }
}

impl From<i32> for SharePermissions {
    fn from(v: i32) -> Self { Self::from_bitmask_strip_share(v as u32) }
}
impl From<SharePermissions> for i32 {
    fn from(v: SharePermissions) -> Self { v.0 as i32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_share_bit() {
        let p = SharePermissions::from_bitmask_strip_share(0b11111); // 31
        assert_eq!(p.bits(), 0b01111); // 15
        assert!(!((p.bitmask() as u8) & SharePermissions::SHARE.0 != 0));
    }

    #[test]
    fn drops_bits_above_31() {
        let p = SharePermissions::from_bitmask_strip_share(0xFF);
        assert_eq!(p.bits(), 0b01111);
    }

    #[test]
    fn read_only_does_not_allow_write_or_delete() {
        let p = SharePermissions::from_bitmask_strip_share(1);
        assert!(p.contains_read());
        assert!(!p.allows_write());
        assert!(!p.allows_delete());
    }

    #[test]
    fn update_allows_write_but_not_create_or_delete() {
        let p = SharePermissions::from_bitmask_strip_share(1 | 2);
        assert!(p.allows_write());
        assert!(p.allows_update());
        assert!(!p.allows_create());
        assert!(!p.allows_delete());
    }

    #[test]
    fn create_allows_write_too() {
        let p = SharePermissions::from_bitmask_strip_share(1 | 4);
        assert!(p.allows_write());
        assert!(p.allows_create());
        assert!(!p.allows_update());
    }

    #[test]
    fn full_perms_minus_share() {
        let p = SharePermissions::from_bitmask_strip_share(31);
        assert!(p.contains_read());
        assert!(p.allows_update());
        assert!(p.allows_create());
        assert!(p.allows_delete());
        assert_eq!(p.bits(), 15);
    }

    #[test]
    fn roundtrip_i32() {
        let p = SharePermissions::from_bitmask_strip_share(7);
        let n: i32 = p.into();
        assert_eq!(n, 7);
        let p2: SharePermissions = n.into();
        assert_eq!(p2, p);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p crabcloud-sharing --lib permissions
```
Expected: PASS (7 tests).

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-sharing/src/permissions.rs
git commit -m "sp7(a): SharePermissions bitmask + tests"
```

### Task A4: `ShareError`

**Files:**
- Create: `crates/crabcloud-sharing/src/error.rs`

- [ ] **Step 1: Write the type**

Create `crates/crabcloud-sharing/src/error.rs`:
```rust
//! Errors returned by the Shares service.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShareError {
    #[error("share not found")]
    NotFound,

    #[error("forbidden")]
    Forbidden,

    #[error("recipient unknown")]
    RecipientUnknown,

    #[error("invalid share type")]
    InvalidShareType,

    #[error("bad permissions bitmask")]
    BadPermissions,

    #[error("re-share rejected: only the owner of a file can share it")]
    ReshareRejected,

    #[error("path not owned by requester (or missing)")]
    PathNotOwned,

    #[error("not implemented in this version (deferred to SP8)")]
    NotImplemented,

    #[error(transparent)]
    DbError(#[from] sqlx::Error),
}

impl ShareError {
    /// HTTP status code that best maps to this error. Used by the OCS
    /// handler layer; kept here so error→status is one consistent table.
    pub fn http_status(&self) -> u16 {
        match self {
            ShareError::NotFound => 404,
            ShareError::Forbidden | ShareError::ReshareRejected | ShareError::PathNotOwned => 403,
            ShareError::RecipientUnknown => 404,
            ShareError::InvalidShareType | ShareError::BadPermissions => 400,
            ShareError::NotImplemented => 501,
            ShareError::DbError(_) => 500,
        }
    }
}
```

- [ ] **Step 2: Confirm it compiles**

```bash
cargo check -p crabcloud-sharing
```
Expected: still failing — `types`, `sql`, `service` modules don't exist. We'll add stubs next so we can keep moving.

- [ ] **Step 3: Add module stubs so the crate compiles**

Create `crates/crabcloud-sharing/src/types.rs`:
```rust
//! Public types for the sharing service.
//! Populated in subsequent tasks; this stub keeps `lib.rs` re-exports honest.

// Stub re-exports are filled in in Task A5.
```

Create `crates/crabcloud-sharing/src/sql.rs`:
```rust
//! SQL constants + row deserialization. Stub; filled in Batch B.
```

Create `crates/crabcloud-sharing/src/service.rs`:
```rust
//! `Shares` service. Stub; filled in Batch B.
```

Adjust `crates/crabcloud-sharing/src/lib.rs` to comment out the not-yet-defined re-exports (we'll restore them as Task A5 / Batch B lands them):
```rust
mod error;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use permissions::SharePermissions;
// Populated by subsequent tasks:
// pub use service::Shares;
// pub use types::{CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields};
```

- [ ] **Step 4: Confirm it compiles**

```bash
cargo check -p crabcloud-sharing
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-sharing/src/error.rs crates/crabcloud-sharing/src/types.rs crates/crabcloud-sharing/src/sql.rs crates/crabcloud-sharing/src/service.rs crates/crabcloud-sharing/src/lib.rs
git commit -m "sp7(a): ShareError + module stubs"
```

### Task A5: `ShareType`, `ItemType`, `ShareRow`, request/update types

**Files:**
- Modify: `crates/crabcloud-sharing/src/types.rs`
- Modify: `crates/crabcloud-sharing/src/lib.rs`

- [ ] **Step 1: Replace the stub with real types + tests**

Replace `crates/crabcloud-sharing/src/types.rs`:
```rust
//! Public types for the sharing service.

use crate::permissions::SharePermissions;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i16", try_from = "i16")]
pub enum ShareType {
    User  = 0,
    Group = 1,
    Link  = 3,
}

impl TryFrom<i16> for ShareType {
    type Error = &'static str;
    fn try_from(v: i16) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ShareType::User),
            1 => Ok(ShareType::Group),
            3 => Ok(ShareType::Link),
            _ => Err("unsupported share_type"),
        }
    }
}

impl From<ShareType> for i16 {
    fn from(v: ShareType) -> Self { v as i16 }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    File,
    Folder,
}

impl ItemType {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ItemType::File => "file",
            ItemType::Folder => "folder",
        }
    }
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "folder" => Some(Self::Folder),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShareRow {
    pub id: i64,
    pub share_type: ShareType,
    pub share_with: Option<String>,
    pub uid_owner: String,
    pub uid_initiator: String,
    pub parent: Option<i64>,
    pub item_type: ItemType,
    pub item_source: i64,
    pub file_source: i64,
    pub file_target: String,
    pub permissions: SharePermissions,
    pub stime: i64,
    pub accepted: bool,
    pub expiration: Option<DateTime<Utc>>,
    pub token: Option<String>,
    pub password_hash: Option<String>,
}

/// Caller-supplied create request. The service validates and normalizes
/// before insertion. `requester` is the authenticated user driving the
/// request; SP7 requires `requester == owner`.
#[derive(Debug, Clone)]
pub struct CreateShareRequest {
    pub requester: String,
    pub path: String,           // absolute path inside requester's home
    pub share_type: ShareType,
    pub share_with: String,
    pub permissions: u32,       // raw bitmask from wire
}

#[derive(Debug, Clone, Default)]
pub struct UpdateShareFields {
    pub permissions:  Option<u32>,
    pub expire_date:  Option<Option<NaiveDate>>,
    pub password:     Option<Option<String>>,
    pub note:         Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_type_round_trips_via_i16() {
        for v in [0_i16, 1, 3] {
            let st = ShareType::try_from(v).unwrap();
            assert_eq!(i16::from(st), v);
        }
    }

    #[test]
    fn share_type_rejects_unknown() {
        assert!(ShareType::try_from(2_i16).is_err());
        assert!(ShareType::try_from(99_i16).is_err());
    }

    #[test]
    fn item_type_db_round_trip() {
        for it in [ItemType::File, ItemType::Folder] {
            assert_eq!(ItemType::from_db_str(it.as_db_str()), Some(it));
        }
        assert!(ItemType::from_db_str("symlink").is_none());
    }
}
```

- [ ] **Step 2: Restore the re-exports in `lib.rs`**

Edit `crates/crabcloud-sharing/src/lib.rs` — uncomment the previously commented lines so it reads:
```rust
mod error;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use permissions::SharePermissions;
pub use types::{CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields};
// Shares re-export lands when the service is implemented in Batch B.
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p crabcloud-sharing --lib types
```
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-sharing/src/types.rs crates/crabcloud-sharing/src/lib.rs
git commit -m "sp7(a): ShareType, ItemType, ShareRow + request types"
```

### Task A6: Empty `Shares` service struct

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-sharing/src/lib.rs`

- [ ] **Step 1: Add the struct**

Replace `crates/crabcloud-sharing/src/service.rs`:
```rust
//! `Shares` — sharing CRUD service. CRUD implementations land in Batch B.

use crabcloud_db::DbPool;
use crabcloud_filecache::Filecache;
use crabcloud_users::Users;
use std::sync::Arc;

#[derive(Clone)]
pub struct Shares {
    pub(crate) pool: Arc<DbPool>,
    pub(crate) users: Arc<Users>,
    pub(crate) filecache: Arc<Filecache>,
}

impl Shares {
    pub fn new(pool: Arc<DbPool>, users: Arc<Users>, filecache: Arc<Filecache>) -> Self {
        Self { pool, users, filecache }
    }
}
```

If `crabcloud_users::Users` isn't directly importable as a single type (it may expose `UserService` or similar) — look at `crates/crabcloud-users/src/lib.rs` for the canonical name. If the project uses `Arc<dyn UserLookup>` instead, replace `Arc<Users>` with the trait object. **Verification step:** grep for `pub use` in `crates/crabcloud-users/src/lib.rs`. If `Users` is a struct alias for the concrete service, use it; otherwise substitute the actual type and add an analogous Groups field by inspecting `groups_of` accessibility.

- [ ] **Step 2: Restore `Shares` re-export**

Final `crates/crabcloud-sharing/src/lib.rs`:
```rust
mod error;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use permissions::SharePermissions;
pub use service::Shares;
pub use types::{CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields};
```

- [ ] **Step 3: Confirm the workspace compiles**

```bash
cargo check --workspace
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-sharing/src/service.rs crates/crabcloud-sharing/src/lib.rs
git commit -m "sp7(a): Shares service skeleton"
```

### Task A7: PR checks + open Batch A PR

- [ ] **Step 1: Pre-PR sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p crabcloud-db --test migrate_end_to_end -- --include-ignored
```
All four must pass.

- [ ] **Step 2: Push + open PR**

```bash
git push -u origin sp7/a-schema
gh pr create --title "sp7: batch A — schema + sharing crate skeleton" --body "$(cat <<'EOF'
## Summary
- Migration 0006_shares: `oc_share` table on sqlite/mysql/postgres.
- New `crabcloud-sharing` crate: `ShareError`, `SharePermissions`, `ShareType`, `ItemType`, `ShareRow`, request/update types, empty `Shares` service struct.
- Service CRUD logic lands in batch B.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `oc_share` shows up in the multidialect end-to-end migration test.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Wait for CI green; merge**

```bash
gh pr merge --squash --delete-branch
```

---

## Batch B — `Shares` service CRUD

**Branch:** `sp7/b-shares-service` off `origin/master`.
**Goal:** Full CRUD for `oc_share`: `create` (owner-check + bit-16 strip + recipient resolve), `get`, `list_outgoing`, `list_for_owner_path`, `list_incoming` (group-aware), `update`, `delete` (owner = revoke, recipient = self-unshare via `accepted=0`). Per-dialect integration tests cover every case.

**Spec sections:** §5 (full service surface + invariants), §9 (auth/permission rules the service enforces).

**Pattern to mirror for SQL:** `crates/crabcloud-filecache/src/propagate.rs` — three constants per query (`_QM` for sqlite/mysql, `_PG` for postgres), dialect-routed at call site via `pool.dialect()`. Row deserialization is generic over `sqlx::Row` to share one function across the three row types.

### Task B1: SQL constants

**Files:**
- Modify: `crates/crabcloud-sharing/src/sql.rs`

- [ ] **Step 1: Write constants**

Define the following `pub(crate) const` strings (verbatim layouts in spec §5 § OCS surface, but for SQL):
- `SELECT_BY_ID_{QM,PG}`
- `SELECT_OUTGOING_{QM,PG}` (ordered by `id`)
- `SELECT_FOR_OWNER_AND_SOURCE_{QM,PG}`
- `DELETE_BY_ID_{QM,PG}`
- `UNACCEPT_BY_ID_{QM,PG}` — `UPDATE oc_share SET accepted = 0 WHERE id = ?` / `$1`
- `INSERT_QM` (positional placeholders; no RETURNING — call `last_insert_rowid()` or `last_insert_id()` on the result)
- `INSERT_PG` — same columns, `$1..$16` placeholders, `RETURNING id`
- `UPDATE_PERMISSIONS_{QM,PG}`
- `UPDATE_EXPIRATION_{QM,PG}`

All select queries name the same 17 columns in order: `id, share_type, share_with, uid_owner, uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, stime, accepted, expiration, token, password, mail_send`.

- [ ] **Step 2: Add `select_incoming(group_count, dialect) -> String`**

Builds the dynamic IN-list query for `list_incoming`. Placeholder style differs by dialect: `?` × N for qm; `$1, $2, …` for pg (the recipient uid uses `$1`, groups start at `$2`).

```rust
pub(crate) fn select_incoming(group_count: usize, dialect: crabcloud_db::Dialect) -> String {
    // Build: "SELECT ... WHERE accepted = 1 AND share_type IN (0,1) AND (
    //         (share_type=0 AND share_with=?) OR (share_type=1 AND share_with IN (?,?,?))
    //        ) ORDER BY id"
    // Postgres variant numbers placeholders ($1 for the user, $2.. for groups).
}
```

- [ ] **Step 3: Commit**

```bash
git commit -am "sp7(b): SQL constants for Shares CRUD"
```

### Task B2: `Shares::create`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`

- [ ] **Step 1: Implement `create`**

Policy (lifted from spec §5):
1. `share_type == Link` → `NotImplemented`.
2. `permissions & 1 == 0` → `BadPermissions`.
3. `perms = SharePermissions::from_bitmask_strip_share(permissions)`.
4. Look up `path` in requester's home filecache. Missing → `PathNotOwned`. Storage_id mismatch → `ReshareRejected`.
5. Verify recipient exists (`users.exists` for user; `users.groups().exists` for group). Otherwise `RecipientUnknown`.
6. Compute `item_type` from filecache mime (`httpd/unix-directory` ⇒ Folder, else File).
7. `file_target = format!("/{}", basename(&path))`.
8. Insert row with `accepted=1`, `stime=now`, `uid_initiator = uid_owner = requester`.
9. Return constructed `ShareRow` with the new `id`.

Insert path differs by dialect — `INSERT_QM` then `.last_insert_id()`, vs `INSERT_PG` with `RETURNING id`. Two-arm match on `self.pool.dialect()`.

**Verification note:** the exact accessor for the pool (`self.pool.any()`, `self.pool.executor()`, etc.) and the exact `Users` / `Filecache` method names follow what the existing crates expose. Before running, grep `crates/crabcloud-{users,filecache,db}/src` for the relevant function shapes. The spec specifies the conceptual API; rename to match the canonical names.

- [ ] **Step 2: Confirm it compiles**

```bash
cargo check -p crabcloud-sharing
```

- [ ] **Step 3: Commit**

```bash
git commit -am "sp7(b): Shares::create + dialect-routed insert"
```

### Task B3: Per-dialect integration test harness

**Files:**
- Create: `crates/crabcloud-sharing/tests/sharing_e2e.rs`
- Create: `crates/crabcloud-sharing/tests/common/mod.rs`

- [ ] **Step 1: Common fixture**

In `tests/common/mod.rs`, define:
```rust
pub enum FixtureKind { Sqlite, MySql, Postgres }
pub struct Fixture { pub pool: Arc<DbPool>, pub users: Arc<Users>, pub filecache: Arc<Filecache>, pub shares: Shares }
impl Fixture { pub async fn new(k: FixtureKind) -> Self { /* ... */ } }
pub async fn seed_user(fx: &Fixture, uid: &str) { /* Users::create */ }
pub async fn seed_file(fx: &Fixture, uid: &str, path: &str, is_dir: bool) -> i64 { /* Filecache::upsert_path */ }
pub fn share_request(req: &str, p: &str, st: ShareType, with: &str, perms: u32) -> CreateShareRequest { /* ... */ }
```
Pool builders come from `crabcloud_config::test_support` (mirror `crabcloud-db/tests/migrate_end_to_end.rs`).

- [ ] **Step 2: Tests for `create`**

In `tests/sharing_e2e.rs`, define each scenario as a `async fn _(fx: &Fixture)`, then wrap each dialect's runner with `#[tokio::test]` (sqlite) and `#[tokio::test] #[ignore = "needs docker / testcontainers"]` (mysql, postgres). Scenarios:
- `create_user_share_happy_path` — alice→bob with perms=3 returns row with bits 1|2.
- `rejects_bit_one_cleared` — perms=2 → `BadPermissions`.
- `strips_bit_16` — perms=0x1F (31) → stored bits = 0x0F (15).
- `rejects_reshare_attempt` — alice shares /X with bob; bob attempts to share /X with carol → `ReshareRejected` (or `PathNotOwned`).
- `rejects_link_share_type` — `ShareType::Link` → `NotImplemented`.
- `rejects_unknown_recipient` — share_with="nobody" → `RecipientUnknown`.

- [ ] **Step 3: Run sqlite tests**

```bash
cargo test -p crabcloud-sharing --test sharing_e2e
```

- [ ] **Step 4: Run mysql + postgres**

```bash
cargo test -p crabcloud-sharing --test sharing_e2e -- --include-ignored
```

- [ ] **Step 5: Commit**

```bash
git commit -am "sp7(b): integration tests for Shares::create across dialects"
```

### Task B4: `get`, `list_outgoing`, `list_for_owner_path`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-sharing/tests/sharing_e2e.rs`

- [ ] **Step 1: Implement readers**

`get(id)` — `fetch_optional` against `SELECT_BY_ID_*`; `Some(row).map(row_from)`.

`list_outgoing(owner)` — `fetch_all` against `SELECT_OUTGOING_*`.

`list_for_owner_path(owner, path)` — first look up the filecache row for `(home_sid, path)`; missing → `NotFound`. Then `SELECT_FOR_OWNER_AND_SOURCE_*` with `(owner, fileid)`. Returns all shares whose `file_source == fileid` and `uid_owner == owner` (may be multiple recipients of the same path).

`row_from<R: sqlx::Row>(row) -> Result<ShareRow, ShareError>` — generic deserializer. Read columns by name. Convert `share_type i16` via `ShareType::try_from`, `item_type` via `ItemType::from_db_str`, `permissions i32` via `SharePermissions::from_bitmask_strip_share`, `accepted i16` to bool, `expiration NaiveDateTime` to `DateTime<Utc>`.

- [ ] **Step 2: Tests**

- `get_returns_the_inserted_share` — create then get; ids match.
- `get_returns_none_for_unknown_id` — id=999999 → None.
- `list_outgoing_returns_each_share_alice_created` — two creates, two results.
- `list_for_owner_path_filters_by_source` — three creates across two paths; list for /X returns two.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p crabcloud-sharing --test sharing_e2e
git commit -am "sp7(b): Shares::get + list_outgoing + list_for_owner_path"
```

### Task B5: `list_incoming` (group-aware)

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-sharing/tests/sharing_e2e.rs`

- [ ] **Step 1: Implement**

```rust
pub async fn list_incoming(&self, recipient: &UserId) -> Result<Vec<ShareRow>, ShareError> {
    let groups = self.users.groups().groups_of(recipient).await.map_err(ShareError::from_db)?;
    let sql_text = sql::select_incoming(groups.len(), self.pool.dialect());
    let mut q = sqlx::query(&sql_text).bind(recipient.as_str());
    for g in &groups { q = q.bind(g.clone()); }
    let rows = q.fetch_all(self.pool.any()).await?;
    rows.into_iter().map(row_from).collect()
}
```

- [ ] **Step 2: Tests**

- `list_incoming_returns_user_shares` — share user→user; recipient sees it.
- `list_incoming_returns_group_shares` — share user→group; recipient who is a member sees it.
- `list_incoming_skips_unaccepted_rows` — flip `accepted=0` via Batch B6's delete; recipient sees nothing. (Cross-task; this assertion lives with the B6 tests.)

- [ ] **Step 3: Run + commit**

```bash
cargo test -p crabcloud-sharing --test sharing_e2e
git commit -am "sp7(b): Shares::list_incoming (user + group)"
```

### Task B6: `update`, `delete`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-sharing/tests/sharing_e2e.rs`

- [ ] **Step 1: `update`**

```rust
pub async fn update(&self, id: i64, requester: &UserId, fields: UpdateShareFields) -> Result<ShareRow, ShareError> {
    let existing = self.get(id).await?.ok_or(ShareError::NotFound)?;
    if existing.uid_owner != requester.as_str() { return Err(ShareError::Forbidden); }
    if fields.password.is_some() || fields.note.is_some() { return Err(ShareError::NotImplemented); }
    if let Some(raw) = fields.permissions {
        if raw & 1 == 0 { return Err(ShareError::BadPermissions); }
        let perms = SharePermissions::from_bitmask_strip_share(raw);
        // run UPDATE_PERMISSIONS_*
    }
    if let Some(date) = fields.expire_date {
        let naive = date.map(|d| d.and_hms_opt(0, 0, 0).unwrap());
        // run UPDATE_EXPIRATION_*
    }
    self.get(id).await?.ok_or(ShareError::NotFound)
}
```

- [ ] **Step 2: `delete`**

```rust
pub async fn delete(&self, id: i64, requester: &UserId) -> Result<(), ShareError> {
    let row = self.get(id).await?.ok_or(ShareError::NotFound)?;
    let is_owner = row.uid_owner == requester.as_str();
    let is_direct = matches!(
        (row.share_type, row.share_with.as_deref()),
        (ShareType::User, Some(s)) if s == requester.as_str()
    );
    let is_group_recipient = if let (ShareType::Group, Some(g)) = (row.share_type, row.share_with.as_deref()) {
        self.users.groups().groups_of(requester).await
            .map_err(ShareError::from_db)?.iter().any(|x| x == g)
    } else { false };

    if is_owner {
        // DELETE_BY_ID_*
    } else if is_direct || is_group_recipient {
        if !row.accepted { return Err(ShareError::NotFound); }
        // UNACCEPT_BY_ID_*
    } else {
        return Err(ShareError::Forbidden);
    }
    Ok(())
}
```

- [ ] **Step 3: Tests**

- `update_permissions_owner_can_flip_bits` — flip from 3 to 11 (1|2|8) succeeds.
- `update_rejects_non_owner` — bob PUTs → `Forbidden`.
- `delete_owner_removes_row` — after delete, `get` returns None.
- `delete_recipient_flips_accepted` — after recipient delete, row still exists with accepted=false; second delete → `NotFound`.
- `delete_third_party_forbidden` — eve → `Forbidden`.

Plus the deferred `list_incoming_skips_unaccepted_rows` from B5.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p crabcloud-sharing --test sharing_e2e
git commit -am "sp7(b): Shares::update + Shares::delete"
```

### Task B7: Multi-dialect runs

- [ ] **Step 1: Add mysql + postgres wrappers**

For each `_works_on_sqlite` test, add `_works_on_mysql` and `_works_on_postgres` variants marked `#[ignore = "needs docker / testcontainers"]`. Each calls the same scenarios against a different `FixtureKind`.

- [ ] **Step 2: Run**

```bash
cargo test -p crabcloud-sharing -- --include-ignored
```

- [ ] **Step 3: Commit**

```bash
git commit -am "sp7(b): multidialect coverage for the Shares service"
```

### Task B8: PR

Pre-PR sweep + push + open PR. Title: `sp7: batch B — Shares service CRUD`. Merge after CI green.

---

## Batch C — `Mount.metadata`, `SharedSubrootStorage`, `ShareMountResolver`

**Branch:** `sp7/c-mount-wrapper` off `origin/master`.
**Goal:** `Mount` gains an optional `metadata` field. New `SharedSubrootStorage` wraps an owner's storage with subroot translation + permission filtering. New `ShareMountResolver` composes `HomeMountResolver` with `Shares` + `Filecache`. Server binary swap happens in Batch D.

**Spec sections:** §3.4 (permission enforcement table), §6 (code shapes for both), §10 #6 (filecache-missing fallback).

### Task C1: Extend `Mount`

**Files:**
- Modify: `crates/crabcloud-fs/src/mount.rs`
- Modify: `crates/crabcloud-fs/Cargo.toml` — add `crabcloud-sharing.workspace = true`.
- Modify: every call site that constructs `Mount` literally (`grep -rn 'Mount {' crates`).

- [ ] **Step 1: Add `MountKind` + `MountMetadata` + field**

In `mount.rs`:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind { Home, Share }

#[derive(Clone, Debug)]
pub struct MountMetadata {
    pub kind: MountKind,
    pub owner_uid: Option<String>,
    pub permissions: Option<crabcloud_sharing::SharePermissions>,
}
```
On `Mount`: `pub metadata: Option<MountMetadata>`.

- [ ] **Step 2: Fix existing constructions**

`cargo build --workspace`. For each error, add `metadata: None,` to the `Mount { ... }` literal. Expected: `resolver/mod.rs`'s `HomeMountResolver`, plus any test fixtures.

- [ ] **Step 3: Verify + commit**

```bash
cargo test --workspace --lib
git commit -am "sp7(c): Mount.metadata + MountKind + MountMetadata"
```

### Task C2: `SharedSubrootStorage`

**Files:**
- Create: `crates/crabcloud-fs/src/storage/mod.rs` (if absent — add `pub mod share_subroot;`)
- Create: `crates/crabcloud-fs/src/storage/share_subroot.rs`

- [ ] **Step 1: Implement**

Use the §6 `SharedSubrootStorage` block verbatim. Invariants:
- `id()` returns `inner.id()` unchanged.
- `translate(p) = owner_path.join(p)`.
- `write(existing=true)` → bit 2; `write(existing=false)` → bit 4.
- `mkdir` → bit 4. `delete` → bit 8. `move_` → bit 2.
- `read`, `list`, `head` pass through.

- [ ] **Step 2: Unit tests with `MemoryStorage`**

Seed `/Vacation Photos/x.jpg` in `MemoryStorage`. Wrap at owner_path `/Vacation Photos` + perms 3. Assert:
1. `list("/")` returns the wrapped contents.
2. `write("/x.jpg", existing=true)` succeeds.
3. `write("/new.jpg", existing=false)` → `PermissionDenied`.
4. `mkdir("/sub")` → `PermissionDenied`.
5. `delete("/x.jpg")` → `PermissionDenied`.
6. Re-wrap with `0x0F` (full SP7-allowed bits); previous denied ops now succeed.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p crabcloud-fs --lib storage::share_subroot
git commit -am "sp7(c): SharedSubrootStorage + permission-filter tests"
```

### Task C3: `ShareMountResolver`

**Files:**
- Create: `crates/crabcloud-fs/src/resolver/share.rs`
- Modify: `crates/crabcloud-fs/src/resolver/mod.rs` — `pub mod share; pub use share::ShareMountResolver;`

- [ ] **Step 1: Implement**

Use the §6 `ShareMountResolver` block. Steps:
1. `home.mounts_for(uid)` first.
2. `shares.list_incoming(uid)` for each row:
   - Look up owner's current path via `filecache.path_for_fileid(owner_storage_id, row.item_source)`. None → `tracing::warn!` + continue.
   - Compute unique mount name (basename of `row.file_target`; collision suffix `(2)`, `(3)`, …).
   - Push `Mount { path_prefix, storage: Arc::new(SharedSubrootStorage::new(...)), metadata: Some(MountMetadata { kind: Share, owner_uid, permissions }) }`.

- [ ] **Step 2: Unit tests with fakers**

Define small in-test traits `FakeShares: list_incoming` and `FakeFilecache: path_for_fileid` so resolver tests don't need real DB. Scenarios:
- Home has `/Photos`; incoming share named `Photos` → mount renamed to `Photos (2)`.
- Two incoming shares with the same basename → `Photos`, `Photos (2)`, `Photos (3)`.
- `path_for_fileid` returns `None` → mount skipped, home unaffected.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p crabcloud-fs --lib resolver::share
git commit -am "sp7(c): ShareMountResolver + collision-suffix logic"
```

### Task C4: PR

Pre-PR sweep + push + PR. Title: `sp7: batch C — mount wrapper + share resolver`. Merge after CI green.

---

## Batch D — OCS endpoints + AppState wiring

**Branch:** `sp7/d-ocs-api` off `origin/master`.
**Goal:** Five OCS endpoints under `/ocs/v2.php/apps/files_sharing/api/v1/shares` are live. `AppState.shares` populated. Server binary swaps to `ShareMountResolver` so incoming shares appear in DAV too.

**Spec sections:** §7 (OCS surface — request/response shapes + error codes), §9 (auth/permission semantics).

**OCS handler pattern to mirror:** `crates/crabcloud-http/src/routes/ocs/admin_users.rs` — envelope helpers, `format=json` negotiation, `AuthContext` extraction, `axum::Form` body extraction.

### Task D1: Wire `Shares` into `AppState`

**Files:**
- Modify: `crates/crabcloud-core/Cargo.toml` — `crabcloud-sharing.workspace = true`.
- Modify: `crates/crabcloud-core/src/state.rs` — `AppState.shares: Arc<Shares>`; builder constructs it.

- [ ] **Step 1: Add `pub shares: Arc<crabcloud_sharing::Shares>` to `AppState`**

In `AppStateBuilder::build()` (or wherever the existing services are constructed), after `users` + `filecache` exist:
```rust
let shares = Arc::new(crabcloud_sharing::Shares::new(
    pool.clone(),
    users.clone(),
    filecache.clone(),
));
```
Include `shares` in the `AppState { ... }` literal.

- [ ] **Step 2: Fix any direct `AppState { ... }` literals in tests**

Search `AppState {` outside `state.rs`. Most tests go through the builder — only direct literals need editing.

- [ ] **Step 3: Verify + commit**

```bash
cargo test --workspace --lib
git commit -am "sp7(d): wire Shares into AppState"
```

### Task D2: OCS module + `POST /shares`

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs` — `pub mod files_sharing;` + nest under `/apps/files_sharing/api/v1`.
- Modify: `crates/crabcloud-http/Cargo.toml` — add `crabcloud-sharing` workspace dep.
- Create: `crates/crabcloud-http/tests/files_sharing_e2e.rs`

- [ ] **Step 1: Module scaffolding**

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shares", post(create_handler).get(list_handler))
        .route("/shares/{id}", get(get_handler).put(update_handler).delete(delete_handler))
}
```

Helpers:
- `share_to_json(row: &ShareRow, state: &AppState) -> serde_json::Value` — wire shape per spec §7 (`id` is stringified, `share_type` is the i16, `share_with_displayname` / `displayname_owner` from `state.users.display_name`).
- `ocs_envelope(status: u16, message: &str, data: serde_json::Value) -> Response` — wraps in `{ ocs: { meta: { status, statuscode, message }, data } }` and chooses XML vs JSON by `format` query param.
- `from_share_error(err: ShareError) -> Response` — `ocs_envelope(err.http_status(), &err.to_string(), Value::Null)`.

- [ ] **Step 2: `create_handler`**

```rust
#[derive(Deserialize)]
struct CreateShareForm {
    path: String,
    #[serde(rename = "shareType")] share_type: i16,
    #[serde(rename = "shareWith")] share_with: Option<String>,
    permissions: u32,
}

async fn create_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Form(form): Form<CreateShareForm>,
) -> Response {
    let st = match ShareType::try_from(form.share_type) {
        Ok(st) => st,
        Err(_) => return from_share_error(ShareError::InvalidShareType),
    };
    let req = CreateShareRequest {
        requester: ctx.user_id.as_str().to_string(),
        path: form.path,
        share_type: st,
        share_with: form.share_with.unwrap_or_default(),
        permissions: form.permissions,
    };
    match state.shares.create(req).await {
        Ok(row) => ocs_envelope(200, "ok", share_to_json(&row, &state)),
        Err(e) => from_share_error(e),
    }
}
```

- [ ] **Step 3: Handler test**

In `files_sharing_e2e.rs`:
1. Build state via `AppStateBuilder` (`cfg.filecache.enabled = false`).
2. Create alice + bob via `state.users.create(...)`.
3. Seed `/X` in alice's filecache.
4. Build router; `oneshot` `POST /ocs/v2.php/apps/files_sharing/api/v1/shares?format=json` with Bearer alice's token + `Content-Type: application/x-www-form-urlencoded` + body `path=/X&shareType=0&shareWith=bob&permissions=3`.
5. Decode response as `serde_json::Value`. Assert `ocs.meta.statuscode == 200`, `ocs.data.share_with == "bob"`, `ocs.data.permissions == 3`.

- [ ] **Step 4: Commit**

```bash
git commit -am "sp7(d): OCS POST /shares + handler test"
```

### Task D3: `GET /shares` (list + by id)

- [ ] **Step 1: `list_handler`**

```rust
#[derive(Deserialize)]
struct ListQuery {
    path: Option<String>,
    shared_with_me: Option<bool>,
    subfiles: Option<bool>,
}

async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<ListQuery>,
) -> Response {
    if q.subfiles.unwrap_or(false) { return from_share_error(ShareError::NotImplemented); }
    let rows = if let Some(path) = q.path {
        state.shares.list_for_owner_path(&ctx.user_id, &path).await
    } else if q.shared_with_me.unwrap_or(false) {
        state.shares.list_incoming(&ctx.user_id).await
    } else {
        state.shares.list_outgoing(&ctx.user_id).await
    };
    match rows {
        Ok(rs) => {
            let arr = serde_json::Value::Array(rs.iter().map(|r| share_to_json(r, &state)).collect());
            ocs_envelope(200, "ok", arr)
        }
        Err(e) => from_share_error(e),
    }
}
```

- [ ] **Step 2: `get_handler`**

Look up row. If None → 404 envelope. Otherwise authorize: owner OR direct recipient OR (group share + requester in that group) OR admin (`ctx.is_admin`). If unauthorized → 404 envelope (avoids existence leak).

- [ ] **Step 3: Tests**

- `GET /shares?path=/X&format=json` as alice returns one entry.
- `GET /shares?shared_with_me=true&format=json` as bob returns the share.
- `GET /shares?subfiles=true&format=json` → 501.
- `GET /shares/{id}&format=json` works for owner + recipient, 404 for third party.

- [ ] **Step 4: Commit**

```bash
git commit -am "sp7(d): OCS GET /shares (list + single)"
```

### Task D4: `PUT` + `DELETE`

- [ ] **Step 1: `update_handler`**

```rust
#[derive(Deserialize)]
struct UpdateShareForm {
    permissions: Option<u32>,
    #[serde(rename = "expireDate")] expire_date: Option<String>,
    password: Option<String>,
    note: Option<String>,
}

async fn update_handler(...) -> Response {
    let expire = expire_date.map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d"))
        .transpose().map_err(|_| ShareError::BadPermissions)?; // 400 if malformed
    let fields = UpdateShareFields {
        permissions,
        expire_date: expire.map(Some),
        password: password.map(Some),
        note,
    };
    match state.shares.update(id, &ctx.user_id, fields).await { ... }
}
```

- [ ] **Step 2: `delete_handler`**

`state.shares.delete(id, &ctx.user_id).await` → envelope.

- [ ] **Step 3: Tests**

- PUT permissions flip; subsequent GET shows new bits.
- PUT as non-owner → 403.
- PUT with `password=...` → 501.
- DELETE owner → row gone (GET 404).
- DELETE recipient → row exists with accepted=false; list_outgoing still shows it.
- DELETE third party → 403.

- [ ] **Step 4: Commit**

```bash
git commit -am "sp7(d): OCS PUT + DELETE /shares/{id}"
```

### Task D5: Server binary swap

**Files:**
- Modify: `crates/crabcloud-server/src/main.rs`
- Modify: `crates/crabcloud-server/Cargo.toml` — add `crabcloud-sharing.workspace = true`.

- [ ] **Step 1: Swap the resolver**

Find where the existing `HomeMountResolver::new(storage_factory.clone())` is constructed. Replace with:
```rust
let resolver = Arc::new(ShareMountResolver::new(
    HomeMountResolver::new(storage_factory.clone()),
    state.shares.clone(),
    storage_factory.clone(),
    state.filecache.clone(),
));
```
Then pass `resolver` to the `View` constructor (or wherever the resolver is consumed).

- [ ] **Step 2: DAV-level test**

Append to `files_sharing_e2e.rs`:
- alice POSTs a share with perms=3 (read+write).
- bob `PROPFIND /dav/files/bob/` (Depth: 1) returns a response with one of the entries matching the share path.
- bob `PUT /dav/files/bob/<share-name>/new.txt` with body → 201/204.
- Recreate share with perms=1; bob's PUT returns 403.

- [ ] **Step 3: Commit**

```bash
git commit -am "sp7(d): swap to ShareMountResolver — shares visible via DAV"
```

### Task D6: PR

Pre-PR sweep + push + PR. Title: `sp7: batch D — OCS sharing API + mount resolver wired`. Merge after CI green.

---

## Batch E — UI surface

**Branch:** `sp7/e-ui` off `origin/master`.
**Goal:** Files UI's row `⋯` menu gets a Share entry; `ShareModal` (recipient autocomplete + permission toggles + current-shares list); sidebar "Shared with you" chip; row badges for `shared_by` + `share_count`. DTOs extended.

**Spec sections:** §8 (UI mockups + components), §3.2 (mount→DTO decoration).

### Task E1: Extend `FileEntry` DTO + helper

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns/files.rs`
- Modify: `crates/crabcloud-sharing/src/service.rs` — add `share_counts_for`.
- Modify: server-fn integration test for `list_dir`.

- [ ] **Step 1: Add fields**

In `FileEntry`:
```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub shared_by: Option<String>,
#[serde(default)]
pub share_count: i64,
```

- [ ] **Step 2: `Shares::share_counts_for`**

```rust
pub async fn share_counts_for(&self, owner: &UserId, fileids: &[i64]) -> Result<HashMap<i64, i64>, ShareError>
```
Builds a dialect-appropriate `SELECT file_source, COUNT(*) FROM oc_share WHERE uid_owner = ? AND file_source IN (?, ?, ?) GROUP BY file_source`. Empty input → empty map. Unit test on sqlite.

- [ ] **Step 3: Populate `shared_by` from mount metadata**

In `list_dir`, the View must expose the resolved mount list to the server fn. If it doesn't already, extend `View::list` (or the helper `list_dir` calls) to return `(entries, mount_snapshot)` where `mount_snapshot: Vec<(StoragePath, Option<MountMetadata>)>`. For each entry at the root level whose full user path matches a mount's `path_prefix`, set `entry.shared_by = mount.metadata.and_then(|m| m.owner_uid.clone())`.

- [ ] **Step 4: Populate `share_count`**

After the per-entry loop, collect `fileids`, call `state.shares.share_counts_for(&uid, &fileids).await?`, decorate.

- [ ] **Step 5: Tests**

- Alice lists a folder with one shared subfolder → `share_count == 1` on that row.
- Bob lists his root after alice shares with him → `shared_by == "alice"` on the share-mount row.

- [ ] **Step 6: Commit**

```bash
git commit -am "sp7(e): extend FileEntry with shared_by + share_count"
```

### Task E2 + E3: Share menu entry + `ShareModal`

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/row.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Create: `crates/crabcloud-ui/src/pages/files/share_modal.rs`
- Modify: `crates/crabcloud-ui/src/server_fns/files.rs` — add `share_recipient_search`.

- [ ] **Step 1: Open-modal signal in `FilesRoute`**

In `pages/files/mod.rs`, add `let share_path = use_signal(|| Option::<String>::None);`. Conditionally render `share_modal::ShareModal { path: ..., on_close: ... }` when `share_path.read().is_some()`.

- [ ] **Step 2: Menu entry**

In `row.rs`'s `⋯` menu rsx, append after Delete:
```rust
button {
    class: "row-menu-item",
    onclick: move |_| open_share_modal.call(row.path.clone()),
    "🔗  Share"
}
```
`open_share_modal: EventHandler<String>` is passed in from `FilesRoute` and writes to `share_path`.

- [ ] **Step 3: `share_recipient_search` server fn**

```rust
#[server(endpoint = "api/files/share_recipient_search", prefix = "")]
pub async fn share_recipient_search(q: String) -> Result<Vec<RecipientCandidate>, ServerFnError> {
    let state = require_state().await?;
    let _uid = require_user().await?;
    if q.is_empty() { return Ok(vec![]); }
    let users = state.users.search(&q, 10).await?;
    let groups = state.users.groups().search(&q, 10).await?;
    let mut out: Vec<RecipientCandidate> = users.into_iter()
        .map(|u| RecipientCandidate { id: u.uid.clone(), display_name: u.display_name, kind: "user".into(), share_type_int: 0 })
        .collect();
    out.extend(groups.into_iter().map(|g| RecipientCandidate { id: g.gid.clone(), display_name: g.gid, kind: "group".into(), share_type_int: 1 }));
    out.truncate(10);
    Ok(out)
}
```
If `Users::search` / `Groups::search` don't exist, add them — small SQL `WHERE LOWER(uid) LIKE LOWER(? || '%') OR LOWER(display_name) LIKE LOWER(? || '%') LIMIT N`. Don't add fuzzy matching; prefix-match is enough for MVP.

- [ ] **Step 4: `ShareModal` component**

Modal chrome reuses `delete_modal.rs`'s backdrop+panel. Inside the panel:

**Recipient picker** — input bound to `query: Signal<String>`. A `use_future` watches `query`; after a 250ms debounce, fetch `share_recipient_search(query)`. Render results below the input. Clicking a result sets `selected: Signal<Option<RecipientCandidate>>`. The Add button (enabled when `selected.is_some()`) POSTs:
```rust
let body = format!("path={}&shareType={}&shareWith={}&permissions=3",
    urlencoding::encode(&path),
    selected.share_type_int,
    urlencoding::encode(&selected.id));
gloo_net::http::Request::post("/ocs/v2.php/apps/files_sharing/api/v1/shares?format=json")
    .header("OCS-APIRequest", "true")
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(body).send().await.unwrap();
shares_resource.restart();
```

**Current shares list** — `use_resource` calls `GET /ocs/v2.php/apps/files_sharing/api/v1/shares?path={path}&format=json`. Renders each row:
- Recipient name (use `share_with_displayname`).
- "Can edit" checkbox bound to `(permissions & 6) != 0`. Toggling fires `PUT` with new bitmask (`existing | 6` to set, `existing & !6` to clear; always keep bit 1).
- "Can delete" checkbox bound to `(permissions & 8) != 0`. Same.
- `✕` → `DELETE /shares/{id}`.
After any mutation, `shares_resource.restart()`.

CSS additions in `crates/crabcloud-ui/assets/app.css`: `.share-modal`, `.share-modal-recipient-input`, `.share-modal-row`, `.row-menu-item` (if not already there).

- [ ] **Step 5: Run + commit**

```bash
cd crates/crabcloud-ui && dx build --release && cd ../..
# Sanity-check in a browser: open Share modal, autocomplete works, share added.
git commit -am "sp7(e): ShareModal + recipient autocomplete server fn"
```

### Task E4: "Shared with you" sidebar chip

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/chrome.rs`

- [ ] **Step 1: Add entry**

In the sidebar list, insert:
```rust
let incoming = use_resource(move || async { count_incoming_shares().await });
let enabled = matches!(incoming.read().as_ref(), Some(Ok(n)) if *n > 0);
li {
    class: if enabled { "sidebar-link" } else { "sidebar-link sidebar-link-disabled" },
    onclick: enabled.then(|| move |_| navigator().push("/apps/files/")),
    "🌐 Shared with you"
}
```
Add `count_incoming_shares() -> i64` server fn that returns `state.shares.list_incoming(&uid).await?.len() as i64`.

- [ ] **Step 2: Commit**

```bash
git commit -am "sp7(e): Shared with you sidebar chip"
```

### Task E5: Row badges

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/row.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Render**

In the row rsx, after the name span:
```rust
if let Some(by) = &entry.shared_by {
    span { class: "row-shared-by", "(shared by {by})" }
}
if entry.share_count > 0 {
    span { class: "row-share-chip", "🔗 {entry.share_count}" }
}
```

- [ ] **Step 2: CSS**

```css
.row-shared-by { color: var(--muted); font-size: 0.85em; margin-left: 0.5em; }
.row-share-chip { background: var(--chip-bg); border-radius: 1em; padding: 0 0.5em; font-size: 0.85em; margin-left: 0.5em; }
```

- [ ] **Step 3: Commit + PR**

```bash
git commit -am "sp7(e): row badges for shared_by + share_count"
```
Push + PR. Title: `sp7: batch E — UI for sharing`. Merge after CI green.

---

## Batch F — e2e, screenshot, polish

**Branch:** `sp7/f-tests-polish` off `origin/master`.
**Goal:** Playwright covers sharing happy path + permission-denied + revoke. New share-modal screenshot. Carry-forward tests for spec §10 #1 and #6.

### Task F1: Playwright sharing tests

**Files:**
- Create: `e2e/tests/sharing.spec.ts`
- Modify: `.github/workflows/ci.yml` — seed bob (and alice if not seeded) before `serve`.

- [ ] **Step 1: Seed extra users in CI**

After "Migrate" but before "Start server", add:
```yaml
- name: Bootstrap test users
  run: |
    for u in alice bob; do
      printf 'hunter2\nhunter2\n' | cargo run --release -p crabcloud-server -- --config config/e2e.toml user-add --uid "$u"
    done
```
(Verify the `user-add` subcommand's exact flag set in `crates/crabcloud-server/src/cli.rs` — it reads password from prompt, the example above pipes via stdin.)

- [ ] **Step 2: Tests**

In `e2e/tests/sharing.spec.ts`:
- `alice shares /X with bob → bob sees it at root with badge` — two browser contexts (alice + bob), share via the modal, verify bob's view.
- `bob cannot upload to a read-only share` — alice creates with `permissions=1`, bob attempts an upload, expect error toast / 403.
- `alice revokes → bob's share-mount disappears after reload` — alice clicks the `✕` in the current-shares list; bob reloads; the mount is gone.

Helper extraction: factor `loginInBrowser(page, uid, password)` into `e2e/tests/helpers.ts` if not already there (look in `files.spec.ts`).

- [ ] **Step 3: Run + commit**

```bash
cd e2e && npm test -- sharing.spec.ts && cd ..
git commit -am "sp7(f): playwright sharing scenarios"
```

### Task F2: Share-modal screenshot

**Files:**
- Modify: `e2e/screenshots.ts`
- Create: `docs/screenshots/share-modal.png`

- [ ] **Step 1: Extend the flow**

After the last existing screenshot:
1. Open ⋯ on a seeded folder.
2. Click Share.
3. Wait for `.share-modal` to be visible.
4. Type "bo" in the recipient input.
5. Wait for the autocomplete dropdown.
6. Screenshot to `docs/screenshots/share-modal.png`.

- [ ] **Step 2: Run + commit**

```bash
cd e2e && npm run screenshots && cd ..
git add docs/screenshots/share-modal.png e2e/screenshots.ts
git commit -m "sp7(f): share-modal screenshot"
```

### Task F3: Storage-event propagation test

Per spec §10 #1.

**Files:**
- Create: `crates/crabcloud-fs/tests/share_propagation.rs`

- [ ] **Step 1: Test**

- Build alice's storage with `/Vacation/x.jpg` + filecache row.
- Create a share to bob.
- Delete `/Vacation/x.jpg` through alice's home mount (the deletion propagates via the existing StorageEvent channel into the filecache).
- Assert: filecache row for `(alice_sid, /Vacation/x.jpg)` is gone.
- Assert: bob's `View::list("/Vacation Photos")` returns no entry for `x.jpg`.

- [ ] **Step 2: Run + commit**

```bash
cargo test -p crabcloud-fs --test share_propagation
git commit -am "sp7(f): storage-event propagation across share boundary"
```

### Task F4: Resolver-skips-missing-row test

Per spec §10 #6.

**Files:**
- Modify: `crates/crabcloud-fs/src/resolver/share.rs#tests`

- [ ] **Step 1: Test**

- Construct a `ShareRow` whose `item_source` does NOT correspond to any filecache entry (use a faker that returns `None`).
- Call `mounts_for(recipient)`.
- Assert: returned list has length 1 (only home mount).
- Assert: a `tracing::warn!` line was emitted (capture via a test subscriber or use `tracing_test`).

- [ ] **Step 2: Run + commit**

```bash
cargo test -p crabcloud-fs --lib resolver::share
git commit -am "sp7(f): resolver skips share when filecache row is missing"
```

### Task F5: PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd e2e && npm test && cd ..
```

Push + PR. Title: `sp7: batch F — sharing tests + screenshot + polish`. Merge after CI green.

---

## Closing checklist

After Batch F merges:

- [ ] Every acceptance criterion from spec §10 passes on master.
- [ ] (Manual) Nextcloud desktop client (3.x) against the local server: log in as alice, create + revoke a share with bob; confirm bob's client picks up the change.
- [ ] Write `docs/superpowers/specs/2026-05-13-sharing-user-group-and-virtual-mount-design.followup-sp8.md` capturing carry-forward notes for SP8 (public links, password protection, file-drop, expiration-enforcement for user/group shares if a user pushed for it).
- [ ] Update the program memory file (`memory/project_rustcloud_program.md` if it exists; otherwise the team-facing program-status doc) to flip SP7 → done, SP8 → next.
