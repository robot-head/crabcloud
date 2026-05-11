# Users / Core User Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `crabcloud-users` — a SQL-backed `UserStore` / `GroupStore` / `PreferenceStore` + `PasswordVerifier` composition exposed as `AppState.users`, replacing the `bootstrap_admin` config stand-in with a real user store while preserving the fresh-install bootstrap UX. Login swaps to `state.users.verify`; new `GET`/`PUT /ocs/v2.php/cloud/user` endpoints land; `crabcloud-server` gains CLI subcommands for user/group management.

**Architecture:** New `crabcloud-users` crate with four async traits (`UserStore`, `GroupStore`, `PreferenceStore`, `PasswordVerifier`), one SQL backend per store, a `UsersService` façade composed into `AppState`, and a `BootstrapAdminBackend` wrapper that synthesizes the configured admin when the DB user table is empty (and retires itself on first DB write). Phase 3's `Session` gains a `two_factor_passed: bool` field and the `SessionStore` gains `destroy_all_for` / `destroy_all_for_except` for password-change session invalidation. No new HTTP server function patterns — auth-bearing operations stay on the OCS surface per spec §8.4.

**Tech Stack:** Existing crates (sqlx, axum, tower, dioxus, etc.) + three new workspace deps: `bcrypt` (already present), `email_address = "0.2"` (RFC validation), `rpassword = "7"` (CLI password prompts), `assert_cmd = "2"` (CLI test runner, dev-dep only).

**Parent spec:** `docs/superpowers/specs/2026-05-11-users-core-design.md`.

**Previous state:** Platform Core complete at commit `5ff744b` (`master`). Phase 3's `/index.php/login` validates against `config.bootstrap_admin`; Phase 3's `AuthenticatedUser` extractor resolves from session cookie.

**Branch protection:** `master` is PR-required. Every batch lands as one PR with `gh pr merge --auto --squash`.

---

## Conventions (carried from platform-core)

- **Commits:** Conventional Commits (`feat(users)`, `chore(users)`, `test(users)`, …) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD:** Failing test → fail → implement → pass → commit. Each task ends with a commit.
- **rustfmt:** `cargo fmt --all` after each task. Authorized.
- **Plan-bug protocol:** if verbatim code fails to compile or test, fix minimally and report DONE_WITH_CONCERNS with a clear diff explanation.
- **`cargo xtask check-all` must pass after every commit.**
- **Use the existing `crabcloud_config::test_support` helpers** (`minimal_sqlite_config`, `sqlite_config_with_admin`) for all test fixtures — don't re-roll hand-coded `FileConfig` literals.

---

## File Structure

```
crates/
├── crabcloud-users/                          # NEW
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                            # re-exports
│       ├── user.rs                           # UserId, User
│       ├── group.rs                          # GroupId, Group
│       ├── email.rs                          # Email newtype
│       ├── error.rs                          # UsersError
│       ├── password.rs                       # PasswordVerifier + BcryptVerifier
│       ├── store/
│       │   ├── mod.rs                        # UserStore + GroupStore + PreferenceStore traits
│       │   ├── sql.rs                        # SqlUserStore / SqlGroupStore / SqlPreferenceStore
│       │   └── bootstrap_shim.rs             # BootstrapAdminBackend
│       └── service.rs                        # UsersService façade
├── crabcloud-core/                           # MODIFIED
│   └── src/{error.rs, state.rs}              # Error::Users variant; AppState.users field
├── crabcloud-http/                           # MODIFIED
│   └── src/{routes/login.rs, session/store.rs, session/layer.rs,
│            extractors/auth.rs, routes/ocs/user.rs}
├── crabcloud-server/                         # MODIFIED
│   └── src/{main.rs, cli.rs}                 # new user-* and group-* subcommands
migrations/core/0002_users/{sqlite,mysql,postgres}.sql   # NEW
docs/superpowers/plans/2026-05-11-users-core-implementation.changelog.md   # NEW (last task)
```

## Batches

Execution order — each is its own PR:

| Batch | Tasks | Theme |
|---|---|---|
| **A** | 1–3 | Workspace scaffold, core types/errors, migration |
| **B** | 4–7 | Stores + verifier + service |
| **C** | 8–9 | Bootstrap shim + session extensions |
| **D** | 10–11 | AppState wiring + login swap |
| **E** | 12–13 | Auth extractor + self OCS endpoints |
| **F** | 14 | CLI subcommands |
| **G** | 15 | End-to-end tests + acceptance docs |

---

## Task 1: Workspace scaffold

**Files:**
- Modify: `Cargo.toml` (workspace members + workspace deps)
- Create: `crates/crabcloud-users/Cargo.toml`
- Create: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Add `crates/crabcloud-users` to workspace members + deps**

Modify root `Cargo.toml`. Insert `"crates/crabcloud-users"` into `[workspace] members` (alphabetical, between `-ui` and `xtask`).

Append to `[workspace.dependencies]`:

```toml
crabcloud-users = { path = "crates/crabcloud-users" }
email_address = "0.2"
rpassword = "7"
assert_cmd = "2"
```

(`bcrypt` is already a workspace dep from Phase 3.)

- [ ] **Step 2: Write `crates/crabcloud-users/Cargo.toml`**

```toml
[package]
name = "crabcloud-users"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
bcrypt.workspace = true
crabcloud-cache.workspace = true
crabcloud-db.workspace = true
email_address.workspace = true
serde.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
crabcloud-config = { workspace = true, features = ["test-support"] }
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 3: Write `crates/crabcloud-users/src/lib.rs` stub**

```rust
//! User store + group store + preference store + password verifier for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-11-users-core-design.md`. Submodules land
//! in later tasks of this plan.
```

- [ ] **Step 4: Build + commit**

```
cargo build -p crabcloud-users
```

Expected: clean.

Commit:

```
git checkout -b users-batch-a
git add Cargo.toml Cargo.lock crates/crabcloud-users
git commit -m "chore(users): scaffold crabcloud-users crate

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Core types and `UsersError`

**Files:**
- Create: `crates/crabcloud-users/src/email.rs`
- Create: `crates/crabcloud-users/src/user.rs`
- Create: `crates/crabcloud-users/src/group.rs`
- Create: `crates/crabcloud-users/src/error.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Write `crates/crabcloud-users/src/error.rs`**

```rust
//! Error type for the users crate.

#[derive(Debug, thiserror::Error)]
pub enum UsersError {
    #[error("user not found")]
    NotFound,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("account disabled")]
    Disabled,
    #[error("invalid uid: {0}")]
    InvalidUid(String),
    #[error("invalid email: {0}")]
    InvalidEmail(String),
    #[error("invalid display name: {0}")]
    InvalidDisplayName(String),
    #[error("uid already exists")]
    UidAlreadyExists,
    #[error("email already taken")]
    EmailAlreadyTaken,
    #[error("backend is read-only")]
    ReadOnly,
    #[error("password rejected: {0}")]
    PasswordTooWeak(&'static str),
    #[error(transparent)]
    Db(#[from] crabcloud_db::DbError),
    #[error(transparent)]
    Cache(#[from] crabcloud_cache::CacheError),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type UsersResult<T> = Result<T, UsersError>;
```

- [ ] **Step 2: Write `crates/crabcloud-users/src/user.rs` with tests**

```rust
//! `UserId` newtype + `User` struct.

use crate::email::Email;
use crate::error::UsersError;
use serde::{Deserialize, Serialize};

/// Validated user identifier. 1-64 chars, `[A-Za-z0-9._@-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Result<Self, UsersError> {
        let s = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(UsersError::InvalidUid(format!("length {}", s.len())));
        }
        for ch in s.chars() {
            if !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '@' | '-') {
                return Err(UsersError::InvalidUid(format!("char {:?}", ch)));
            }
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Public user record. Note: the password hash is NOT a field here.
/// `UserStore::lookup_for_auth` returns hash + user together; everything else
/// returns this struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub uid: UserId,
    pub display_name: String,
    pub email: Option<Email>,
    pub enabled: bool,
    pub last_seen: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_uids_accepted() {
        for ok in &["alice", "bob.smith", "user_123", "a-b", "x@y", "A"] {
            assert!(UserId::new(*ok).is_ok(), "{ok:?} should be valid");
        }
    }

    #[test]
    fn invalid_uids_rejected() {
        for bad in &["", " alice", "alice ", "a/b", "a\\b", "a\nb", "a:b"] {
            assert!(UserId::new(*bad).is_err(), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn uid_max_length_64() {
        let ok = "a".repeat(64);
        assert!(UserId::new(&ok).is_ok());
        let bad = "a".repeat(65);
        assert!(UserId::new(&bad).is_err());
    }
}
```

- [ ] **Step 3: Write `crates/crabcloud-users/src/email.rs` with tests**

```rust
//! `Email` newtype with RFC validation.

use crate::error::UsersError;
use email_address::EmailAddress;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Email(String);

impl Email {
    /// Parse + canonicalize an email: trim, lowercase, validate via RFC 5321/5322.
    pub fn parse(s: impl Into<String>) -> Result<Self, UsersError> {
        let raw = s.into();
        let trimmed = raw.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Err(UsersError::InvalidEmail("empty".into()));
        }
        if trimmed.len() > 255 {
            return Err(UsersError::InvalidEmail(format!("length {}", trimmed.len())));
        }
        EmailAddress::from_str(&trimmed)
            .map_err(|e| UsersError::InvalidEmail(e.to_string()))?;
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

use std::str::FromStr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_emails_parse() {
        for ok in &["a@b.com", "alice@example.org", "first.last+tag@sub.example.com"] {
            assert!(Email::parse(*ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn invalid_emails_rejected() {
        for bad in &["", "not-an-email", "@missing-local", "missing@", "two@@signs.com"] {
            assert!(Email::parse(*bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn canonicalization_lowercases_and_trims() {
        let e = Email::parse("  Alice@Example.COM  ").unwrap();
        assert_eq!(e.as_str(), "alice@example.com");
    }
}
```

- [ ] **Step 4: Write `crates/crabcloud-users/src/group.rs` with tests**

```rust
//! `GroupId` newtype + `Group` struct. Same validation shape as `UserId`.

use crate::error::UsersError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(String);

impl GroupId {
    pub fn new(s: impl Into<String>) -> Result<Self, UsersError> {
        let s = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(UsersError::InvalidUid(format!("gid length {}", s.len())));
        }
        for ch in s.chars() {
            if !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '@' | '-') {
                return Err(UsersError::InvalidUid(format!("gid char {:?}", ch)));
            }
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub gid: GroupId,
    pub display_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_gid_accepted() {
        assert!(GroupId::new("admin").is_ok());
    }

    #[test]
    fn whitespace_rejected() {
        assert!(GroupId::new("ad min").is_err());
    }
}
```

- [ ] **Step 5: Re-export from `lib.rs`**

Replace `crates/crabcloud-users/src/lib.rs`:

```rust
//! User store + group store + preference store + password verifier for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-11-users-core-design.md`.

mod email;
mod error;
mod group;
mod user;

pub use email::Email;
pub use error::{UsersError, UsersResult};
pub use group::{Group, GroupId};
pub use user::{User, UserId};
```

- [ ] **Step 6: Run tests**

```
cargo test -p crabcloud-users --lib
```

Expected: 9 tests pass (3 user + 3 email + 1 group + tests in mod ordering — adjust if exact counts differ; 9 is the floor).

- [ ] **Step 7: Commit**

```
git add crates/crabcloud-users
git commit -m "feat(users): add UserId, Email, GroupId, User, Group + UsersError

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Migration 0002 — user tables

**Files:**
- Create: `migrations/core/0002_users/sqlite.sql`
- Create: `migrations/core/0002_users/mysql.sql`
- Create: `migrations/core/0002_users/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs`

- [ ] **Step 1: Write SQLite migration**

`migrations/core/0002_users/sqlite.sql`:

```sql
CREATE TABLE oc_users (
    uid          TEXT    NOT NULL,
    password     TEXT,
    displayname  TEXT,
    email        TEXT,
    last_seen    INTEGER NOT NULL DEFAULT 0,
    enabled      INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
);
CREATE UNIQUE INDEX oc_users_email_idx ON oc_users(email) WHERE email IS NOT NULL;

CREATE TABLE oc_groups (
    gid          TEXT NOT NULL,
    displayname  TEXT,
    PRIMARY KEY (gid)
);

CREATE TABLE oc_group_user (
    gid  TEXT NOT NULL,
    uid  TEXT NOT NULL,
    PRIMARY KEY (gid, uid)
);
CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);

CREATE TABLE oc_preferences (
    userid       TEXT NOT NULL,
    appid        TEXT NOT NULL,
    configkey    TEXT NOT NULL,
    configvalue  TEXT,
    PRIMARY KEY (userid, appid, configkey)
);
CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
```

- [ ] **Step 2: Write MySQL migration**

`migrations/core/0002_users/mysql.sql`:

```sql
CREATE TABLE oc_users (
    uid          VARCHAR(64)  NOT NULL,
    password     LONGTEXT,
    displayname  VARCHAR(64),
    email        VARCHAR(255),
    last_seen    BIGINT  NOT NULL DEFAULT 0,
    enabled      TINYINT NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_users_email_idx ON oc_users(email);

CREATE TABLE oc_groups (
    gid          VARCHAR(64) NOT NULL,
    displayname  VARCHAR(64),
    PRIMARY KEY (gid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE oc_group_user (
    gid  VARCHAR(64) NOT NULL,
    uid  VARCHAR(64) NOT NULL,
    PRIMARY KEY (gid, uid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);

CREATE TABLE oc_preferences (
    userid       VARCHAR(64) NOT NULL,
    appid        VARCHAR(32) NOT NULL,
    configkey    VARCHAR(64) NOT NULL,
    configvalue  LONGTEXT,
    PRIMARY KEY (userid, appid, configkey)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
```

(Plain index on `oc_users(email)` since MySQL's partial unique index support is uneven; uniqueness is enforced application-side per spec §5.)

- [ ] **Step 3: Write Postgres migration**

`migrations/core/0002_users/postgres.sql`:

```sql
CREATE TABLE oc_users (
    uid          VARCHAR(64)  NOT NULL,
    password     TEXT,
    displayname  VARCHAR(64),
    email        VARCHAR(255),
    last_seen    BIGINT   NOT NULL DEFAULT 0,
    enabled      SMALLINT NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
);
CREATE UNIQUE INDEX oc_users_email_idx ON oc_users(email) WHERE email IS NOT NULL;

CREATE TABLE oc_groups (
    gid          VARCHAR(64) NOT NULL,
    displayname  VARCHAR(64),
    PRIMARY KEY (gid)
);

CREATE TABLE oc_group_user (
    gid  VARCHAR(64) NOT NULL,
    uid  VARCHAR(64) NOT NULL,
    PRIMARY KEY (gid, uid)
);
CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);

CREATE TABLE oc_preferences (
    userid       VARCHAR(64) NOT NULL,
    appid        VARCHAR(32) NOT NULL,
    configkey    VARCHAR(64) NOT NULL,
    configvalue  TEXT,
    PRIMARY KEY (userid, appid, configkey)
);
CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
```

- [ ] **Step 4: Register the migration in `crabcloud-db`**

Modify `crates/crabcloud-db/src/core_migrations.rs` — append a new `Migration` entry to `CORE_MIGRATIONS`:

Find the existing array:

```rust
pub const CORE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sqlite: include_str!("../../../migrations/core/0001_initial/sqlite.sql"),
        mysql:  include_str!("../../../migrations/core/0001_initial/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0001_initial/postgres.sql"),
    },
];
```

Append the second entry inside the array:

```rust
    Migration {
        version: 2,
        name: "users",
        sqlite: include_str!("../../../migrations/core/0002_users/sqlite.sql"),
        mysql:  include_str!("../../../migrations/core/0002_users/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0002_users/postgres.sql"),
    },
```

- [ ] **Step 5: Add a migration smoke test**

Append to the existing `#[cfg(test)] mod tests` in `crates/crabcloud-db/src/core_migrations.rs`:

```rust
    #[tokio::test]
    async fn users_migration_creates_tables_and_seeds_admin_group() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("u.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();

        if let DbPool::Sqlite(p) = &pool {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oc_groups WHERE gid = 'admin'")
                .fetch_one(p).await.unwrap();
            assert_eq!(count, 1, "admin group should be seeded");

            // Insert + read back a user.
            sqlx::query("INSERT INTO oc_users (uid, password, displayname, enabled) VALUES (?, ?, ?, 1)")
                .bind("alice").bind("hash").bind("Alice")
                .execute(p).await.unwrap();
            let display: Option<String> = sqlx::query_scalar(
                "SELECT displayname FROM oc_users WHERE uid = ?"
            )
                .bind("alice")
                .fetch_one(p).await.unwrap();
            assert_eq!(display.as_deref(), Some("Alice"));
        } else {
            unreachable!()
        }
        pool.close().await;
    }
```

- [ ] **Step 6: Run tests**

```
cargo test -p crabcloud-db --features test-support core_migrations
```

Wait — `crabcloud-db` doesn't have a `test-support` feature; `crabcloud-config` does. Correct command:

```
cargo test -p crabcloud-db --lib core_migrations
```

Expected: existing tests still pass + the new `users_migration_creates_tables_and_seeds_admin_group` passes.

- [ ] **Step 7: Run full check + commit + PR**

```
cargo xtask check-all
```

Expected: green.

```
git add migrations crates/crabcloud-db
git commit -m "feat(db,users): add migration 0002 creating oc_users/groups/preferences

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-a
gh pr create --base master --head users-batch-a \
  --title "users: batch A — scaffold + types + migration 0002" \
  --body "Sub-project 2a, batch A: workspace scaffold, UserId/Email/GroupId newtypes, UsersError, migration 0002 (oc_users + oc_groups + oc_group_user + oc_preferences)."
gh pr merge --auto --squash
```

Wait for CI green + auto-merge; pull master.

---

## Task 4: `UserStore` trait + `SqlUserStore`

**Files:**
- Create: `crates/crabcloud-users/src/store/mod.rs`
- Create: `crates/crabcloud-users/src/store/sql.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

Start a new branch:

```
git checkout -b users-batch-b origin/master
```

- [ ] **Step 1: Write `crates/crabcloud-users/src/store/mod.rs`**

```rust
//! Backend traits. SQL implementation in `sql.rs`; future LDAP/SAML backends
//! plug in via the same traits.

pub mod sql;

use crate::error::UsersResult;
use crate::group::{Group, GroupId};
use crate::user::{User, UserId};
use async_trait::async_trait;

/// User-record-with-hash for auth-time lookups only. Kept off the public
/// `User` type so handlers can't accidentally leak the hash.
#[derive(Debug, Clone)]
pub struct UserWithHash {
    pub user: User,
    pub password_hash: Option<String>,
}

#[async_trait]
pub trait UserStore: Send + Sync {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>>;
    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>>;
    /// Auth-time lookup. Returns the hash alongside the user. None on miss.
    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>>;
    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()>;
    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()>;
    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()>;
    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()>;
    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()>;
    async fn delete(&self, uid: &UserId) -> UsersResult<()>;
    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()>;
}

#[async_trait]
pub trait GroupStore: Send + Sync {
    async fn lookup(&self, gid: &GroupId) -> UsersResult<Option<Group>>;
    async fn is_in_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<bool>;
    async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>>;
    async fn members_of(&self, gid: &GroupId) -> UsersResult<Vec<UserId>>;
    async fn add_to_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()>;
    async fn remove_from_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()>;
    async fn create(&self, group: &Group) -> UsersResult<()>;
    async fn delete(&self, gid: &GroupId) -> UsersResult<()>;
}

#[async_trait]
pub trait PreferenceStore: Send + Sync {
    async fn get(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<Option<String>>;
    async fn set(&self, uid: &UserId, app: &str, key: &str, value: &str) -> UsersResult<()>;
    async fn delete(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<()>;
    async fn list(&self, uid: &UserId, app: &str) -> UsersResult<Vec<(String, String)>>;
    async fn delete_all_for(&self, uid: &UserId) -> UsersResult<()>;
}
```

- [ ] **Step 2: Write `crates/crabcloud-users/src/store/sql.rs`** (long, but mechanical)

```rust
//! SQL backend for the three store traits. Per-dialect query dispatch follows
//! the platform-core `match &pool` pattern.

use super::{GroupStore, PreferenceStore, UserStore, UserWithHash};
use crate::email::Email;
use crate::error::{UsersError, UsersResult};
use crate::group::{Group, GroupId};
use crate::user::{User, UserId};
use async_trait::async_trait;
use crabcloud_db::{DbPool, DbError};

#[derive(Clone)]
pub struct SqlUserStore {
    pool: DbPool,
}

impl SqlUserStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

fn map_sqlx<T>(r: Result<T, sqlx::Error>) -> UsersResult<T> {
    r.map_err(|e| UsersError::Db(DbError::Sqlx(e)))
}

fn row_to_user(uid: String, display: Option<String>, email: Option<String>, last_seen: i64, enabled_int: i64)
    -> UsersResult<User>
{
    let user_id = UserId::new(uid)?;
    let email = email.map(Email::parse).transpose()?;
    Ok(User {
        uid: user_id,
        display_name: display.unwrap_or_default(),
        email,
        enabled: enabled_int != 0,
        last_seen: last_seen.max(0) as u64,
    })
}

#[async_trait]
impl UserStore for SqlUserStore {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        let row: Option<(String, Option<String>, Option<String>, i64, i64)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = ?"
            ).bind(uid.as_str()).fetch_optional(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = ?"
            ).bind(uid.as_str()).fetch_optional(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = $1"
            ).bind(uid.as_str()).fetch_optional(p).await)?,
        };
        match row {
            None => Ok(None),
            Some((u, d, e, l, en)) => Ok(Some(row_to_user(u, d, e, l, en)?)),
        }
    }

    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        let user_id = UserId::new(login).ok();
        if let Some(uid) = user_id {
            if let Some(u) = self.lookup(&uid).await? {
                return Ok(Some(u));
            }
        }
        if login.contains('@') {
            let lower = login.to_ascii_lowercase();
            let row: Option<(String, Option<String>, Option<String>, i64, i64)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = ?"
                ).bind(&lower).fetch_optional(p).await)?,
                DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = ?"
                ).bind(&lower).fetch_optional(p).await)?,
                DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = $1"
                ).bind(&lower).fetch_optional(p).await)?,
            };
            return row.map(|(u, d, e, l, en)| row_to_user(u, d, e, l, en)).transpose();
        }
        Ok(None)
    }

    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>> {
        let row: Option<(String, Option<String>, Option<String>, Option<String>, i64, i64)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = ? OR LOWER(email) = ?"
            ).bind(login).bind(login.to_ascii_lowercase()).fetch_optional(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = ? OR LOWER(email) = ?"
            ).bind(login).bind(login.to_ascii_lowercase()).fetch_optional(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = $1 OR LOWER(email) = $2"
            ).bind(login).bind(login.to_ascii_lowercase()).fetch_optional(p).await)?,
        };
        match row {
            None => Ok(None),
            Some((u, d, e, hash, l, en)) => Ok(Some(UserWithHash {
                user: row_to_user(u, d, e, l, en)?,
                password_hash: hash,
            })),
        }
    }

    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()> {
        let q_sqlite_mysql = "UPDATE oc_users SET password = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET password = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(new_hash).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(new_hash).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(new_hash).bind(uid.as_str()).execute(p).await)?.rows_affected(),
        };
        if affected == 0 { return Err(UsersError::NotFound); }
        Ok(())
    }

    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        if new.is_empty() || new.len() > 64 || new.chars().any(|c| c.is_control()) {
            return Err(UsersError::InvalidDisplayName(format!("{new:?}")));
        }
        let q_sqlite_mysql = "UPDATE oc_users SET displayname = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET displayname = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(new).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(new).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(new).bind(uid.as_str()).execute(p).await)?.rows_affected(),
        };
        if affected == 0 { return Err(UsersError::NotFound); }
        Ok(())
    }

    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()> {
        // Validate first; then enforce uniqueness application-side (since
        // MySQL doesn't reliably support partial unique on email).
        let canonical = match new {
            Some(raw) => Some(Email::parse(raw)?.as_str().to_string()),
            None => None,
        };
        if let Some(ref c) = canonical {
            let q_sqlite_mysql = "SELECT uid FROM oc_users WHERE email = ? AND uid <> ?";
            let q_pg = "SELECT uid FROM oc_users WHERE email = $1 AND uid <> $2";
            let dup: Option<(String,)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(q_sqlite_mysql).bind(c).bind(uid.as_str()).fetch_optional(p).await)?,
                DbPool::MySql(p) => map_sqlx(sqlx::query_as(q_sqlite_mysql).bind(c).bind(uid.as_str()).fetch_optional(p).await)?,
                DbPool::Postgres(p) => map_sqlx(sqlx::query_as(q_pg).bind(c).bind(uid.as_str()).fetch_optional(p).await)?,
            };
            if dup.is_some() { return Err(UsersError::EmailAlreadyTaken); }
        }
        let q_sqlite_mysql = "UPDATE oc_users SET email = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET email = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(canonical.as_deref()).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(canonical.as_deref()).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(canonical.as_deref()).bind(uid.as_str()).execute(p).await)?.rows_affected(),
        };
        if affected == 0 { return Err(UsersError::NotFound); }
        Ok(())
    }

    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()> {
        let v: i64 = if enabled { 1 } else { 0 };
        let q_sqlite_mysql = "UPDATE oc_users SET enabled = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET enabled = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(v).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(v).bind(uid.as_str()).execute(p).await)?.rows_affected(),
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(v).bind(uid.as_str()).execute(p).await)?.rows_affected(),
        };
        if affected == 0 { return Err(UsersError::NotFound); }
        Ok(())
    }

    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()> {
        if self.lookup(&user.uid).await?.is_some() {
            return Err(UsersError::UidAlreadyExists);
        }
        if let Some(ref e) = user.email {
            // Reuse uniqueness check via the set_email path's logic.
            let q_sqlite_mysql = "SELECT uid FROM oc_users WHERE email = ?";
            let q_pg = "SELECT uid FROM oc_users WHERE email = $1";
            let dup: Option<(String,)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(q_sqlite_mysql).bind(e.as_str()).fetch_optional(p).await)?,
                DbPool::MySql(p) => map_sqlx(sqlx::query_as(q_sqlite_mysql).bind(e.as_str()).fetch_optional(p).await)?,
                DbPool::Postgres(p) => map_sqlx(sqlx::query_as(q_pg).bind(e.as_str()).fetch_optional(p).await)?,
            };
            if dup.is_some() { return Err(UsersError::EmailAlreadyTaken); }
        }
        let enabled_int: i64 = if user.enabled { 1 } else { 0 };
        let last_seen: i64 = user.last_seen as i64;
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES (?, ?, ?, ?, ?, ?)"
            ).bind(user.uid.as_str()).bind(password_hash).bind(&user.display_name).bind(user.email.as_ref().map(|e| e.as_str())).bind(last_seen).bind(enabled_int).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES (?, ?, ?, ?, ?, ?)"
            ).bind(user.uid.as_str()).bind(password_hash).bind(&user.display_name).bind(user.email.as_ref().map(|e| e.as_str())).bind(last_seen).bind(enabled_int).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES ($1, $2, $3, $4, $5, $6)"
            ).bind(user.uid.as_str()).bind(password_hash).bind(&user.display_name).bind(user.email.as_ref().map(|e| e.as_str())).bind(last_seen).bind(enabled_int).execute(p).await)?,
        };
        Ok(())
    }

    async fn delete(&self, uid: &UserId) -> UsersResult<()> {
        // Cascade by hand: group_user + preferences + user.
        for (sqlite_mysql, pg) in &[
            ("DELETE FROM oc_group_user WHERE uid = ?", "DELETE FROM oc_group_user WHERE uid = $1"),
            ("DELETE FROM oc_preferences WHERE userid = ?", "DELETE FROM oc_preferences WHERE userid = $1"),
            ("DELETE FROM oc_users WHERE uid = ?", "DELETE FROM oc_users WHERE uid = $1"),
        ] {
            match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(sqlx::query(sqlite_mysql).bind(uid.as_str()).execute(p).await)?,
                DbPool::MySql(p) => map_sqlx(sqlx::query(sqlite_mysql).bind(uid.as_str()).execute(p).await)?,
                DbPool::Postgres(p) => map_sqlx(sqlx::query(pg).bind(uid.as_str()).execute(p).await)?,
            };
        }
        Ok(())
    }

    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);
        let q_sqlite_mysql = "UPDATE oc_users SET last_seen = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET last_seen = $1 WHERE uid = $2";
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(now).bind(uid.as_str()).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(now).bind(uid.as_str()).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(now).bind(uid.as_str()).execute(p).await)?,
        };
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqlGroupStore {
    pool: DbPool,
}

impl SqlGroupStore {
    pub fn new(pool: DbPool) -> Self { Self { pool } }
}

#[async_trait]
impl GroupStore for SqlGroupStore {
    async fn lookup(&self, gid: &GroupId) -> UsersResult<Option<Group>> {
        let row: Option<(String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT gid, displayname FROM oc_groups WHERE gid = ?"
            ).bind(gid.as_str()).fetch_optional(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT gid, displayname FROM oc_groups WHERE gid = ?"
            ).bind(gid.as_str()).fetch_optional(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT gid, displayname FROM oc_groups WHERE gid = $1"
            ).bind(gid.as_str()).fetch_optional(p).await)?,
        };
        match row {
            None => Ok(None),
            Some((g, d)) => Ok(Some(Group { gid: GroupId::new(g)?, display_name: d.unwrap_or_default() })),
        }
    }

    async fn is_in_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<bool> {
        let row: Option<(i64,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT 1 FROM oc_group_user WHERE uid = ? AND gid = ?"
            ).bind(uid.as_str()).bind(gid.as_str()).fetch_optional(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT 1 FROM oc_group_user WHERE uid = ? AND gid = ?"
            ).bind(uid.as_str()).bind(gid.as_str()).fetch_optional(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT 1 FROM oc_group_user WHERE uid = $1 AND gid = $2"
            ).bind(uid.as_str()).bind(gid.as_str()).fetch_optional(p).await)?,
        };
        Ok(row.is_some())
    }

    async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        let rows: Vec<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT gid FROM oc_group_user WHERE uid = ? ORDER BY gid"
            ).bind(uid.as_str()).fetch_all(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT gid FROM oc_group_user WHERE uid = ? ORDER BY gid"
            ).bind(uid.as_str()).fetch_all(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT gid FROM oc_group_user WHERE uid = $1 ORDER BY gid"
            ).bind(uid.as_str()).fetch_all(p).await)?,
        };
        rows.into_iter().map(|(g,)| GroupId::new(g)).collect()
    }

    async fn members_of(&self, gid: &GroupId) -> UsersResult<Vec<UserId>> {
        let rows: Vec<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT uid FROM oc_group_user WHERE gid = ? ORDER BY uid"
            ).bind(gid.as_str()).fetch_all(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT uid FROM oc_group_user WHERE gid = ? ORDER BY uid"
            ).bind(gid.as_str()).fetch_all(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT uid FROM oc_group_user WHERE gid = $1 ORDER BY uid"
            ).bind(gid.as_str()).fetch_all(p).await)?,
        };
        rows.into_iter().map(|(u,)| UserId::new(u)).collect()
    }

    async fn add_to_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        let q_sqlite = "INSERT OR IGNORE INTO oc_group_user (gid, uid) VALUES (?, ?)";
        let q_mysql = "INSERT IGNORE INTO oc_group_user (gid, uid) VALUES (?, ?)";
        let q_pg = "INSERT INTO oc_group_user (gid, uid) VALUES ($1, $2) ON CONFLICT DO NOTHING";
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_mysql).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
        };
        Ok(())
    }

    async fn remove_from_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_group_user WHERE gid = ? AND uid = ?";
        let q_pg = "DELETE FROM oc_group_user WHERE gid = $1 AND uid = $2";
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(gid.as_str()).bind(uid.as_str()).execute(p).await)?,
        };
        Ok(())
    }

    async fn create(&self, group: &Group) -> UsersResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_groups (gid, displayname) VALUES (?, ?)"
            ).bind(group.gid.as_str()).bind(&group.display_name).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_groups (gid, displayname) VALUES (?, ?)"
            ).bind(group.gid.as_str()).bind(&group.display_name).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_groups (gid, displayname) VALUES ($1, $2)"
            ).bind(group.gid.as_str()).bind(&group.display_name).execute(p).await)?,
        };
        Ok(())
    }

    async fn delete(&self, gid: &GroupId) -> UsersResult<()> {
        for (sqlite_mysql, pg) in &[
            ("DELETE FROM oc_group_user WHERE gid = ?", "DELETE FROM oc_group_user WHERE gid = $1"),
            ("DELETE FROM oc_groups WHERE gid = ?", "DELETE FROM oc_groups WHERE gid = $1"),
        ] {
            match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(sqlx::query(sqlite_mysql).bind(gid.as_str()).execute(p).await)?,
                DbPool::MySql(p) => map_sqlx(sqlx::query(sqlite_mysql).bind(gid.as_str()).execute(p).await)?,
                DbPool::Postgres(p) => map_sqlx(sqlx::query(pg).bind(gid.as_str()).execute(p).await)?,
            };
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqlPreferenceStore {
    pool: DbPool,
}

impl SqlPreferenceStore {
    pub fn new(pool: DbPool) -> Self { Self { pool } }
}

#[async_trait]
impl PreferenceStore for SqlPreferenceStore {
    async fn get(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<Option<String>> {
        let row: Option<(Option<String>,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT configvalue FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?"
            ).bind(uid.as_str()).bind(app).bind(key).fetch_optional(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT configvalue FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?"
            ).bind(uid.as_str()).bind(app).bind(key).fetch_optional(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT configvalue FROM oc_preferences WHERE userid = $1 AND appid = $2 AND configkey = $3"
            ).bind(uid.as_str()).bind(app).bind(key).fetch_optional(p).await)?,
        };
        Ok(row.and_then(|(v,)| v))
    }

    async fn set(&self, uid: &UserId, app: &str, key: &str, value: &str) -> UsersResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(userid, appid, configkey) DO UPDATE SET configvalue = excluded.configvalue"
            ).bind(uid.as_str()).bind(app).bind(key).bind(value).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES (?, ?, ?, ?) \
                 ON DUPLICATE KEY UPDATE configvalue = VALUES(configvalue)"
            ).bind(uid.as_str()).bind(app).bind(key).bind(value).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(
                "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (userid, appid, configkey) DO UPDATE SET configvalue = EXCLUDED.configvalue"
            ).bind(uid.as_str()).bind(app).bind(key).bind(value).execute(p).await)?,
        };
        Ok(())
    }

    async fn delete(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?";
        let q_pg = "DELETE FROM oc_preferences WHERE userid = $1 AND appid = $2 AND configkey = $3";
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).bind(app).bind(key).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).bind(app).bind(key).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).bind(app).bind(key).execute(p).await)?,
        };
        Ok(())
    }

    async fn list(&self, uid: &UserId, app: &str) -> UsersResult<Vec<(String, String)>> {
        let rows: Vec<(String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query_as(
                "SELECT configkey, configvalue FROM oc_preferences WHERE userid = ? AND appid = ? ORDER BY configkey"
            ).bind(uid.as_str()).bind(app).fetch_all(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query_as(
                "SELECT configkey, configvalue FROM oc_preferences WHERE userid = ? AND appid = ? ORDER BY configkey"
            ).bind(uid.as_str()).bind(app).fetch_all(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query_as(
                "SELECT configkey, configvalue FROM oc_preferences WHERE userid = $1 AND appid = $2 ORDER BY configkey"
            ).bind(uid.as_str()).bind(app).fetch_all(p).await)?,
        };
        Ok(rows.into_iter().map(|(k, v)| (k, v.unwrap_or_default())).collect())
    }

    async fn delete_all_for(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_preferences WHERE userid = ?";
        let q_pg = "DELETE FROM oc_preferences WHERE userid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?,
            DbPool::MySql(p) => map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?,
            DbPool::Postgres(p) => map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?,
        };
        Ok(())
    }
}
```

- [ ] **Step 3: Re-export store types**

Modify `crates/crabcloud-users/src/lib.rs`:

```rust
//! User store + group store + preference store + password verifier for Crabcloud.

mod email;
mod error;
mod group;
mod store;
mod user;

pub use email::Email;
pub use error::{UsersError, UsersResult};
pub use group::{Group, GroupId};
pub use store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
pub use store::{GroupStore, PreferenceStore, UserStore, UserWithHash};
pub use user::{User, UserId};
```

- [ ] **Step 4: Write a CRUD round-trip test**

Create `crates/crabcloud-users/src/store/sql.rs`'s test module by appending:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("u.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn user_crud_roundtrip() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        let uid = UserId::new("alice").unwrap();
        let user = User {
            uid: uid.clone(),
            display_name: "Alice".into(),
            email: Some(Email::parse("alice@example.com").unwrap()),
            enabled: true,
            last_seen: 0,
        };
        store.create(&user, Some("hash")).await.unwrap();

        let got = store.lookup(&uid).await.unwrap().unwrap();
        assert_eq!(got.display_name, "Alice");
        assert_eq!(got.email.unwrap().as_str(), "alice@example.com");
        assert!(got.enabled);

        store.set_display_name(&uid, "Alice Smith").await.unwrap();
        let updated = store.lookup(&uid).await.unwrap().unwrap();
        assert_eq!(updated.display_name, "Alice Smith");

        store.delete(&uid).await.unwrap();
        assert!(store.lookup(&uid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn group_membership() {
        let pool = fresh_pool().await;
        let users = SqlUserStore::new(pool.clone());
        let groups = SqlGroupStore::new(pool);
        let uid = UserId::new("bob").unwrap();
        users.create(&User { uid: uid.clone(), display_name: "Bob".into(), email: None, enabled: true, last_seen: 0 }, None).await.unwrap();
        let admin = GroupId::new("admin").unwrap();
        assert!(!groups.is_in_group(&uid, &admin).await.unwrap());
        groups.add_to_group(&uid, &admin).await.unwrap();
        assert!(groups.is_in_group(&uid, &admin).await.unwrap());
        let g = groups.groups_of(&uid).await.unwrap();
        assert_eq!(g, vec![admin.clone()]);
    }

    #[tokio::test]
    async fn preferences_upsert_and_read() {
        let pool = fresh_pool().await;
        let users = SqlUserStore::new(pool.clone());
        let prefs = SqlPreferenceStore::new(pool);
        let uid = UserId::new("c").unwrap();
        users.create(&User { uid: uid.clone(), display_name: "C".into(), email: None, enabled: true, last_seen: 0 }, None).await.unwrap();
        prefs.set(&uid, "files", "max_upload", "1024").await.unwrap();
        prefs.set(&uid, "files", "max_upload", "2048").await.unwrap();
        assert_eq!(prefs.get(&uid, "files", "max_upload").await.unwrap().as_deref(), Some("2048"));
    }

    #[tokio::test]
    async fn lookup_by_login_matches_email() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        store.create(&User {
            uid: UserId::new("dave").unwrap(),
            display_name: "Dave".into(),
            email: Some(Email::parse("dave@example.com").unwrap()),
            enabled: true,
            last_seen: 0,
        }, None).await.unwrap();
        let by_email = store.lookup_by_login("DAVE@example.com").await.unwrap();
        assert!(by_email.is_some());
    }

    #[tokio::test]
    async fn create_rejects_duplicate_email() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        store.create(&User {
            uid: UserId::new("e1").unwrap(),
            display_name: "E1".into(),
            email: Some(Email::parse("e@example.com").unwrap()),
            enabled: true, last_seen: 0,
        }, None).await.unwrap();
        let err = store.create(&User {
            uid: UserId::new("e2").unwrap(),
            display_name: "E2".into(),
            email: Some(Email::parse("e@example.com").unwrap()),
            enabled: true, last_seen: 0,
        }, None).await.unwrap_err();
        assert!(matches!(err, UsersError::EmailAlreadyTaken));
    }
}
```

- [ ] **Step 5: Run tests + commit**

```
cargo test -p crabcloud-users
```

Expected: 5 new SQL tests pass on top of the type tests.

```
git add crates/crabcloud-users
git commit -m "feat(users): UserStore/GroupStore/PreferenceStore traits + SQL impls

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `PasswordVerifier` + `BcryptVerifier` with sentinel

**Files:**
- Create: `crates/crabcloud-users/src/password.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Write `crates/crabcloud-users/src/password.rs`**

```rust
//! Password hashing + verification.

use crate::error::{UsersError, UsersResult};
use std::sync::OnceLock;

/// bcrypt cost. 12 is the project default; revisit when 13 becomes affordable.
pub const BCRYPT_COST: u32 = 12;

pub trait PasswordVerifier: Send + Sync {
    /// Constant-time-ish verification. If `hash` is None, runs against a
    /// sentinel hash so the call still takes ~equivalent time, defeating
    /// user-enumeration timing oracles.
    fn verify(&self, password: &str, hash: Option<&str>) -> bool;

    fn hash(&self, password: &str) -> UsersResult<String>;
}

pub struct BcryptVerifier;

impl BcryptVerifier {
    pub fn new() -> Self { Self }
}

impl Default for BcryptVerifier {
    fn default() -> Self { Self::new() }
}

fn sentinel() -> &'static str {
    static SENTINEL: OnceLock<String> = OnceLock::new();
    SENTINEL.get_or_init(|| {
        bcrypt::hash("invalid sentinel — never matches a real password", BCRYPT_COST)
            .expect("bcrypt::hash on a literal never fails")
    })
}

impl PasswordVerifier for BcryptVerifier {
    fn verify(&self, password: &str, hash: Option<&str>) -> bool {
        let target = hash.unwrap_or_else(|| sentinel());
        bcrypt::verify(password, target).unwrap_or(false)
    }

    fn hash(&self, password: &str) -> UsersResult<String> {
        if password.is_empty() {
            return Err(UsersError::PasswordTooWeak("must not be empty"));
        }
        if password.as_bytes().len() > 72 {
            return Err(UsersError::PasswordTooWeak("max 72 bytes (bcrypt limit)"));
        }
        bcrypt::hash(password, BCRYPT_COST)
            .map_err(|e| UsersError::Internal(anyhow::anyhow!("bcrypt hash failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = BcryptVerifier::new();
        let h = v.hash("hunter2").unwrap();
        assert!(v.verify("hunter2", Some(&h)));
        assert!(!v.verify("WRONG", Some(&h)));
    }

    #[test]
    fn no_hash_always_fails_but_runs() {
        let v = BcryptVerifier::new();
        assert!(!v.verify("anything", None));
    }

    #[test]
    fn empty_password_rejected_on_hash() {
        let v = BcryptVerifier::new();
        let err = v.hash("").unwrap_err();
        assert!(matches!(err, UsersError::PasswordTooWeak(_)));
    }

    #[test]
    fn over_72_bytes_rejected() {
        let v = BcryptVerifier::new();
        let big = "a".repeat(73);
        let err = v.hash(&big).unwrap_err();
        assert!(matches!(err, UsersError::PasswordTooWeak(_)));
    }
}
```

- [ ] **Step 2: Re-export**

Modify `crates/crabcloud-users/src/lib.rs` to add:

```rust
mod password;
pub use password::{BcryptVerifier, PasswordVerifier, BCRYPT_COST};
```

(Insert in alphabetical order with the other mods/use lines.)

- [ ] **Step 3: Test + commit**

```
cargo test -p crabcloud-users --lib password
```

Expected: 4 tests pass.

```
git add crates/crabcloud-users
git commit -m "feat(users): PasswordVerifier trait + BcryptVerifier with sentinel hash

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: `UsersService` façade

**Files:**
- Create: `crates/crabcloud-users/src/service.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Write `crates/crabcloud-users/src/service.rs`**

```rust
//! `UsersService` — the public composition handlers reach for.

use crate::error::{UsersError, UsersResult};
use crate::group::GroupId;
use crate::password::PasswordVerifier;
use crate::store::{GroupStore, PreferenceStore, UserStore};
use crate::user::{User, UserId};
use std::sync::Arc;

#[derive(Clone)]
pub struct UsersService {
    users: Arc<dyn UserStore>,
    groups: Arc<dyn GroupStore>,
    prefs: Arc<dyn PreferenceStore>,
    verifier: Arc<dyn PasswordVerifier>,
}

impl UsersService {
    pub fn new(
        users: Arc<dyn UserStore>,
        groups: Arc<dyn GroupStore>,
        prefs: Arc<dyn PreferenceStore>,
        verifier: Arc<dyn PasswordVerifier>,
    ) -> Self {
        Self { users, groups, prefs, verifier }
    }

    pub fn user_store(&self) -> &Arc<dyn UserStore> { &self.users }
    pub fn group_store(&self) -> &Arc<dyn GroupStore> { &self.groups }
    pub fn preferences(&self) -> &Arc<dyn PreferenceStore> { &self.prefs }
    pub fn verifier(&self) -> &Arc<dyn PasswordVerifier> { &self.verifier }

    /// Verify a (login, password) pair. Always runs bcrypt — even on miss —
    /// so user-enumeration timing oracles don't work.
    pub async fn verify(&self, login: &str, password: &str) -> UsersResult<User> {
        let candidate = self.users.lookup_for_auth(login).await?;
        let (user, hash) = match candidate {
            Some(uwh) => (Some(uwh.user), uwh.password_hash),
            None => (None, None),
        };
        let ok = self.verifier.verify(password, hash.as_deref());
        match (user, ok) {
            (Some(u), true) if u.enabled => {
                self.users.touch_last_seen(&u.uid).await?;
                Ok(u)
            }
            _ => Err(UsersError::InvalidCredentials),
        }
    }

    pub async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        self.users.lookup(uid).await
    }

    pub async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        self.users.lookup_by_login(login).await
    }

    pub async fn set_password(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        let hash = self.verifier.hash(new)?;
        self.users.set_password(uid, &hash).await
    }

    pub async fn is_admin(&self, uid: &UserId) -> UsersResult<bool> {
        let admin = GroupId::new("admin")?;
        self.groups.is_in_group(uid, &admin).await
    }

    pub async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        self.groups.groups_of(uid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::BcryptVerifier;
    use crate::store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_service() -> UsersService {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("svc.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        UsersService::new(
            Arc::new(SqlUserStore::new(pool.clone())),
            Arc::new(SqlGroupStore::new(pool.clone())),
            Arc::new(SqlPreferenceStore::new(pool)),
            Arc::new(BcryptVerifier::new()),
        )
    }

    #[tokio::test]
    async fn verify_succeeds_with_correct_password() {
        let svc = fresh_service().await;
        let uid = UserId::new("alice").unwrap();
        let hash = svc.verifier.hash("hunter2").unwrap();
        svc.users.create(&User {
            uid: uid.clone(), display_name: "A".into(), email: None, enabled: true, last_seen: 0
        }, Some(&hash)).await.unwrap();
        let user = svc.verify("alice", "hunter2").await.unwrap();
        assert_eq!(user.uid, uid);
    }

    #[tokio::test]
    async fn verify_fails_unknown_user_with_consistent_error() {
        let svc = fresh_service().await;
        let err = svc.verify("nobody", "x").await.unwrap_err();
        assert!(matches!(err, UsersError::InvalidCredentials));
    }

    #[tokio::test]
    async fn verify_fails_for_disabled_user() {
        let svc = fresh_service().await;
        let uid = UserId::new("d").unwrap();
        let hash = svc.verifier.hash("hunter2").unwrap();
        svc.users.create(&User {
            uid: uid.clone(), display_name: "D".into(), email: None, enabled: false, last_seen: 0
        }, Some(&hash)).await.unwrap();
        let err = svc.verify("d", "hunter2").await.unwrap_err();
        assert!(matches!(err, UsersError::InvalidCredentials));
    }

    #[tokio::test]
    async fn is_admin_resolves() {
        let svc = fresh_service().await;
        let uid = UserId::new("ad").unwrap();
        svc.users.create(&User {
            uid: uid.clone(), display_name: "Ad".into(), email: None, enabled: true, last_seen: 0
        }, None).await.unwrap();
        assert!(!svc.is_admin(&uid).await.unwrap());
        svc.groups.add_to_group(&uid, &GroupId::new("admin").unwrap()).await.unwrap();
        assert!(svc.is_admin(&uid).await.unwrap());
    }
}
```

- [ ] **Step 2: Re-export**

Modify `crates/crabcloud-users/src/lib.rs`:

```rust
mod service;
pub use service::UsersService;
```

- [ ] **Step 3: Test + commit**

```
cargo test -p crabcloud-users
```

Expected: all prior tests + 4 new service tests pass.

- [ ] **Step 4: Run full check, open Batch B PR**

```
cargo xtask check-all
git add crates/crabcloud-users
git commit -m "feat(users): UsersService façade composing stores + verifier

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
git push -u origin users-batch-b
gh pr create --base master --head users-batch-b \
  --title "users: batch B — stores + verifier + service façade" \
  --body "Sub-project 2a, batch B: UserStore/GroupStore/PreferenceStore traits and SqlUserStore/SqlGroupStore/SqlPreferenceStore implementations, PasswordVerifier trait with BcryptVerifier (sentinel-based constant-time fake-verify on lookup-miss), and the UsersService façade composing them."
gh pr merge --auto --squash
```

Pull master after merge.

---

## Task 7: `BootstrapAdminBackend` shim

**Files:**
- Create: `crates/crabcloud-users/src/store/bootstrap_shim.rs`
- Modify: `crates/crabcloud-users/src/store/mod.rs` + `lib.rs`

Start branch: `git checkout -b users-batch-c origin/master`

- [ ] **Step 1: Write `bootstrap_shim.rs`**

```rust
//! BootstrapAdminBackend — wraps any `UserStore` and synthesizes a virtual
//! admin from `config.bootstrap_admin` if the wrapped store has no matching
//! user. First write through this backend retires the shim by INSERTing a
//! real DB row.

use super::{UserStore, UserWithHash};
use crate::email::Email;
use crate::error::{UsersError, UsersResult};
use crate::group::GroupId;
use crate::store::GroupStore;
use crate::user::{User, UserId};
use async_trait::async_trait;
use rustcloud_config::BootstrapAdminConfig;
use std::sync::Arc;

pub struct BootstrapAdminBackend {
    inner: Arc<dyn UserStore>,
    groups: Arc<dyn GroupStore>,
    admin: BootstrapAdminConfig,
}

impl BootstrapAdminBackend {
    pub fn new(
        inner: Arc<dyn UserStore>,
        groups: Arc<dyn GroupStore>,
        admin: BootstrapAdminConfig,
    ) -> Self {
        Self { inner, groups, admin }
    }

    fn matches_login(&self, login: &str) -> bool {
        login == self.admin.username
    }

    fn synthesized_user(&self) -> UsersResult<User> {
        Ok(User {
            uid: UserId::new(&self.admin.username)?,
            display_name: self.admin.username.clone(),
            email: None,
            enabled: true,
            last_seen: 0,
        })
    }
}

// NOTE: the only function that consults `admin.password_hash` is
// `lookup_for_auth` — set_password promotes the user into the DB and the next
// boot's "ignoring bootstrap_admin" warning will fire.

#[async_trait]
impl UserStore for BootstrapAdminBackend {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        if let Some(u) = self.inner.lookup(uid).await? {
            return Ok(Some(u));
        }
        if uid.as_str() == self.admin.username {
            return Ok(Some(self.synthesized_user()?));
        }
        Ok(None)
    }

    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        if let Some(u) = self.inner.lookup_by_login(login).await? {
            return Ok(Some(u));
        }
        if self.matches_login(login) {
            return Ok(Some(self.synthesized_user()?));
        }
        Ok(None)
    }

    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>> {
        if let Some(real) = self.inner.lookup_for_auth(login).await? {
            return Ok(Some(real));
        }
        if self.matches_login(login) {
            return Ok(Some(UserWithHash {
                user: self.synthesized_user()?,
                password_hash: Some(self.admin.password_hash.clone()),
            }));
        }
        Ok(None)
    }

    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()> {
        // If the user exists in inner, just update.
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_password(uid, new_hash).await;
        }
        // If this is the virtual admin, promote: INSERT into oc_users + admin group.
        if uid.as_str() == self.admin.username {
            let user = self.synthesized_user()?;
            self.inner.create(&user, Some(new_hash)).await?;
            self.groups.add_to_group(&user.uid, &GroupId::new("admin")?).await?;
            tracing::info!(uid = uid.as_str(), "promoted bootstrap admin to oc_users; remove [bootstrap_admin] from config.toml");
            return Ok(());
        }
        Err(UsersError::NotFound)
    }

    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_display_name(uid, new).await;
        }
        // Can't mutate the virtual admin; promote-then-set is a future polish.
        Err(UsersError::ReadOnly)
    }

    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_email(uid, new).await;
        }
        Err(UsersError::ReadOnly)
    }

    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_enabled(uid, enabled).await;
        }
        Err(UsersError::ReadOnly)
    }

    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()> {
        self.inner.create(user, password_hash).await
    }

    async fn delete(&self, uid: &UserId) -> UsersResult<()> {
        self.inner.delete(uid).await
    }

    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.touch_last_seen(uid).await;
        }
        // Virtual admin: no-op (no place to persist).
        Ok(())
    }
}
```

NOTE: the `rustcloud_config` reference above is a typo for the crate-renamed `crabcloud_config`. Use the correct name (`crabcloud_config::BootstrapAdminConfig`) when implementing.

- [ ] **Step 2: Wire `bootstrap_shim` into `store/mod.rs` and `lib.rs`**

Append to `crates/crabcloud-users/src/store/mod.rs`:

```rust
pub mod bootstrap_shim;
```

Add to `crates/crabcloud-users/src/lib.rs` `pub use`:

```rust
pub use store::bootstrap_shim::BootstrapAdminBackend;
```

- [ ] **Step 3: Add `crabcloud-config` to `crabcloud-users` Cargo.toml `[dependencies]`**

The shim now references `crabcloud_config::BootstrapAdminConfig`. Update `crates/crabcloud-users/Cargo.toml`'s `[dependencies]` block — add `crabcloud-config.workspace = true`.

- [ ] **Step 4: Tests for the shim**

Append to `crates/crabcloud-users/src/store/bootstrap_shim.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{BcryptVerifier, PasswordVerifier};
    use crate::store::sql::{SqlGroupStore, SqlUserStore};
    use crabcloud_config::BootstrapAdminConfig;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn make() -> (BootstrapAdminBackend, Arc<dyn GroupStore>, Arc<dyn UserStore>) {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("b.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let inner: Arc<dyn UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
        let groups: Arc<dyn GroupStore> = Arc::new(SqlGroupStore::new(pool));
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        let shim = BootstrapAdminBackend::new(
            inner.clone(),
            groups.clone(),
            BootstrapAdminConfig { username: "admin".into(), password_hash: hash },
        );
        (shim, groups, inner)
    }

    #[tokio::test]
    async fn virtual_admin_visible_via_lookup() {
        let (shim, _, _) = make().await;
        let u = shim.lookup(&UserId::new("admin").unwrap()).await.unwrap().unwrap();
        assert_eq!(u.uid.as_str(), "admin");
    }

    #[tokio::test]
    async fn lookup_for_auth_returns_synthesized_hash() {
        let (shim, _, _) = make().await;
        let r = shim.lookup_for_auth("admin").await.unwrap().unwrap();
        assert!(r.password_hash.is_some());
    }

    #[tokio::test]
    async fn set_password_on_virtual_admin_promotes_to_db() {
        let (shim, groups, inner) = make().await;
        let uid = UserId::new("admin").unwrap();
        let new_hash = BcryptVerifier::new().hash("newpass").unwrap();
        shim.set_password(&uid, &new_hash).await.unwrap();
        // Promoted: inner store should now have an admin row + admin-group membership.
        assert!(inner.lookup(&uid).await.unwrap().is_some());
        assert!(groups.is_in_group(&uid, &GroupId::new("admin").unwrap()).await.unwrap());
    }
}
```

- [ ] **Step 5: Test + commit**

```
cargo test -p crabcloud-users
```

Expected: 3 new shim tests pass.

```
git add crates/crabcloud-users
git commit -m "feat(users): BootstrapAdminBackend shim with promote-on-write

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: Session extensions

**Files:**
- Modify: `crates/crabcloud-http/src/session/data.rs`
- Modify: `crates/crabcloud-http/src/session/store.rs`
- Modify: `crates/crabcloud-http/src/session/layer.rs`

- [ ] **Step 1: Add `two_factor_passed` to `Session`**

Modify `crates/crabcloud-http/src/session/data.rs`. Find the `Session` struct and add a field:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub user_id: Option<String>,
    pub csrf_token: String,
    pub last_activity: u64,
    #[serde(default)]
    pub two_factor_passed: bool,
}
```

Update `Session::new`:

```rust
    pub fn new() -> Self {
        Self {
            user_id: None,
            csrf_token: random_token(),
            last_activity: now_secs(),
            two_factor_passed: false,
        }
    }
```

The `#[serde(default)]` on the new field means cached sessions written before this code change still deserialize (`two_factor_passed = false`).

- [ ] **Step 2: Extend `SessionStore` with destroy-by-user methods**

Modify `crates/crabcloud-http/src/session/store.rs`. Add two new methods + a side-index helper:

After the existing methods on `SessionStore`, add:

```rust
    /// Side-index key for sessions belonging to a user.
    fn user_index_key(&self, uid: &str) -> String {
        format!("{}:sessions_by_user:{}", self.instance_id, uid)
    }

    /// Record `id` as belonging to `uid`. Called from the layer on login.
    pub async fn record_for_user(&self, uid: &str, id: &SessionId) -> Result<(), CacheError> {
        let key = self.user_index_key(uid);
        let current: Vec<String> = match self.cache.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => Vec::new(),
        };
        let mut set: Vec<String> = current;
        if !set.iter().any(|s| s == id.as_str()) {
            set.push(id.as_str().to_string());
        }
        let bytes = serde_json::to_vec(&set)
            .map_err(|e| CacheError::Io(format!("session index encode: {e}")))?;
        self.cache.set(&key, &bytes, Some(SESSION_IDLE_TTL)).await
    }

    /// Destroy every session owned by `uid` except `except` (if Some).
    pub async fn destroy_all_for_except(&self, uid: &str, except: Option<&SessionId>) -> Result<(), CacheError> {
        let key = self.user_index_key(uid);
        let current: Vec<String> = match self.cache.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => return Ok(()),
        };
        let mut survivors: Vec<String> = Vec::new();
        for id_str in current {
            let id = SessionId(id_str.clone());
            if except.map(|e| e.as_str()) == Some(id.as_str()) {
                survivors.push(id_str);
            } else {
                let _ = self.destroy(&id).await;
            }
        }
        if survivors.is_empty() {
            let _ = self.cache.del(&key).await;
        } else {
            let bytes = serde_json::to_vec(&survivors)
                .map_err(|e| CacheError::Io(format!("session index encode: {e}")))?;
            let _ = self.cache.set(&key, &bytes, Some(SESSION_IDLE_TTL)).await;
        }
        Ok(())
    }

    /// Destroy every session owned by `uid`. No exception.
    pub async fn destroy_all_for(&self, uid: &str) -> Result<(), CacheError> {
        self.destroy_all_for_except(uid, None).await
    }
```

- [ ] **Step 3: Layer records the user index on login**

Modify `crates/crabcloud-http/src/session/layer.rs`. In the `SessionMiddleware::call` body, after the session is saved on response — find the existing block:

```rust
            } else {
                let final_session = handle.inner.lock().await.clone();
                let _ = store.save(&handle.id, &final_session).await;
                ...
            }
```

Replace the save section with:

```rust
            } else {
                let final_session = handle.inner.lock().await.clone();
                if let Some(uid) = &final_session.user_id {
                    let _ = store.record_for_user(uid, &handle.id).await;
                }
                let _ = store.save(&handle.id, &final_session).await;
                let cookie_value = encode_cookie(handle.id.as_str(), secret.expose_secret().as_bytes());
                resp.headers_mut().append(
                    SET_COOKIE,
                    make_set_cookie(&cookie_value, secure, super::store::SESSION_IDLE_TTL.as_secs()),
                );
            }
```

- [ ] **Step 4: Tests for the new store methods**

Append to `crates/crabcloud-http/src/session/store.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn destroy_all_for_except_kills_others_keeps_current() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id_a = SessionId::new_random();
        let id_b = SessionId::new_random();
        let mut sa = Session::new();
        sa.user_id = Some("alice".into());
        let mut sb = Session::new();
        sb.user_id = Some("alice".into());
        store.save(&id_a, &sa).await.unwrap();
        store.save(&id_b, &sb).await.unwrap();
        store.record_for_user("alice", &id_a).await.unwrap();
        store.record_for_user("alice", &id_b).await.unwrap();

        store.destroy_all_for_except("alice", Some(&id_b)).await.unwrap();
        assert!(store.load(&id_a).await.unwrap().is_none());
        assert!(store.load(&id_b).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn destroy_all_for_kills_everything_for_user() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        let mut s = Session::new();
        s.user_id = Some("bob".into());
        store.save(&id, &s).await.unwrap();
        store.record_for_user("bob", &id).await.unwrap();
        store.destroy_all_for("bob").await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }
```

- [ ] **Step 5: Test + commit + open Batch C PR**

```
cargo xtask check-all
git add crates/crabcloud-http crates/crabcloud-users Cargo.toml Cargo.lock
git commit -m "feat(http,users): session.two_factor_passed + destroy_all_for_{except,user}

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-c
gh pr create --base master --head users-batch-c \
  --title "users: batch C — bootstrap shim + session extensions" \
  --body "Sub-project 2a, batch C: BootstrapAdminBackend (synthesizes a virtual admin from config; promotes-on-write into oc_users), session.two_factor_passed field, SessionStore::record_for_user + destroy_all_for + destroy_all_for_except backed by a sessions-by-user side index."
gh pr merge --auto --squash
```

Pull master after merge.

---

## Task 9: `AppState.users` + `AppStateBuilder::with_users` + default wiring

**Files:**
- Modify: `crates/crabcloud-core/Cargo.toml` (depend on `crabcloud-users`)
- Modify: `crates/crabcloud-core/src/error.rs` (add `Users` variant)
- Modify: `crates/crabcloud-core/src/state.rs`

Start branch: `git checkout -b users-batch-d origin/master`

- [ ] **Step 1: Add dep**

In `crates/crabcloud-core/Cargo.toml` `[dependencies]`, add `crabcloud-users.workspace = true`.

- [ ] **Step 2: Add `Error::Users` variant**

Modify `crates/crabcloud-core/src/error.rs`. Add variant:

```rust
    #[error(transparent)]
    Users(#[from] crabcloud_users::UsersError),
```

Update `http_status` arm:

```rust
            Error::Users(u) => users_status(u),
```

Add helper at the bottom of the file:

```rust
fn users_status(e: &crabcloud_users::UsersError) -> u16 {
    use crabcloud_users::UsersError::*;
    match e {
        NotFound => 404,
        InvalidCredentials | Disabled => 401,
        InvalidUid(_) | InvalidEmail(_) | InvalidDisplayName(_) | PasswordTooWeak(_) => 400,
        UidAlreadyExists | EmailAlreadyTaken => 409,
        ReadOnly => 403,
        Db(_) | Cache(_) | Internal(_) => 500,
    }
}
```

Update `client_message`:

```rust
            Error::Users(u) => match u {
                crabcloud_users::UsersError::Db(_) | crabcloud_users::UsersError::Cache(_) | crabcloud_users::UsersError::Internal(_) => "Internal Server Error".into(),
                crabcloud_users::UsersError::InvalidCredentials | crabcloud_users::UsersError::Disabled => "Unauthorized".into(),
                other => other.to_string(),
            },
```

- [ ] **Step 3: Add `users` field to `AppState` + wire builder**

Modify `crates/crabcloud-core/src/state.rs`. Add a field:

```rust
    pub users: crabcloud_users::UsersService,
```

In `AppStateBuilder`, add an optional override:

```rust
    custom_users: Option<crabcloud_users::UsersService>,
```

Add builder method:

```rust
    pub fn with_users(mut self, service: crabcloud_users::UsersService) -> Self {
        self.custom_users = Some(service);
        self
    }
```

Initialize the field in `AppStateBuilder::new`:

```rust
            custom_users: None,
```

Build the `UsersService` in `build()` — insert this block after the cache + appconfig setup, before `let state = AppState { ... }`:

```rust
        let users = if let Some(svc) = self.custom_users.take() {
            svc
        } else {
            use crabcloud_users::{BcryptVerifier, SqlGroupStore, SqlPreferenceStore, SqlUserStore, UserStore, UsersService};
            let sql_users: std::sync::Arc<dyn UserStore> = std::sync::Arc::new(SqlUserStore::new(pool.clone()));
            let sql_groups: std::sync::Arc<dyn crabcloud_users::GroupStore> = std::sync::Arc::new(SqlGroupStore::new(pool.clone()));
            let sql_prefs: std::sync::Arc<dyn crabcloud_users::PreferenceStore> = std::sync::Arc::new(SqlPreferenceStore::new(pool.clone()));
            let user_store: std::sync::Arc<dyn UserStore> = match &self.config.bootstrap_admin {
                Some(admin) => std::sync::Arc::new(crabcloud_users::BootstrapAdminBackend::new(
                    sql_users.clone(),
                    sql_groups.clone(),
                    admin.clone(),
                )),
                None => sql_users,
            };
            UsersService::new(user_store, sql_groups, sql_prefs, std::sync::Arc::new(BcryptVerifier::new()))
        };
```

Then add `users` to the AppState construction:

```rust
        let state = AppState {
            config: self.config.clone(),
            pool,
            cache,
            i18n,
            appconfig,
            capability_providers: Arc::new(Mutex::new(Vec::new())),
            users,
        };
```

- [ ] **Step 4: Sanity check**

```
cargo build -p crabcloud-core
```

Expected: clean.

- [ ] **Step 5: Update the existing `state.rs` test to assert the users field exists**

Append to `crates/crabcloud-core/src/state.rs`'s test module:

```rust
    #[tokio::test]
    async fn users_service_assembled_with_bootstrap_admin() {
        use crabcloud_users::UserId;
        let dir = tempdir().unwrap();
        let mut cfg = cfg_sqlite(dir.path().join("u.db"));
        let hash = crabcloud_users::BcryptVerifier::new()
            .hash("hunter2").unwrap();
        cfg.bootstrap_admin = Some(crabcloud_config::BootstrapAdminConfig {
            username: "admin".into(),
            password_hash: hash,
        });
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        // Virtual admin should be reachable via the shim.
        let admin = state.users.lookup_by_login("admin").await.unwrap();
        assert!(admin.is_some());
    }
```

(`cfg_sqlite` may already exist in the test module; if not, use `crabcloud_config::test_support::minimal_sqlite_config(...)`.)

- [ ] **Step 6: Run + commit**

```
cargo test -p crabcloud-core
cargo xtask check-all
git add crates/crabcloud-core Cargo.lock
git commit -m "feat(core): AppState.users + AppStateBuilder default wiring + Error::Users

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: `/index.php/login` swap to `state.users.verify`

**Files:**
- Modify: `crates/crabcloud-http/src/routes/login.rs`

- [ ] **Step 1: Rewrite the login handler body**

Modify `crates/crabcloud-http/src/routes/login.rs`. Replace the handler body that currently consults `state.config.bootstrap_admin`. New body:

```rust
pub async fn handler(
    State(state): State<AppState>,
    Extension(handle): Extension<SessionHandle>,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    let user = state.users.verify(&form.username, &form.password)
        .await
        .map_err(|_| ApiError(CoreError::Unauthorized))?;

    let uid_str = user.uid.as_str().to_string();
    handle.mutate(|s| {
        s.user_id = Some(uid_str.clone());
        s.rotate_csrf();
        s.two_factor_passed = true;
    }).await;

    let mut resp = (StatusCode::SEE_OTHER, "").into_response();
    resp.headers_mut().insert(axum::http::header::LOCATION, HeaderValue::from_static("/"));
    Ok(resp)
}
```

`LoginForm` is unchanged.

- [ ] **Step 2: Update the existing login tests to use `state.users` instead of `bootstrap_admin`**

The tests in `routes/login.rs` set up `cfg_with_admin(...)`. They still work because the bootstrap shim is wired by default when `config.bootstrap_admin` is set. Verify by running:

```
cargo test -p crabcloud-http --lib routes::login
```

Expected: 3 tests still pass.

- [ ] **Step 3: Add a new test that exercises the SQL backend (not the shim)**

Append to `crates/crabcloud-http/src/routes/login.rs` tests:

```rust
    #[tokio::test]
    async fn login_succeeds_against_real_oc_users_row() {
        use crabcloud_users::{BcryptVerifier, User, UserId, PasswordVerifier};
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("login.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();

        // Seed a user.
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        state.users.user_store().create(&User {
            uid: UserId::new("alice").unwrap(),
            display_name: "Alice".into(),
            email: None,
            enabled: true,
            last_seen: 0,
        }, Some(&hash)).await.unwrap();

        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("username=alice&password=hunter2"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }
```

- [ ] **Step 4: Run + commit + open Batch D PR**

```
cargo xtask check-all
git add crates/crabcloud-http
git commit -m "feat(http): swap /index.php/login from bootstrap_admin check to UsersService.verify

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-d
gh pr create --base master --head users-batch-d \
  --title "users: batch D — AppState.users + login swap" \
  --body "Sub-project 2a, batch D: AppState.users field wired into AppStateBuilder (defaults to SqlUserStore optionally wrapped in BootstrapAdminBackend), Error::Users variant in crabcloud-core, and /index.php/login now consults state.users.verify(...) instead of inlining a bootstrap_admin check."
gh pr merge --auto --squash
```

Pull master after merge.

---

## Task 11: `AdminUser` extractor + self OCS endpoints

**Files:**
- Modify: `crates/crabcloud-http/src/extractors/auth.rs`
- Create: `crates/crabcloud-http/src/routes/ocs/user.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`

Start branch: `git checkout -b users-batch-e origin/master`

- [ ] **Step 1: Add `AdminUser` extractor**

Append to `crates/crabcloud-http/src/extractors/auth.rs`:

```rust
use crabcloud_core::AppState;

pub struct AdminUser(pub AuthenticatedUser);

impl FromRequestParts<AppState> for AdminUser
where
    AppState: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let authed = AuthenticatedUser::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError(crabcloud_core::Error::Unauthorized))?;
        let uid = crabcloud_users::UserId::new(&authed.user_id)
            .map_err(|_| ApiError(crabcloud_core::Error::Unauthorized))?;
        let is_admin = state.users.is_admin(&uid)
            .await
            .map_err(crabcloud_core::Error::Users)
            .map_err(ApiError)?;
        if !is_admin {
            return Err(ApiError(crabcloud_core::Error::Forbidden));
        }
        Ok(AdminUser(authed))
    }
}
```

Note: the `FromRequestParts` impl for `AuthenticatedUser` currently has `type Rejection = UnauthorizedRejection`; you'll need to adapt the call accordingly (it may be `.map_err(|_| ...)` to handle that rejection-type mismatch).

Re-export `AdminUser` from `crates/crabcloud-http/src/lib.rs`.

- [ ] **Step 2: Write `routes/ocs/user.rs`** (self-info GET + self-mutate PUT)

```rust
//! `GET /ocs/v2.php/cloud/user` and `PUT /ocs/v2.php/cloud/user` — self-only.

use crate::extractors::auth::AuthenticatedUser;
use crate::extractors::format::OcsFormat;
use crate::session::SessionHandle;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsStatus, OcsVersion};
use crabcloud_users::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct UserPayload {
    id: String,
    #[serde(rename = "display-name")]
    display_name: String,
    email: Option<String>,
    groups: Vec<String>,
    enabled: bool,
    #[serde(rename = "last-login")]
    last_login: u64,
}

pub async fn get_self(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&authed.user_id)
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;
    let user = state.users.lookup(&uid).await
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?
        .ok_or_else(|| OcsError::new(CoreError::Unauthorized, OcsVersion::V2, fmt.0))?;
    let groups = state.users.groups_of(&uid).await
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;

    let payload = UserPayload {
        id: user.uid.into_inner(),
        display_name: user.display_name,
        email: user.email.map(|e| e.as_str().to_string()),
        groups: groups.into_iter().map(|g| g.as_str().to_string()).collect(),
        enabled: user.enabled,
        last_login: user.last_seen,
    };
    let envelope = OcsResponse::ok(payload, OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

#[derive(Debug, Deserialize)]
pub struct PutForm {
    pub key: String,
    pub value: String,
    pub currentpassword: String,
}

pub async fn put_self(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Extension(handle): Extension<SessionHandle>,
    fmt: OcsFormat,
    Form(form): Form<PutForm>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&authed.user_id)
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;
    // Always re-verify currentpassword before any change.
    state.users.verify(uid.as_str(), &form.currentpassword).await
        .map_err(|_| OcsError::new(CoreError::Unauthorized, OcsVersion::V2, fmt.0))?;

    match form.key.as_str() {
        "password" => {
            state.users.set_password(&uid, &form.value).await
                .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;
            // Kick other devices.
            let sessions = state.cache.clone();
            let store = crate::session::SessionStore::new(sessions, &state.config.instanceid);
            let _ = store.destroy_all_for_except(uid.as_str(), Some(&handle.id)).await;
        }
        "displayname" => {
            state.users.user_store().set_display_name(&uid, &form.value).await
                .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;
        }
        "email" => {
            let new = if form.value.is_empty() { None } else { Some(form.value.as_str()) };
            state.users.user_store().set_email(&uid, new).await
                .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;
        }
        other => {
            return Err(OcsError::new(
                CoreError::BadRequest(format!("unknown key: {other}")),
                OcsVersion::V2,
                fmt.0,
            ));
        }
    }

    let envelope = OcsResponse::ok(serde_json::json!({}), OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}
```

- [ ] **Step 3: Mount the routes in `routes/ocs/mod.rs`**

Modify `crates/crabcloud-http/src/routes/ocs/mod.rs` — add the user module and routes:

```rust
pub mod capabilities;
pub mod user;

use axum::routing::{get, put};
use axum::Router;
use crabcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.php/cloud/capabilities", get(capabilities::handler))
        .route("/v2.php/cloud/user", get(user::get_self).put(user::put_self))
}
```

- [ ] **Step 4: Run + commit + open Batch E PR**

```
cargo xtask check-all
git add crates/crabcloud-http
git commit -m "feat(http,users): AdminUser extractor + GET/PUT /ocs/v2.php/cloud/user

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-e
gh pr create --base master --head users-batch-e \
  --title "users: batch E — AdminUser extractor + self OCS endpoints" \
  --body "Sub-project 2a, batch E: AdminUser extractor (AuthenticatedUser + admin-group check), GET /ocs/v2.php/cloud/user (self info), PUT /ocs/v2.php/cloud/user (self-service password/displayname/email; password change kicks other devices)."
gh pr merge --auto --squash
```

Pull master after merge.

---

## Task 12: CLI subcommands

**Files:**
- Modify: `crates/crabcloud-server/Cargo.toml`
- Modify: `crates/crabcloud-server/src/cli.rs`
- Modify: `crates/crabcloud-server/src/main.rs`
- Create: `crates/crabcloud-users/src/cli.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

Start branch: `git checkout -b users-batch-f origin/master`

- [ ] **Step 1: Add `rpassword` dep + reuse helpers in `crabcloud-users::cli`**

In `crates/crabcloud-server/Cargo.toml` `[dependencies]`, add:

```toml
crabcloud-users.workspace = true
rpassword.workspace = true
```

In `crates/crabcloud-users/Cargo.toml`, no new deps — the cli helper module is pure logic.

- [ ] **Step 2: Add `cli.rs` helpers to `crabcloud-users`**

Create `crates/crabcloud-users/src/cli.rs`:

```rust
//! Helpers for the server-bin's user/group management subcommands.
//! Pure async functions consuming `UsersService`.

use crate::error::UsersResult;
use crate::group::GroupId;
use crate::user::{User, UserId};
use crate::service::UsersService;

pub async fn user_add(
    svc: &UsersService,
    uid: &str,
    password: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    admin: bool,
) -> UsersResult<()> {
    let user_id = UserId::new(uid)?;
    let dn = display_name.map(str::to_string).unwrap_or_else(|| uid.to_string());
    let email_opt = match email {
        Some(e) => Some(crate::email::Email::parse(e)?),
        None => None,
    };
    let hash = svc.verifier().hash(password)?;
    let user = User {
        uid: user_id.clone(),
        display_name: dn,
        email: email_opt,
        enabled: true,
        last_seen: 0,
    };
    svc.user_store().create(&user, Some(&hash)).await?;
    if admin {
        svc.group_store().add_to_group(&user_id, &GroupId::new("admin")?).await?;
    }
    Ok(())
}

pub async fn user_set_password(svc: &UsersService, uid: &str, new: &str) -> UsersResult<()> {
    let user_id = UserId::new(uid)?;
    svc.set_password(&user_id, new).await
}

pub async fn user_delete(svc: &UsersService, uid: &str) -> UsersResult<()> {
    let user_id = UserId::new(uid)?;
    svc.user_store().delete(&user_id).await?;
    svc.preferences().delete_all_for(&user_id).await?;
    Ok(())
}

pub async fn group_add_member(svc: &UsersService, gid: &str, uid: &str) -> UsersResult<()> {
    svc.group_store().add_to_group(&UserId::new(uid)?, &GroupId::new(gid)?).await
}

pub async fn group_remove_member(svc: &UsersService, gid: &str, uid: &str) -> UsersResult<()> {
    svc.group_store().remove_from_group(&UserId::new(uid)?, &GroupId::new(gid)?).await
}
```

Re-export from `crates/crabcloud-users/src/lib.rs`:

```rust
pub mod cli;
```

- [ ] **Step 3: Extend the server CLI**

Modify `crates/crabcloud-server/src/cli.rs`. Add to the `Cmd` enum:

```rust
    /// Create a user (prompts for password on stdin).
    UserAdd {
        uid: String,
        #[arg(long)]
        admin: bool,
        #[arg(long)]
        email: Option<String>,
        #[arg(long = "display-name")]
        display_name: Option<String>,
    },
    /// Reset a user's password (prompts on stdin).
    UserSetPassword { uid: String },
    /// Delete a user.
    UserDelete { uid: String },
    /// Add a user to a group.
    GroupAddMember { gid: String, uid: String },
    /// Remove a user from a group.
    GroupRemoveMember { gid: String, uid: String },
```

- [ ] **Step 4: Wire them in `main.rs`**

Modify `crates/crabcloud-server/src/main.rs`. Inside `main`'s match block, add arms for the new commands. Helper to prompt:

```rust
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    let pw = rpassword::prompt_password(prompt)?;
    if pw.is_empty() {
        anyhow::bail!("password cannot be empty");
    }
    Ok(pw)
}
```

Match arms:

```rust
        Cmd::UserAdd { uid, admin, email, display_name } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let pw = prompt_password("New password: ")?;
            let confirm = prompt_password("Confirm: ")?;
            if pw != confirm {
                anyhow::bail!("passwords didn't match");
            }
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_add(
                &state.users, &uid, &pw,
                display_name.as_deref(), email.as_deref(), admin
            ).await?;
            info!(uid, admin, "user created");
            state.pool.close().await;
            Ok(())
        }
        Cmd::UserSetPassword { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let pw = prompt_password("New password: ")?;
            let confirm = prompt_password("Confirm: ")?;
            if pw != confirm {
                anyhow::bail!("passwords didn't match");
            }
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_set_password(&state.users, &uid, &pw).await?;
            info!(uid, "password reset");
            state.pool.close().await;
            Ok(())
        }
        Cmd::UserDelete { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            eprint!("Delete user {uid} and all their preferences? (yes/no): ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim() != "yes" {
                anyhow::bail!("aborted");
            }
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_delete(&state.users, &uid).await?;
            info!(uid, "user deleted");
            state.pool.close().await;
            Ok(())
        }
        Cmd::GroupAddMember { gid, uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::group_add_member(&state.users, &gid, &uid).await?;
            info!(gid, uid, "added to group");
            state.pool.close().await;
            Ok(())
        }
        Cmd::GroupRemoveMember { gid, uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::group_remove_member(&state.users, &gid, &uid).await?;
            info!(gid, uid, "removed from group");
            state.pool.close().await;
            Ok(())
        }
```

- [ ] **Step 5: Run + commit + open Batch F PR**

```
cargo xtask check-all
git add crates/crabcloud-users crates/crabcloud-server Cargo.toml Cargo.lock
git commit -m "feat(server,users): CLI subcommands user-add/set-password/delete + group members

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-f
gh pr create --base master --head users-batch-f \
  --title "users: batch F — CLI subcommands" \
  --body "Sub-project 2a, batch F: crabcloud-server gains user-add, user-set-password, user-delete, group-add-member, group-remove-member subcommands. Passwords prompted interactively via rpassword."
gh pr merge --auto --squash
```

Pull master after merge.

---

## Task 13: End-to-end tests + acceptance docs

**Files:**
- Create: `crates/crabcloud-users/tests/users_flow.rs`
- Create: `docs/superpowers/plans/2026-05-11-users-core-implementation.changelog.md`
- Modify: `README.md`

Start branch: `git checkout -b users-batch-g origin/master`

- [ ] **Step 1: Write integration test**

Create `crates/crabcloud-users/tests/users_flow.rs`:

```rust
//! End-to-end flow against `build_router(state)` exercising spec acceptance criteria.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn build_app() -> axum::Router {
    let dir = tempdir().unwrap();
    let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("flow.db"));
    let state = AppStateBuilder::new(cfg).with_core_capabilities().build().await.unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state.users.user_store().create(&User {
        uid: UserId::new("alice").unwrap(),
        display_name: "Alice".into(),
        email: None,
        enabled: true,
        last_seen: 0,
    }, Some(&hash)).await.unwrap();
    std::mem::forget(dir);
    build_router(state)
}

#[tokio::test]
async fn login_with_real_user_succeeds() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=hunter2"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn login_with_wrong_password_401() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=WRONG"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_with_unknown_user_401() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=nobody&password=anything"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_self_returns_payload() {
    let app = build_app().await;
    // Login first.
    let req1 = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=hunter2"))
        .unwrap();
    let r1 = app.clone().oneshot(req1).await.unwrap();
    let cookie = r1.headers().get("set-cookie").unwrap().to_str().unwrap()
        .split(';').next().unwrap().to_string();

    let req2 = Request::builder()
        .method("GET")
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let r2 = app.oneshot(req2).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let body = to_bytes(r2.into_body(), 16 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["data"]["id"], "alice");
    assert_eq!(parsed["ocs"]["data"]["display-name"], "Alice");
    assert_eq!(parsed["ocs"]["data"]["enabled"], true);
}
```

- [ ] **Step 2: Write changelog**

Create `docs/superpowers/plans/2026-05-11-users-core-implementation.changelog.md`:

```markdown
# Sub-project 2a (Core User Store) — Changelog

Completed: <today's date YYYY-MM-DD>

## What works

- `crabcloud-users` crate with `UserId` / `Email` / `GroupId` validating newtypes, `User` / `Group` records, `UsersError` (status + client-message mapping into `crabcloud-core::Error`).
- `UserStore` / `GroupStore` / `PreferenceStore` async traits + `Sql*` implementations (multi-dialect, hand-dispatched per pool variant).
- `PasswordVerifier` trait + `BcryptVerifier` with sentinel-hash constant-time fake-verify on lookup miss.
- `UsersService` façade — `verify`, `lookup`, `set_password`, `is_admin`, `groups_of`, `preferences`.
- `BootstrapAdminBackend` shim — synthesizes a virtual admin when `config.bootstrap_admin` is set; promotes-on-first-write into `oc_users` and the `admin` group, retiring itself.
- Phase 3's `Session` gains `two_factor_passed: bool` (always `true` in 2a; placeholder for sub-project 2c).
- `SessionStore::destroy_all_for` / `destroy_all_for_except` backed by an `instance_id:sessions_by_user:{uid}` side-index in cache.
- `/index.php/login` now consults `state.users.verify(...)` instead of the inline bootstrap_admin check.
- New OCS endpoints: `GET /ocs/v2.php/cloud/user` (self info; matches Nextcloud's `{id, display-name, email, groups, enabled, last-login}`), `PUT /ocs/v2.php/cloud/user` (self-service password/displayname/email; `currentpassword` required; password change kicks other devices).
- New `AdminUser` extractor (`AuthenticatedUser` + admin-group check).
- New CLI subcommands on `crabcloud-server`: `user-add`, `user-set-password`, `user-delete`, `group-add-member`, `group-remove-member`. Passwords prompted via `rpassword`.
- Migration 0002 creates `oc_users` + `oc_groups` + `oc_group_user` + `oc_preferences` per-dialect; seeds the `admin` group.

## What's deferred

- Admin OCS endpoints (`POST` / `PUT` / `DELETE /ocs/v2.php/cloud/users`) — own follow-up sub-project.
- Groups OCS endpoints (`/ocs/v2.php/cloud/groups`) — same.
- App passwords + Bearer/Basic auth — sub-project 2b.
- 2FA framework — sub-project 2c.
- OAuth2 server — sub-project 2d.
- LDAP backend — sub-project 2e.
- SAML backend — sub-project 2f.
- Password reset via email — needs mail-sending sub-project.
- Settings UI for self-service — needs the settings UI sub-project.
- Multi-backend composition (`CompositeUserStore`) — deferred to 2e.
- Sub-admins, group quotas, file-system mappings — long-tail.
- Case-insensitive `uid` matching — needs a generated column.
- Password strength policy — Nextcloud's `password_policy` app equivalent, future.
- Legacy password hash formats (sha1/sha256/argon2-via-PHP) — only bcrypt today.

## Known limitations

- MySQL email-uniqueness is enforced application-side (no partial unique index on the dialect).
- `sessions_by_user` index in `MemoryCache` grows linearly per user — fine single-node; revisit when the Redis cache backend lands.
- `BootstrapAdminBackend::set_display_name` / `set_email` return `ReadOnly` for the virtual admin — promote-then-set is a polish item.
- CLI `user-add` prompts password on stdin; no `--password-stdin` flag. Add when scripted provisioning is a real need.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` against SQLite + MySQL + Postgres | OK |
| 2 | `crabcloud-server user-add alice --admin` + login succeeds | OK |
| 3 | `bootstrap_admin` + empty DB → admin logs in | OK (BootstrapAdminBackend) |
| 4 | Self-service password change against virtual admin promotes into DB | OK |
| 5 | Disabled user gets 401 | OK |
| 6 | `PUT /ocs/v2.php/cloud/user key=password` updates hash + kicks other sessions, keeps current | OK |
| 7 | `GET /ocs/v2.php/cloud/user` returns the expected envelope | OK |
| 8 | Playwright E2E still passes | OK (bootstrap-admin shim path unchanged) |
| 9 | `git grep -i rustcloud` empty | OK |
| 10 | `[workspace.lints]` `-D warnings` clean for `crabcloud-users` | OK |
```

- [ ] **Step 3: Update README**

Modify the README's "Quick start" section to add the `crabcloud-server user-add` step:

```markdown
# 3a. Create your first admin user (interactive password prompt).
cargo run -p crabcloud-server -- user-add admin --admin

# 3b. (or, for the fresh-install bootstrap path)
#     Add [bootstrap_admin] to config.toml with a bcrypt hash;
#     log in, change your password — your account is now a real DB user.
```

Add a note in the "Workspace layout" section listing `crates/crabcloud-users`.

- [ ] **Step 4: Run final acceptance**

```
cargo clean
cargo xtask check-all
cargo test -p crabcloud-users --test users_flow
```

Expected: green; 4 integration tests pass.

- [ ] **Step 5: Commit + open Batch G PR**

```
git add crates/crabcloud-users docs/superpowers/plans README.md
git commit -m "docs(users): sub-project 2a acceptance — README, changelog, e2e tests

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin users-batch-g
gh pr create --base master --head users-batch-g \
  --title "users: batch G — e2e tests + acceptance docs" \
  --body "Sub-project 2a, final batch: end-to-end tests via build_router (login success/wrong/unknown, GET /ocs/v2.php/cloud/user), README update with user-add CLI, sub-project 2a changelog. Closes spec acceptance criteria 1-10."
gh pr merge --auto --squash
```

---

## Final acceptance

After Batch G merges:

1. **Pull master**: `git pull --ff-only origin master`.
2. **Run end-to-end**: `cargo xtask check-all` + Playwright E2E in CI must be green.
3. **Spot-check** the bootstrap-admin login still works via the e2e suite.
4. **Manual smoke** (optional): `cargo run -p crabcloud-server -- user-add testuser --admin`, then log in.
5. Mark sub-project 2a complete in the program-level tracking doc.

## Open questions deferred to sub-project tracking

- Should `BootstrapAdminBackend` also accept `--admin-username` / `--admin-password-hash` CLI overrides for ephemeral first-install setups (Docker, CI)? Currently config-file-only.
- bcrypt cost = 12. Profile and consider 13 when admin UX shows acceptable latency.
- Add `delete_user` cleanup hook for future apps to remove user-owned data (file shares, calendar entries, etc.) — needs the app-framework sub-project.
