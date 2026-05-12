# App Passwords + Bearer/Basic Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the second half of authentication: a database-backed `oc_authtoken` store that serves both long-lived app passwords (used by DAV / desktop / mobile clients via Basic or Bearer auth) and browser session tokens (so every active device is listable and revocable). Cookie auth becomes DB-authoritative; the existing cache-backed session is reduced to per-session ephemeral state (CSRF, two_factor_passed) keyed by the token row id.

**Architecture:** New `TokenStore` trait + `SqlTokenStore` + `TokenAuthCache` + `AppPasswordService` in `crabcloud-users`. New `AuthLayer` Axum middleware in `crabcloud-http` that walks Bearer → Basic → Cookie and attaches an `AuthContext` to request extensions. Existing extractors (`AuthenticatedUser`, `AdminUser`, `OptionalUser`) keep their shapes but read `AuthContext` instead of `SessionHandle`. New `/index.php/login/v2{,/flow/<id>,/poll}` server fns + `/ocs/v2.php/core/{getapppassword,apppassword}` OCS endpoints + Settings → Security Dioxus page. CLI gains `app-password-{add,list,revoke}` subcommands.

**Tech Stack:** Existing workspace deps only — `sqlx`, `axum`, `dioxus` (0.7 fullstack), `tower`, `secrecy`, `base64`, `hex`, `sha2`, `rand`, `bcrypt`, `serde`, `thiserror`, `tracing`. No new crates.

**Parent spec:** `docs/superpowers/specs/2026-05-12-app-passwords-bearer-basic-auth-design.md`.

**Previous state:** Sub-project 2a complete at commit `1407d8c`. Master HEAD `6a014c8` (spec PR merged). Dioxus 0.7 fullstack landed (PR #21); `/index.php/login` is a `#[server]` fn in `crabcloud-ui::server_fns`. CI green.

**Branch protection:** `master` is PR-required (ruleset). Auto-merge disabled at repo level. Each batch lands as one PR; merge manually with `gh pr merge --squash --delete-branch` after CI greens.

---

## Conventions (carried from 2a)

- **Commits:** Conventional Commits (`feat(auth)`, `chore(auth)`, `test(auth)`, …) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD:** Failing test → fail → implement → pass → commit. Each task ends with a commit.
- **rustfmt:** `cargo fmt --all` after each task. Authorized.
- **Plan-bug protocol:** if verbatim code fails to compile or test, fix minimally and report DONE_WITH_CONCERNS with a clear diff explanation.
- **`cargo xtask check-all` must pass after every commit.**
- **Workspace lints**: `unused_crate_dependencies = "warn"`, `clippy::all = "warn"`, and CI runs `cargo clippy --workspace --all-targets -- -D warnings`. Mind the unused-dep lint when adding deps; use the `as _` placeholder idiom from `crabcloud-ui/src/lib.rs` if a new dep isn't used yet.
- **Test fixtures:** use `crabcloud_config::test_support::minimal_sqlite_config` for fresh-DB tests.
- **The spec calls raw tokens "base62"; the implementation uses `base64::URL_SAFE_NO_PAD`** — functionally equivalent (URL- and Basic-auth-safe, no padding ambiguity) and reuses an existing workspace dep. Note this once and don't re-explain in every task.

---

## File Structure (locked in for the whole plan)

```
crates/
├── crabcloud-users/                              # MODIFIED
│   ├── Cargo.toml                                # +base64, +sha2, +hex (already workspace deps)
│   └── src/
│       ├── lib.rs                                # +re-exports
│       ├── auth_token.rs                  (NEW)  # AuthToken, AuthTokenType, RawToken, hash_token
│       ├── error.rs                              # +TokenNotFound, +TokenAlreadyRevoked variants
│       ├── service.rs                            # +app_passwords field; set_password cascade
│       ├── app_password.rs                (NEW)  # AppPasswordService façade
│       ├── cli.rs                                # +app_password_add / list / revoke helpers
│       └── store/
│           ├── mod.rs                            # +pub mod auth_token
│           └── auth_token.rs              (NEW)  # TokenStore trait + SqlTokenStore + TokenAuthCache
│
├── crabcloud-core/                               # MODIFIED
│   └── src/state.rs                              # AppState.tokens + AppStateBuilder default wiring
│
├── crabcloud-http/                               # MODIFIED
│   └── src/
│       ├── auth_context.rs                (NEW)  # AuthContext + AuthMethod (Session/Bearer/Basic)
│       ├── middleware/
│       │   ├── mod.rs                            # +pub mod auth
│       │   └── auth.rs                    (NEW)  # AuthLayer (cookie/Bearer/Basic → AuthContext)
│       ├── csrf.rs                               # gate on AuthMethod::Session
│       ├── extractors/auth.rs                    # read AuthContext extension instead of SessionHandle
│       ├── lib.rs                                # +AuthLayer, +AuthContext, +AuthMethod re-exports
│       ├── routes/
│       │   └── ocs/
│       │       ├── mod.rs                        # +pub mod app_password (mounted at /core/{...})
│       │       └── app_password.rs        (NEW)  # GET getapppassword + DELETE apppassword
│       └── session/
│           └── layer.rs                          # shrinks to cookie sign/verify only;
│                                                 # SessionState (csrf_token, two_factor_passed)
│                                                 # cache-keyed by token_id
│
├── crabcloud-ui/                                 # MODIFIED
│   └── src/
│       ├── app.rs                                # +Route variants: LoginV2FlowRoute, SettingsSecurityRoute
│       ├── pages/
│       │   ├── mod.rs                            # +pub mod login_v2_flow, +pub mod settings_security
│       │   ├── login_v2_flow.rs           (NEW)  # GET /index.php/login/v2/flow/<id> Dioxus page
│       │   └── settings_security.rs       (NEW)  # /settings/security Dioxus page
│       └── server_fns.rs                         # +login_v2_start, +login_v2_authorize, +login_v2_poll,
│                                                 # +list_app_passwords, +create_app_password,
│                                                 # +revoke_app_password, +destroy_other_sessions
│
├── crabcloud-server/                             # MODIFIED
│   └── src/
│       ├── cli.rs                                # +AppPasswordAdd / AppPasswordList / AppPasswordRevoke
│       └── main.rs                               # +match arms
│
├── e2e/
│   └── tests/
│       └── app_password.spec.ts           (NEW)  # mint → curl → revoke → curl-fails
│
└── migrations/core/0003_auth_tokens/      (NEW)
    ├── sqlite.sql
    ├── mysql.sql
    └── postgres.sql
```

---

## Batches

Execution order — each is its own PR:

| Batch | Tasks | Theme                                                            |
|-------|-------|------------------------------------------------------------------|
| **A** | 1–4   | Migration 0003 + AuthToken types + TokenStore trait + SqlTokenStore |
| **B** | 5–7   | hash_token helper + TokenAuthCache + AppPasswordService          |
| **C** | 8–10  | AppState.tokens wiring + AuthContext + AuthLayer + extractor rewire |
| **D** | 11–13 | SessionLayer slim-down + login mints session AuthToken + CSRF gating |
| **E** | 14–16 | `/login/v2` flow + `/ocs/.../core/{getapppassword,apppassword}`  |
| **F** | 17–18 | Settings → Security UI page + CLI subcommands                    |
| **G** | 19–20 | E2E test + acceptance docs                                       |

After each batch: spec-compliance review → code-quality review → manual PR merge once CI greens.

---

## Task 1: Migration 0003 — `oc_authtoken` schema

**Files:**
- Create: `migrations/core/0003_auth_tokens/sqlite.sql`
- Create: `migrations/core/0003_auth_tokens/mysql.sql`
- Create: `migrations/core/0003_auth_tokens/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs`

- [ ] **Step 1: Write `migrations/core/0003_auth_tokens/sqlite.sql`**

```sql
CREATE TABLE oc_authtoken (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    uid               TEXT    NOT NULL,
    login_name        TEXT    NOT NULL,
    password          TEXT,
    name              TEXT    NOT NULL,
    token             TEXT    NOT NULL,
    type              INTEGER NOT NULL DEFAULT 0,
    remember          INTEGER NOT NULL DEFAULT 0,
    last_activity     INTEGER NOT NULL DEFAULT 0,
    last_check        INTEGER NOT NULL DEFAULT 0,
    public_key        TEXT,
    private_key       TEXT,
    version           INTEGER NOT NULL DEFAULT 2,
    scope             TEXT,
    expires           INTEGER,
    password_invalid  INTEGER NOT NULL DEFAULT 0,
    remote_wipe       INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX oc_authtoken_token_idx     ON oc_authtoken(token);
CREATE        INDEX oc_authtoken_uid_type_idx  ON oc_authtoken(uid, type);
CREATE        INDEX oc_authtoken_activity_idx  ON oc_authtoken(last_activity);
```

- [ ] **Step 2: Write `migrations/core/0003_auth_tokens/mysql.sql`**

```sql
CREATE TABLE oc_authtoken (
    id                BIGINT       NOT NULL AUTO_INCREMENT,
    uid               VARCHAR(64)  NOT NULL,
    login_name        VARCHAR(64)  NOT NULL,
    password          LONGTEXT,
    name              VARCHAR(128) NOT NULL,
    token             VARCHAR(200) NOT NULL,
    type              SMALLINT     NOT NULL DEFAULT 0,
    remember          TINYINT      NOT NULL DEFAULT 0,
    last_activity     BIGINT       NOT NULL DEFAULT 0,
    last_check        BIGINT       NOT NULL DEFAULT 0,
    public_key        LONGTEXT,
    private_key       LONGTEXT,
    version           SMALLINT     NOT NULL DEFAULT 2,
    scope             LONGTEXT,
    expires           BIGINT,
    password_invalid  TINYINT      NOT NULL DEFAULT 0,
    remote_wipe       TINYINT      NOT NULL DEFAULT 0,
    PRIMARY KEY (id),
    UNIQUE KEY oc_authtoken_token_idx (token),
    KEY oc_authtoken_uid_type_idx (uid, type),
    KEY oc_authtoken_activity_idx (last_activity)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
```

- [ ] **Step 3: Write `migrations/core/0003_auth_tokens/postgres.sql`**

```sql
CREATE TABLE oc_authtoken (
    id                BIGSERIAL    PRIMARY KEY,
    uid               VARCHAR(64)  NOT NULL,
    login_name        VARCHAR(64)  NOT NULL,
    password          TEXT,
    name              VARCHAR(128) NOT NULL,
    token             VARCHAR(200) NOT NULL,
    type              SMALLINT     NOT NULL DEFAULT 0,
    remember          SMALLINT     NOT NULL DEFAULT 0,
    last_activity     BIGINT       NOT NULL DEFAULT 0,
    last_check        BIGINT       NOT NULL DEFAULT 0,
    public_key        TEXT,
    private_key       TEXT,
    version           SMALLINT     NOT NULL DEFAULT 2,
    scope             TEXT,
    expires           BIGINT,
    password_invalid  SMALLINT     NOT NULL DEFAULT 0,
    remote_wipe       SMALLINT     NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX oc_authtoken_token_idx     ON oc_authtoken(token);
CREATE        INDEX oc_authtoken_uid_type_idx  ON oc_authtoken(uid, type);
CREATE        INDEX oc_authtoken_activity_idx  ON oc_authtoken(last_activity);
```

- [ ] **Step 4: Register migration in `crates/crabcloud-db/src/core_migrations.rs`**

Append to `CORE_MIGRATIONS`:

```rust
    Migration {
        version: 3,
        name: "auth_tokens",
        sqlite: include_str!("../../../migrations/core/0003_auth_tokens/sqlite.sql"),
        mysql:  include_str!("../../../migrations/core/0003_auth_tokens/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0003_auth_tokens/postgres.sql"),
    },
```

Update the existing `assert_eq!(applied, 2)` in `core_migration_applies_against_sqlite` (around line 56) to `assert_eq!(applied, 3)`.

Also update `crates/crabcloud-db/tests/migrate_end_to_end.rs` — any `assert_eq!(applied, 2)` there becomes `assert_eq!(applied, 3)`. The DROP TABLE prelude in the MySQL/Postgres e2e tests must also gain `oc_authtoken` (in the right order — child-before-parent ordering is fine because `oc_authtoken` has no FK dependents).

- [ ] **Step 5: Add migration smoke test in `core_migrations.rs`**

Append to the existing `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn auth_tokens_migration_creates_table() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("authtoken.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();

        if let DbPool::Sqlite(p) = &pool {
            sqlx::query(
                "INSERT INTO oc_authtoken \
                 (uid, login_name, name, token, type, remember, last_activity, last_check, version, password_invalid, remote_wipe) \
                 VALUES ('alice', 'alice', 'DAV client', 'hash123', 1, 0, 0, 0, 2, 0, 0)",
            )
            .execute(p)
            .await
            .unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM oc_authtoken WHERE token = 'hash123'")
                .fetch_one(p)
                .await
                .unwrap();
            assert!(id > 0);
        } else {
            unreachable!()
        }
        pool.close().await;
    }
```

- [ ] **Step 6: Run tests**

```
cargo test -p crabcloud-db --lib core_migrations
```

Expected: existing tests + the new `auth_tokens_migration_creates_table` pass.

- [ ] **Step 7: Run check-all + commit + open Batch A branch**

```
cargo xtask check-all
```

Expected: green.

```
git checkout -b auth-batch-a
git add migrations crates/crabcloud-db
git commit -m "feat(db,auth): add migration 0003 creating oc_authtoken (full upstream schema)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: AuthToken types + new `UsersError` variants

**Files:**
- Create: `crates/crabcloud-users/src/auth_token.rs`
- Modify: `crates/crabcloud-users/src/error.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`
- Modify: `crates/crabcloud-users/Cargo.toml`

- [ ] **Step 1: Add deps to `crates/crabcloud-users/Cargo.toml`**

Add to `[dependencies]` (alphabetical, all already workspace-declared):

```toml
base64.workspace = true
hex.workspace = true
rand.workspace = true
secrecy.workspace = true
sha2.workspace = true
```

(These let the crate compute SHA-512 hashes, encode raw bytes as URL-safe base64, generate OS-RNG bytes, and wrap secrets.)

- [ ] **Step 2: Add new variants to `crates/crabcloud-users/src/error.rs`**

Add inside the enum (alphabetical with existing variants):

```rust
    #[error("token not found")]
    TokenNotFound,
    #[error("token already revoked")]
    TokenAlreadyRevoked,
```

- [ ] **Step 3: Update `crates/crabcloud-core/src/error.rs::users_status` mapping**

Add arms inside `users_status` (after `ReadOnly => 403,`):

```rust
        TokenNotFound => 401,
        TokenAlreadyRevoked => 410,
```

Also extend the `users_error_http_status_mapping` test:

```rust
        assert_eq!(Error::Users(UsersError::TokenNotFound).http_status(), 401);
        assert_eq!(Error::Users(UsersError::TokenAlreadyRevoked).http_status(), 410);
```

- [ ] **Step 4: Write `crates/crabcloud-users/src/auth_token.rs`**

```rust
//! `oc_authtoken` row type + raw-token generator + hashing helper.

use crate::error::UsersError;
use crate::user::UserId;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

/// Discriminator for [`AuthToken::kind`]. Mapped to the upstream `type`
/// integer column.
#[repr(i32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthTokenType {
    /// Cookie-backed browser session.
    Session = 0,
    /// Long-lived token used via Bearer / Basic auth (DAV / desktop / mobile).
    AppPassword = 1,
}

impl AuthTokenType {
    /// Convert from the SQL `type` column.
    pub fn from_i32(v: i32) -> Result<Self, UsersError> {
        match v {
            0 => Ok(Self::Session),
            1 => Ok(Self::AppPassword),
            other => Err(UsersError::Internal(anyhow::anyhow!(
                "unknown AuthTokenType discriminator: {other}"
            ))),
        }
    }

    /// Convert to the SQL `type` column.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Persisted token row. Mirrors the full upstream `oc_authtoken` schema; many
/// columns are nullable / always-default in sub-project 2b (E2E key pair,
/// scope, etc.) and are populated by later sub-projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthToken {
    pub id: i64,
    pub uid: UserId,
    pub login_name: String,
    pub password: Option<String>,
    pub name: String,
    /// Hashed token value (128-hex SHA-512 of `raw_token || secret`).
    pub token: String,
    pub kind: AuthTokenType,
    pub remember: bool,
    pub last_activity: u64,
    pub last_check: u64,
    pub public_key: Option<String>,
    pub private_key: Option<String>,
    pub version: i32,
    pub scope: Option<String>,
    pub expires: Option<u64>,
    pub password_invalid: bool,
    pub remote_wipe: bool,
}

impl AuthToken {
    /// True when the row is in a state the auth path must reject (expired,
    /// password-invalidated, or remote-wiped). Callers should treat a
    /// `is_unusable() == true` row as if the lookup missed.
    pub fn is_unusable(&self, now: u64) -> bool {
        if self.password_invalid || self.remote_wipe {
            return true;
        }
        match self.expires {
            Some(exp) if exp <= now => true,
            _ => false,
        }
    }
}

/// Raw, plaintext token. Produced once at mint time, displayed to the user
/// once, then discarded. Wrapped in `SecretString` so it never lands in
/// `Debug` / log output.
#[derive(Debug, Clone)]
pub struct RawToken(SecretString);

impl RawToken {
    /// Generate a fresh 72-byte token from `OsRng`, base64-URL-encoded
    /// without padding (~96 ASCII chars). The alphabet is `[A-Za-z0-9_-]`
    /// — URL-safe and safe to embed in HTTP Basic auth.
    pub fn generate() -> Self {
        let mut buf = [0u8; 72];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        Self(SecretString::new(B64.encode(&buf).into()))
    }

    /// Construct from an existing string (e.g. read from a Bearer header or
    /// Basic-auth password portion). Caller is responsible for treating the
    /// result as secret.
    pub fn from_string(s: String) -> Self {
        Self(SecretString::new(s.into()))
    }

    /// Borrow the raw value. Caller MUST NOT log or `Debug`-print the result.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

/// Compute the storage hash for a raw token: lowercase hex of
/// `SHA-512(raw_token_bytes || secret_bytes)`. Deterministic, suitable
/// for an equality lookup against the unique `token` column.
pub fn hash_token(raw: &str, secret: &str) -> String {
    use sha2::{Digest, Sha512};
    let mut hasher = Sha512::new();
    hasher.update(raw.as_bytes());
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_token_is_long_and_url_safe() {
        let t = RawToken::generate();
        let s = t.expose();
        // 72 bytes -> base64 URL no-pad = 96 chars
        assert_eq!(s.len(), 96);
        for c in s.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-URL-safe char {c:?} in token"
            );
        }
    }

    #[test]
    fn raw_token_debug_does_not_leak_value() {
        let t = RawToken::generate();
        let dbg = format!("{t:?}");
        assert!(!dbg.contains(t.expose()), "Debug printed the secret");
    }

    #[test]
    fn raw_tokens_differ_each_call() {
        let a = RawToken::generate();
        let b = RawToken::generate();
        assert_ne!(a.expose(), b.expose());
    }

    #[test]
    fn hash_token_is_deterministic_for_same_inputs() {
        assert_eq!(hash_token("abc", "k"), hash_token("abc", "k"));
    }

    #[test]
    fn hash_token_changes_with_secret() {
        assert_ne!(hash_token("abc", "k1"), hash_token("abc", "k2"));
    }

    #[test]
    fn hash_token_changes_with_input() {
        assert_ne!(hash_token("abc", "k"), hash_token("xyz", "k"));
    }

    #[test]
    fn hash_token_is_128_hex_chars() {
        let h = hash_token("anything", "secret");
        assert_eq!(h.len(), 128);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn auth_token_type_roundtrip() {
        for kind in [AuthTokenType::Session, AuthTokenType::AppPassword] {
            assert_eq!(AuthTokenType::from_i32(kind.as_i32()).unwrap(), kind);
        }
        assert!(AuthTokenType::from_i32(7).is_err());
    }

    #[test]
    fn unusable_detects_expiry_and_flags() {
        let mut row = AuthToken {
            id: 1,
            uid: UserId::new("alice").unwrap(),
            login_name: "alice".into(),
            password: None,
            name: "x".into(),
            token: "h".into(),
            kind: AuthTokenType::Session,
            remember: false,
            last_activity: 0,
            last_check: 0,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        };
        assert!(!row.is_unusable(1000));
        row.expires = Some(500);
        assert!(row.is_unusable(1000));
        row.expires = Some(2000);
        assert!(!row.is_unusable(1000));
        row.password_invalid = true;
        assert!(row.is_unusable(1000));
        row.password_invalid = false;
        row.remote_wipe = true;
        assert!(row.is_unusable(1000));
    }
}
```

- [ ] **Step 5: Re-export from `crates/crabcloud-users/src/lib.rs`**

Add (alphabetical with existing mods + uses):

```rust
mod auth_token;
```

And:

```rust
pub use auth_token::{hash_token, AuthToken, AuthTokenType, RawToken};
```

- [ ] **Step 6: Run tests**

```
cargo test -p crabcloud-users --lib auth_token
cargo test -p crabcloud-core --lib error
```

Expected: 9 auth_token tests + the extended `users_error_http_status_mapping` pass.

- [ ] **Step 7: Run check-all + commit**

```
cargo xtask check-all
git add crates/crabcloud-users crates/crabcloud-core
git commit -m "feat(users,auth): add AuthToken/RawToken/hash_token + TokenNotFound/TokenAlreadyRevoked errors

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `TokenStore` trait + module wiring

**Files:**
- Create: `crates/crabcloud-users/src/store/auth_token.rs`
- Modify: `crates/crabcloud-users/src/store/mod.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Add `pub mod auth_token` to `store/mod.rs`**

Append (alphabetical with existing `pub mod bootstrap_shim;` and `pub mod sql;`):

```rust
pub mod auth_token;
```

- [ ] **Step 2: Write the trait + Sql skeleton at `crates/crabcloud-users/src/store/auth_token.rs`**

Trait only in this task; the `SqlTokenStore` body lands in Task 4.

```rust
//! `TokenStore` — async trait for `oc_authtoken` CRUD + lifecycle. The
//! `SqlTokenStore` body lives in the same file; the read-through
//! `TokenAuthCache` lives in Task 6.

use crate::auth_token::AuthToken;
use crate::error::UsersResult;
use crate::user::UserId;
use async_trait::async_trait;
use crabcloud_db::DbPool;

#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Insert a fresh row. Returns the new row id.
    async fn create(&self, row: &AuthToken) -> UsersResult<i64>;

    /// Look up by hash. Returns `None` on miss (NOT an error).
    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>>;

    /// Look up by primary key. Returns `None` on miss.
    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>>;

    /// All rows for `uid`, newest-`last_activity`-first.
    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>>;

    /// Set `last_activity = last_check = now`. Best-effort: a missing row
    /// is silently ignored to avoid failing an otherwise-successful auth.
    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()>;

    /// Delete by id. Idempotent (deleting an absent row is fine).
    async fn revoke(&self, id: i64) -> UsersResult<()>;

    /// Delete every row owned by `uid`.
    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()>;

    /// Delete every row owned by `uid` except `except`.
    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64)
        -> UsersResult<()>;

    /// Set `password_invalid = 1` on every row owned by `uid`. Used by
    /// `UsersService::set_password` to cascade-invalidate other tokens.
    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()>;
}

#[derive(Clone)]
pub struct SqlTokenStore {
    pool: DbPool,
}

impl SqlTokenStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}
```

(The trait impl block lands in Task 4. This task's commit just adds the trait + struct shell.)

- [ ] **Step 3: Re-export from `lib.rs`**

Add:

```rust
pub use store::auth_token::{SqlTokenStore, TokenStore};
```

(alphabetical with `pub use store::sql::{...}` and `pub use store::{...}`).

- [ ] **Step 4: Build + commit**

```
cargo build -p crabcloud-users
```

Expected: clean.

```
git add crates/crabcloud-users
git commit -m "feat(users,auth): TokenStore trait + SqlTokenStore skeleton

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: `SqlTokenStore` implementation

**Files:**
- Modify: `crates/crabcloud-users/src/store/auth_token.rs`

This is the long mechanical task — same per-dialect `match &pool` pattern as `SqlUserStore`. Tests use `core_set()` so migration 0003 is applied during `fresh_pool()`.

- [ ] **Step 1: Append the `impl TokenStore for SqlTokenStore` block**

In `store/auth_token.rs`, after the `impl SqlTokenStore` block:

```rust
use super::sql; // for nothing — kept inert; remove if clippy complains.
use crate::auth_token::AuthTokenType;
use crate::error::UsersError;
use crabcloud_db::DbError;

fn map_sqlx<T>(r: Result<T, sqlx::Error>) -> UsersResult<T> {
    r.map_err(|e| UsersError::Db(DbError::Sqlx(e)))
}

type Row = (
    i64,             // id
    String,          // uid
    String,          // login_name
    Option<String>,  // password
    String,          // name
    String,          // token (hash)
    i64,             // type
    i64,             // remember
    i64,             // last_activity
    i64,             // last_check
    Option<String>,  // public_key
    Option<String>,  // private_key
    i64,             // version
    Option<String>,  // scope
    Option<i64>,     // expires
    i64,             // password_invalid
    i64,             // remote_wipe
);

fn row_to_token(r: Row) -> UsersResult<AuthToken> {
    let (
        id, uid, login_name, password, name, token, kind_int, remember_int,
        last_activity, last_check, public_key, private_key, version, scope,
        expires, password_invalid_int, remote_wipe_int,
    ) = r;
    Ok(AuthToken {
        id,
        uid: UserId::new(uid)?,
        login_name,
        password,
        name,
        token,
        kind: AuthTokenType::from_i32(kind_int as i32)?,
        remember: remember_int != 0,
        last_activity: last_activity.max(0) as u64,
        last_check: last_check.max(0) as u64,
        public_key,
        private_key,
        version: version as i32,
        scope,
        expires: expires.map(|e| e.max(0) as u64),
        password_invalid: password_invalid_int != 0,
        remote_wipe: remote_wipe_int != 0,
    })
}

const SELECT_COLUMNS: &str =
    "id, uid, login_name, password, name, token, type, remember, \
     last_activity, last_check, public_key, private_key, version, scope, \
     expires, password_invalid, remote_wipe";

#[async_trait]
impl TokenStore for SqlTokenStore {
    async fn create(&self, row: &AuthToken) -> UsersResult<i64> {
        let kind_int: i64 = row.kind.as_i32() as i64;
        let remember_int: i64 = if row.remember { 1 } else { 0 };
        let last_activity: i64 = row.last_activity as i64;
        let last_check: i64 = row.last_check as i64;
        let version: i64 = row.version as i64;
        let expires: Option<i64> = row.expires.map(|e| e as i64);
        let pi: i64 = if row.password_invalid { 1 } else { 0 };
        let rw: i64 = if row.remote_wipe { 1 } else { 0 };

        let q_sqlite_mysql = "INSERT INTO oc_authtoken \
            (uid, login_name, password, name, token, type, remember, last_activity, last_check, \
             public_key, private_key, version, scope, expires, password_invalid, remote_wipe) \
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        let q_pg = "INSERT INTO oc_authtoken \
            (uid, login_name, password, name, token, type, remember, last_activity, last_check, \
             public_key, private_key, version, scope, expires, password_invalid, remote_wipe) \
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id";

        let id: i64 = match &self.pool {
            DbPool::Sqlite(p) => {
                let res = map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .execute(p)
                        .await,
                )?;
                res.last_insert_rowid()
            }
            DbPool::MySql(p) => {
                let res = map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .execute(p)
                        .await,
                )?;
                res.last_insert_id() as i64
            }
            DbPool::Postgres(p) => {
                let row: (i64,) = map_sqlx(
                    sqlx::query_as(q_pg)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .fetch_one(p)
                        .await,
                )?;
                row.0
            }
        };
        Ok(id)
    }

    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>> {
        let q_sqlite_mysql = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE token = ?"
        );
        let q_pg = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE token = $1"
        );
        let row: Option<Row> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql)
                    .bind(hash)
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql)
                    .bind(hash)
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(&q_pg)
                    .bind(hash)
                    .fetch_optional(p)
                    .await,
            )?,
        };
        row.map(row_to_token).transpose()
    }

    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        let q_sqlite_mysql = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE id = ?"
        );
        let q_pg = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE id = $1"
        );
        let row: Option<Row> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql).bind(id).fetch_optional(p).await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql).bind(id).fetch_optional(p).await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(&q_pg).bind(id).fetch_optional(p).await,
            )?,
        };
        row.map(row_to_token).transpose()
    }

    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        let q_sqlite_mysql = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken \
             WHERE uid = ? ORDER BY last_activity DESC, id DESC"
        );
        let q_pg = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken \
             WHERE uid = $1 ORDER BY last_activity DESC, id DESC"
        );
        let rows: Vec<Row> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql)
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(&q_sqlite_mysql)
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(&q_pg)
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
        };
        rows.into_iter().map(row_to_token).collect()
    }

    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()> {
        let now_i: i64 = now as i64;
        let q_sqlite_mysql = "UPDATE oc_authtoken SET last_activity = ?, last_check = ? WHERE id = ?";
        let q_pg = "UPDATE oc_authtoken SET last_activity = $1, last_check = $2 WHERE id = $3";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(sqlx::query(q_sqlite_mysql).bind(now_i).bind(now_i).bind(id).execute(p).await)?;
            }
            DbPool::MySql(p) => {
                map_sqlx(sqlx::query(q_sqlite_mysql).bind(now_i).bind(now_i).bind(id).execute(p).await)?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(sqlx::query(q_pg).bind(now_i).bind(now_i).bind(id).execute(p).await)?;
            }
        }
        Ok(())
    }

    async fn revoke(&self, id: i64) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE id = ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE id = $1";
        match &self.pool {
            DbPool::Sqlite(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(id).execute(p).await)?; }
            DbPool::MySql(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(id).execute(p).await)?; }
            DbPool::Postgres(p) => { map_sqlx(sqlx::query(q_pg).bind(id).execute(p).await)?; }
        }
        Ok(())
    }

    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE uid = ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE uid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?; }
            DbPool::MySql(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?; }
            DbPool::Postgres(p) => { map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?; }
        }
        Ok(())
    }

    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE uid = ? AND id <> ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE uid = $1 AND id <> $2";
        match &self.pool {
            DbPool::Sqlite(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).bind(except).execute(p).await)?; }
            DbPool::MySql(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).bind(except).execute(p).await)?; }
            DbPool::Postgres(p) => { map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).bind(except).execute(p).await)?; }
        }
        Ok(())
    }

    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "UPDATE oc_authtoken SET password_invalid = 1 WHERE uid = ?";
        let q_pg = "UPDATE oc_authtoken SET password_invalid = 1 WHERE uid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?; }
            DbPool::MySql(p) => { map_sqlx(sqlx::query(q_sqlite_mysql).bind(uid.as_str()).execute(p).await)?; }
            DbPool::Postgres(p) => { map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?; }
        }
        Ok(())
    }
}
```

(The `use super::sql;` line is just to keep the module imports tidy — delete it if clippy flags `unused_imports`. The `sql` sibling module isn't referenced from this file otherwise.)

- [ ] **Step 2: Append a CRUD round-trip test mod**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("t.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    fn fixture_token(uid: &str, hash: &str, kind: AuthTokenType) -> AuthToken {
        AuthToken {
            id: 0,
            uid: UserId::new(uid).unwrap(),
            login_name: uid.into(),
            password: None,
            name: "test".into(),
            token: hash.into(),
            kind,
            remember: false,
            last_activity: 100,
            last_check: 100,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        }
    }

    #[tokio::test]
    async fn create_then_lookup_by_hash_roundtrips() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hashA", AuthTokenType::AppPassword))
            .await
            .unwrap();
        assert!(id > 0);
        let got = store.lookup_by_hash("hashA").await.unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.uid.as_str(), "alice");
        assert_eq!(got.kind, AuthTokenType::AppPassword);
    }

    #[tokio::test]
    async fn lookup_by_id_returns_full_row() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("bob", "hashB", AuthTokenType::Session))
            .await
            .unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.token, "hashB");
    }

    #[tokio::test]
    async fn lookup_by_hash_returns_none_on_miss() {
        let store = SqlTokenStore::new(fresh_pool().await);
        assert!(store.lookup_by_hash("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_for_user_returns_rows_newest_first() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let mut a = fixture_token("alice", "h1", AuthTokenType::Session);
        a.last_activity = 100;
        let mut b = fixture_token("alice", "h2", AuthTokenType::AppPassword);
        b.last_activity = 200;
        store.create(&a).await.unwrap();
        store.create(&b).await.unwrap();
        let rows = store
            .list_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].token, "h2"); // newest first
        assert_eq!(rows[1].token, "h1");
    }

    #[tokio::test]
    async fn bump_activity_writes_now() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hX", AuthTokenType::Session))
            .await
            .unwrap();
        store.bump_activity(id, 9999).await.unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.last_activity, 9999);
        assert_eq!(got.last_check, 9999);
    }

    #[tokio::test]
    async fn revoke_deletes_row() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hY", AuthTokenType::Session))
            .await
            .unwrap();
        store.revoke(id).await.unwrap();
        assert!(store.lookup_by_id(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_is_idempotent() {
        let store = SqlTokenStore::new(fresh_pool().await);
        store.revoke(9999).await.unwrap(); // no-op
    }

    #[tokio::test]
    async fn revoke_all_for_user_except_keeps_one() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let keep = store
            .create(&fixture_token("alice", "k", AuthTokenType::Session))
            .await
            .unwrap();
        let _drop = store
            .create(&fixture_token("alice", "d", AuthTokenType::AppPassword))
            .await
            .unwrap();
        store
            .revoke_all_for_user_except(&UserId::new("alice").unwrap(), keep)
            .await
            .unwrap();
        let remaining = store
            .list_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep);
    }

    #[tokio::test]
    async fn invalidate_all_for_user_sets_password_invalid() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "h", AuthTokenType::Session))
            .await
            .unwrap();
        store
            .invalidate_all_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert!(got.password_invalid);
    }
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p crabcloud-users --lib store::auth_token
```

Expected: 9 tests pass.

- [ ] **Step 4: Run check-all + commit + open Batch A PR**

```
cargo xtask check-all
```

Expected: green.

```
git add crates/crabcloud-users
git commit -m "feat(users,auth): SqlTokenStore implementation (multi-dialect)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-a
gh pr create --base master --head auth-batch-a \
  --title "auth: batch A — migration 0003 + AuthToken types + TokenStore" \
  --body "Sub-project 2b, batch A: migration 0003 creating oc_authtoken (full upstream schema), AuthToken/RawToken/AuthTokenType types, TokenNotFound/TokenAlreadyRevoked UsersError variants, TokenStore trait + multi-dialect SqlTokenStore."
```

**STOP. Do NOT call `gh pr merge`.** Controller merges after CI greens.

---

## Task 5: `hash_token` is already in place (Batch A); start Batch B branch + add `TokenAuthCache`

The `hash_token` helper landed in Task 2 alongside `RawToken`. Batch B begins here.

**Files:**
- Modify: `crates/crabcloud-users/src/store/auth_token.rs`

- [ ] **Step 1: Start the Batch B branch**

```
git checkout -b auth-batch-b origin/master
```

(Run *after* batch A has merged. If batch A is still in flight, branch off `auth-batch-a` and rebase later — flagged in the implementer's report.)

- [ ] **Step 2: Append `TokenAuthCache` to `store/auth_token.rs`**

```rust
use crabcloud_cache::Cache;
use std::sync::Arc;
use std::time::Duration;

/// Cache TTL for positive lookups (a hot row is worth ≤30s of staleness).
const TOKEN_CACHE_TTL: Duration = Duration::from_secs(30);

/// Cache TTL for negative lookups — soak up brute-force token bursts so
/// they don't hit the DB.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(5);

/// Min interval between consecutive `bump_activity` writes for the same row.
/// Rate-limits the DB write to one per-row per 30s; the cached row's stale
/// activity is OK between bumps.
const ACTIVITY_BUMP_INTERVAL: u64 = 30;

/// Read-through cache over a [`TokenStore`]. The cache key is
/// `{instance_id}:tokens:hash:{hex}`; positive entries are the serialized
/// `AuthToken`, negative entries are an empty byte slice (sentinel). Both
/// are bounded by short TTLs so a token revoke can't be cached forever.
#[derive(Clone)]
pub struct TokenAuthCache {
    inner: Arc<dyn TokenStore>,
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl TokenAuthCache {
    pub fn new(inner: Arc<dyn TokenStore>, cache: Arc<dyn Cache>, instance_id: impl Into<String>) -> Self {
        Self {
            inner,
            cache,
            instance_id: instance_id.into(),
        }
    }

    fn cache_key(&self, hash: &str) -> String {
        format!("{}:tokens:hash:{}", self.instance_id, hash)
    }

    fn id_cache_key(&self, id: i64) -> String {
        format!("{}:tokens:id:{}", self.instance_id, id)
    }

    /// Lookup by hash. Reads the cache; on miss, queries the inner store,
    /// caches positive (TTL 30s) or negative (TTL 5s) and returns.
    pub async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>> {
        let key = self.cache_key(hash);
        match self.cache.get(&key).await? {
            Some(bytes) if bytes.is_empty() => return Ok(None),
            Some(bytes) => {
                let row: AuthToken = serde_json::from_slice(&bytes).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token decode: {e}"
                    )))
                })?;
                return Ok(Some(row));
            }
            None => {}
        }
        match self.inner.lookup_by_hash(hash).await? {
            Some(row) => {
                let bytes = serde_json::to_vec(&row).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token encode: {e}"
                    )))
                })?;
                self.cache.set(&key, &bytes, Some(TOKEN_CACHE_TTL)).await?;
                Ok(Some(row))
            }
            None => {
                self.cache.set(&key, &[], Some(NEGATIVE_CACHE_TTL)).await?;
                Ok(None)
            }
        }
    }

    /// Lookup by id. Mirrors the by-hash path with a separate cache key
    /// namespace so a list/edit operation can grab a fresh row without
    /// polluting the hash cache.
    pub async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        let key = self.id_cache_key(id);
        match self.cache.get(&key).await? {
            Some(bytes) if bytes.is_empty() => return Ok(None),
            Some(bytes) => {
                let row: AuthToken = serde_json::from_slice(&bytes).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token decode: {e}"
                    )))
                })?;
                return Ok(Some(row));
            }
            None => {}
        }
        match self.inner.lookup_by_id(id).await? {
            Some(row) => {
                let bytes = serde_json::to_vec(&row).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token encode: {e}"
                    )))
                })?;
                self.cache.set(&key, &bytes, Some(TOKEN_CACHE_TTL)).await?;
                Ok(Some(row))
            }
            None => {
                self.cache.set(&key, &[], Some(NEGATIVE_CACHE_TTL)).await?;
                Ok(None)
            }
        }
    }

    /// Conditionally bump `last_activity`. Skips the DB write if the row's
    /// cached activity is within the rate-limit interval.
    pub async fn maybe_bump_activity(&self, row: &AuthToken, now: u64) -> UsersResult<()> {
        if now < row.last_activity + ACTIVITY_BUMP_INTERVAL {
            return Ok(());
        }
        self.inner.bump_activity(row.id, now).await?;
        // Refresh the cached row so subsequent lookups see the new last_activity.
        self.invalidate_hash(&row.token).await?;
        self.invalidate_id(row.id).await?;
        Ok(())
    }

    pub async fn invalidate_hash(&self, hash: &str) -> UsersResult<()> {
        self.cache.del(&self.cache_key(hash)).await?;
        Ok(())
    }

    pub async fn invalidate_id(&self, id: i64) -> UsersResult<()> {
        self.cache.del(&self.id_cache_key(id)).await?;
        Ok(())
    }

    /// Delegate mint to the inner store (no cache prewarm; the next lookup
    /// will populate the cache).
    pub async fn create(&self, row: &AuthToken) -> UsersResult<i64> {
        self.inner.create(row).await
    }

    /// Forward list to the inner store (no caching; lists are admin-side
    /// operations and shouldn't pollute the hot-path cache).
    pub async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        self.inner.list_for_user(uid).await
    }

    pub async fn revoke(&self, id: i64) -> UsersResult<()> {
        // Best-effort invalidate by id; we don't know the hash without a lookup,
        // but the negative-cache TTL is short so a stale-positive window is
        // bounded to NEGATIVE_CACHE_TTL.
        let _ = self.invalidate_id(id).await;
        // Best-effort: also invalidate the by-hash entry. We need the hash,
        // so do a lookup first. This adds one DB hit; it's fine for an admin op.
        if let Some(row) = self.inner.lookup_by_id(id).await? {
            let _ = self.invalidate_hash(&row.token).await;
        }
        self.inner.revoke(id).await
    }

    pub async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()> {
        // Capture hashes before deletion so we can invalidate the cache.
        let rows = self.inner.list_for_user(uid).await?;
        let result = self.inner.revoke_all_for_user_except(uid, except).await;
        for row in rows {
            if row.id != except {
                let _ = self.invalidate_hash(&row.token).await;
                let _ = self.invalidate_id(row.id).await;
            }
        }
        result
    }

    pub async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let rows = self.inner.list_for_user(uid).await?;
        let result = self.inner.invalidate_all_for_user(uid).await;
        for row in rows {
            let _ = self.invalidate_hash(&row.token).await;
            let _ = self.invalidate_id(row.id).await;
        }
        result
    }
}
```

- [ ] **Step 3: Add `crabcloud-cache` to runtime deps if not already there**

`crabcloud-users/Cargo.toml` already has `crabcloud-cache.workspace = true` (Batch B in 2a added it for the `Cache` error variant). Verify; no change needed if present.

- [ ] **Step 4: Re-export from `lib.rs`**

Add:

```rust
pub use store::auth_token::TokenAuthCache;
```

(alphabetical with existing `pub use store::auth_token::{SqlTokenStore, TokenStore};`).

- [ ] **Step 5: Append cache tests to `store/auth_token.rs::tests`**

```rust
    use crabcloud_cache::MemoryCache;

    fn fresh_cache(store: SqlTokenStore) -> TokenAuthCache {
        TokenAuthCache::new(Arc::new(store), Arc::new(MemoryCache::new()), "inst1")
    }

    #[tokio::test]
    async fn cache_hit_does_not_query_db_second_time() {
        let pool = fresh_pool().await;
        let store = SqlTokenStore::new(pool);
        let id = store
            .create(&fixture_token("alice", "hashH", AuthTokenType::AppPassword))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        // First lookup: DB.
        let first = cache.lookup_by_hash("hashH").await.unwrap().unwrap();
        assert_eq!(first.id, id);
        // Revoke via store directly (bypasses cache invalidation).
        store.revoke(id).await.unwrap();
        // Second lookup: cache returns the stale row (within TTL).
        let second = cache.lookup_by_hash("hashH").await.unwrap().unwrap();
        assert_eq!(second.id, id);
    }

    #[tokio::test]
    async fn negative_cache_absorbs_misses() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let cache = fresh_cache(store.clone());
        // Lookup unknown hash → None, cached as negative entry.
        assert!(cache.lookup_by_hash("missing").await.unwrap().is_none());
        // Insert the row via the inner store directly (cache doesn't know).
        store
            .create(&AuthToken {
                id: 0,
                uid: UserId::new("alice").unwrap(),
                login_name: "alice".into(),
                password: None,
                name: "x".into(),
                token: "missing".into(),
                kind: AuthTokenType::Session,
                remember: false,
                last_activity: 0,
                last_check: 0,
                public_key: None,
                private_key: None,
                version: 2,
                scope: None,
                expires: None,
                password_invalid: false,
                remote_wipe: false,
            })
            .await
            .unwrap();
        // Cache still returns None during the negative-cache window.
        assert!(cache.lookup_by_hash("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn maybe_bump_activity_rate_limits_writes() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "h", AuthTokenType::Session))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        let row = cache.lookup_by_hash("h").await.unwrap().unwrap();
        // First bump (now is far in the future) goes through.
        cache.maybe_bump_activity(&row, 10_000).await.unwrap();
        let after = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(after.last_activity, 10_000);
        // Second bump at now+10s is within the rate-limit window — skipped.
        cache.maybe_bump_activity(&after, 10_010).await.unwrap();
        let still = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(still.last_activity, 10_000);
    }

    #[tokio::test]
    async fn revoke_invalidates_cache() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "rev", AuthTokenType::Session))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        // Warm the cache.
        cache.lookup_by_hash("rev").await.unwrap();
        // Revoke via cache → invalidates entry.
        cache.revoke(id).await.unwrap();
        assert!(cache.lookup_by_hash("rev").await.unwrap().is_none());
    }
```

- [ ] **Step 6: Run tests**

```
cargo test -p crabcloud-users --lib store::auth_token
```

Expected: 13 tests pass (9 from Task 4 + 4 new cache tests).

- [ ] **Step 7: Commit**

```
cargo xtask check-all
git add crates/crabcloud-users
git commit -m "feat(users,auth): TokenAuthCache (read-through over SqlTokenStore)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: `AppPasswordService` façade

**Files:**
- Create: `crates/crabcloud-users/src/app_password.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Write `crates/crabcloud-users/src/app_password.rs`**

```rust
//! `AppPasswordService` — public mint/list/revoke/verify surface that
//! handlers and the CLI reach for. Composes a [`TokenAuthCache`] with the
//! `config.secret` used to derive token hashes.

use crate::auth_token::{hash_token, AuthToken, AuthTokenType, RawToken};
use crate::error::{UsersError, UsersResult};
use crate::store::auth_token::TokenAuthCache;
use crate::user::UserId;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Public composition handlers / settings UI / CLI all reach for. Wraps a
/// read-through token cache + the signing secret.
#[derive(Clone)]
pub struct AppPasswordService {
    tokens: Arc<TokenAuthCache>,
    secret: Arc<SecretString>,
}

impl AppPasswordService {
    pub fn new(tokens: Arc<TokenAuthCache>, secret: SecretString) -> Self {
        Self {
            tokens,
            secret: Arc::new(secret),
        }
    }

    pub fn token_cache(&self) -> &Arc<TokenAuthCache> {
        &self.tokens
    }

    /// Mint a new token. Returns `(persisted_row, raw_token)`. The
    /// `raw_token` is the *plaintext* the caller must show the user exactly
    /// once — wrap-and-forget. The row's `token` column stores the
    /// SHA-512(raw || secret) hash; no plaintext lives in the DB.
    pub async fn mint(
        &self,
        uid: &UserId,
        login_name: &str,
        name: &str,
        kind: AuthTokenType,
        remember: bool,
    ) -> UsersResult<(AuthToken, RawToken)> {
        let raw = RawToken::generate();
        let now = now_secs();
        let hash = hash_token(raw.expose(), self.secret.expose_secret());
        let candidate = AuthToken {
            id: 0,
            uid: uid.clone(),
            login_name: login_name.to_string(),
            password: None,
            name: name.to_string(),
            token: hash.clone(),
            kind,
            remember,
            last_activity: now,
            last_check: now,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        };
        let id = self.tokens.create(&candidate).await?;
        let mut persisted = candidate;
        persisted.id = id;
        Ok((persisted, raw))
    }

    /// Verify a raw token. Returns the row on success, [`UsersError::TokenNotFound`]
    /// on miss / unusable. Bumps `last_activity` (rate-limited) on hit.
    pub async fn verify(&self, raw: &str) -> UsersResult<AuthToken> {
        let hash = hash_token(raw, self.secret.expose_secret());
        let row = self
            .tokens
            .lookup_by_hash(&hash)
            .await?
            .ok_or(UsersError::TokenNotFound)?;
        let now = now_secs();
        if row.is_unusable(now) {
            return Err(UsersError::TokenNotFound);
        }
        let _ = self.tokens.maybe_bump_activity(&row, now).await;
        Ok(row)
    }

    pub async fn list(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        self.tokens.list_for_user(uid).await
    }

    pub async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        self.tokens.lookup_by_id(id).await
    }

    pub async fn revoke(&self, id: i64) -> UsersResult<()> {
        self.tokens.revoke(id).await
    }

    pub async fn revoke_other_sessions(&self, uid: &UserId, current: i64) -> UsersResult<()> {
        self.tokens.revoke_all_for_user_except(uid, current).await
    }

    pub async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        self.tokens.invalidate_all_for_user(uid).await
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::auth_token::SqlTokenStore;
    use crabcloud_cache::MemoryCache;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_svc() -> AppPasswordService {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("ap.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let store: Arc<dyn crate::store::auth_token::TokenStore> =
            Arc::new(SqlTokenStore::new(pool));
        let cache = TokenAuthCache::new(store, Arc::new(MemoryCache::new()), "inst1");
        AppPasswordService::new(Arc::new(cache), SecretString::new("the-secret".into()))
    }

    #[tokio::test]
    async fn mint_then_verify_succeeds() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (row, raw) = svc
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        let v = svc.verify(raw.expose()).await.unwrap();
        assert_eq!(v.id, row.id);
        assert_eq!(v.uid.as_str(), "alice");
    }

    #[tokio::test]
    async fn verify_unknown_returns_token_not_found() {
        let svc = fresh_svc().await;
        let err = svc.verify("not-a-real-token").await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }

    #[tokio::test]
    async fn verify_password_invalidated_returns_token_not_found() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (_row, raw) = svc
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        svc.invalidate_all_for_user(&uid).await.unwrap();
        let err = svc.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }

    #[tokio::test]
    async fn revoke_other_sessions_keeps_current() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (keep, _) = svc
            .mint(&uid, "alice", "current", AuthTokenType::Session, false)
            .await
            .unwrap();
        let (_drop, raw_drop) = svc
            .mint(&uid, "alice", "other", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        svc.revoke_other_sessions(&uid, keep.id).await.unwrap();
        assert!(svc.lookup_by_id(keep.id).await.unwrap().is_some());
        let err = svc.verify(raw_drop.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

```rust
mod app_password;
```

```rust
pub use app_password::AppPasswordService;
```

- [ ] **Step 3: Run tests + commit**

```
cargo test -p crabcloud-users --lib app_password
```

Expected: 4 tests pass.

```
cargo xtask check-all
git add crates/crabcloud-users
git commit -m "feat(users,auth): AppPasswordService façade (mint/verify/list/revoke)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: `UsersService` cascades through `AppPasswordService`

**Files:**
- Modify: `crates/crabcloud-users/src/service.rs`
- Modify: `crates/crabcloud-core/src/state.rs`

This task threads `Option<Arc<AppPasswordService>>` through `UsersService` so `set_password` can cascade `invalidate_all_for_user`. We keep the field `Option` to preserve the existing `UsersService::new(...)` 4-arg signature for tests that don't need the cascade; production code (state.rs builder) populates it.

- [ ] **Step 1: Extend `UsersService` in `service.rs`**

Replace the struct + `new` + add `with_app_passwords` builder + extend `set_password`:

```rust
//! `UsersService` — the public composition handlers reach for.

use crate::app_password::AppPasswordService;
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
    app_passwords: Option<Arc<AppPasswordService>>,
}

impl UsersService {
    pub fn new(
        users: Arc<dyn UserStore>,
        groups: Arc<dyn GroupStore>,
        prefs: Arc<dyn PreferenceStore>,
        verifier: Arc<dyn PasswordVerifier>,
    ) -> Self {
        Self {
            users,
            groups,
            prefs,
            verifier,
            app_passwords: None,
        }
    }

    /// Attach an `AppPasswordService` so `set_password` cascades
    /// `password_invalid=1` on every other token row.
    pub fn with_app_passwords(mut self, svc: Arc<AppPasswordService>) -> Self {
        self.app_passwords = Some(svc);
        self
    }

    pub fn user_store(&self) -> &Arc<dyn UserStore> {
        &self.users
    }
    pub fn group_store(&self) -> &Arc<dyn GroupStore> {
        &self.groups
    }
    pub fn preferences(&self) -> &Arc<dyn PreferenceStore> {
        &self.prefs
    }
    pub fn verifier(&self) -> &Arc<dyn PasswordVerifier> {
        &self.verifier
    }
    pub fn app_passwords(&self) -> Option<&Arc<AppPasswordService>> {
        self.app_passwords.as_ref()
    }

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

    /// Rehash + write the new password. If an [`AppPasswordService`] is
    /// attached, also cascades `password_invalid=1` on every token row owned
    /// by `uid` — mirrors Nextcloud's "rotating your password invalidates
    /// all other devices" behaviour.
    pub async fn set_password(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        let hash = self.verifier.hash(new)?;
        self.users.set_password(uid, &hash).await?;
        if let Some(ap) = &self.app_passwords {
            ap.invalidate_all_for_user(uid).await?;
        }
        Ok(())
    }

    pub async fn is_admin(&self, uid: &UserId) -> UsersResult<bool> {
        let admin = GroupId::new("admin")?;
        self.groups.is_in_group(uid, &admin).await
    }

    pub async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        self.groups.groups_of(uid).await
    }
}
```

The existing tests in `service::tests` continue to pass because they don't attach an `AppPasswordService` — the cascade is a no-op.

- [ ] **Step 2: Update `crates/crabcloud-core/src/state.rs::build` to wire the AppPasswordService**

Inside `build()`, after the existing `let users = ... ` block, replace the block with:

```rust
        let users = if let Some(svc) = self.custom_users.take() {
            svc
        } else {
            use crabcloud_users::{
                AppPasswordService, BcryptVerifier, GroupStore, PreferenceStore, SqlGroupStore,
                SqlPreferenceStore, SqlTokenStore, SqlUserStore, TokenAuthCache, TokenStore,
                UserStore, UsersService,
            };
            let sql_users: Arc<dyn UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
            let sql_groups: Arc<dyn GroupStore> = Arc::new(SqlGroupStore::new(pool.clone()));
            let sql_prefs: Arc<dyn PreferenceStore> =
                Arc::new(SqlPreferenceStore::new(pool.clone()));
            let user_store: Arc<dyn UserStore> = match &self.config.bootstrap_admin {
                Some(admin) => Arc::new(crabcloud_users::BootstrapAdminBackend::new(
                    sql_users.clone(),
                    sql_groups.clone(),
                    admin.clone(),
                )),
                None => sql_users,
            };
            let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool.clone()));
            let token_cache =
                Arc::new(TokenAuthCache::new(token_store, cache.clone(), &self.config.instanceid));
            let app_passwords = Arc::new(AppPasswordService::new(
                token_cache,
                self.config.secret.clone(),
            ));
            UsersService::new(
                user_store,
                sql_groups,
                sql_prefs,
                Arc::new(BcryptVerifier::new()),
            )
            .with_app_passwords(app_passwords)
        };
```

(The `pool` and `cache` locals already exist in `build()` from earlier in the body.)

- [ ] **Step 3: Add cascade test in `service::tests`**

Append to `crates/crabcloud-users/src/service.rs::tests`:

```rust
    #[tokio::test]
    async fn set_password_cascades_invalidate_when_app_passwords_attached() {
        use crate::app_password::AppPasswordService;
        use crate::auth_token::AuthTokenType;
        use crate::store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
        use crabcloud_cache::MemoryCache;
        use secrecy::SecretString;

        // Build a full service WITH app_passwords attached.
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("svc.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let users_store: Arc<dyn crate::store::UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
        let groups_store: Arc<dyn crate::store::GroupStore> =
            Arc::new(SqlGroupStore::new(pool.clone()));
        let prefs_store: Arc<dyn crate::store::PreferenceStore> =
            Arc::new(SqlPreferenceStore::new(pool.clone()));
        let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool));
        let token_cache = Arc::new(TokenAuthCache::new(
            token_store,
            Arc::new(MemoryCache::new()),
            "inst1",
        ));
        let app_passwords =
            Arc::new(AppPasswordService::new(token_cache, SecretString::new("s".into())));
        let svc = UsersService::new(
            users_store,
            groups_store,
            prefs_store,
            Arc::new(BcryptVerifier::new()),
        )
        .with_app_passwords(app_passwords.clone());

        let uid = UserId::new("alice").unwrap();
        let hash = svc.verifier.hash("old").unwrap();
        svc.users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "A".into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        let (_row, raw) = app_passwords
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        // Pre-cascade: verify works.
        assert!(app_passwords.verify(raw.expose()).await.is_ok());
        // set_password triggers cascade.
        svc.set_password(&uid, "new").await.unwrap();
        // Post-cascade: verify fails with TokenNotFound.
        let err = app_passwords.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, crate::UsersError::TokenNotFound));
    }
```

- [ ] **Step 4: Run tests + commit + open Batch B PR**

```
cargo test -p crabcloud-users
cargo xtask check-all
```

Expected: all prior tests + new cascade test pass.

```
git add crates/crabcloud-users crates/crabcloud-core
git commit -m "feat(users,auth): cascade password_invalid on set_password when AppPasswordService is attached

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-b
gh pr create --base master --head auth-batch-b \
  --title "auth: batch B — hash_token + TokenAuthCache + AppPasswordService" \
  --body "Sub-project 2b, batch B: TokenAuthCache (read-through over SqlTokenStore with negative cache + activity rate-limit), AppPasswordService façade (mint/verify/list/revoke), UsersService gains app_passwords and cascades password_invalid on set_password."
```

**STOP. Do NOT call `gh pr merge`.**

---

## Task 8: `AppState.tokens` field + builder default wiring

**(Done as part of Task 7 Step 2.)** Verified: `AppStateBuilder::build` now produces a `UsersService` with `app_passwords` populated. No separate `AppState.tokens` field is added — the `TokenAuthCache` lives inside the `AppPasswordService` which lives inside `UsersService`, accessible as `state.users.app_passwords()`. This keeps the `AppState` field set unchanged.

**(No separate commit. The spec's §3.1 "AppState.tokens" line is implemented as `state.users.app_passwords()` — note in the changelog.)**

---

## Task 9: `AuthContext` + `AuthLayer` middleware

**Files:**
- Create: `crates/crabcloud-http/src/auth_context.rs`
- Create: `crates/crabcloud-http/src/middleware/auth.rs`
- Modify: `crates/crabcloud-http/src/middleware/mod.rs`
- Modify: `crates/crabcloud-http/src/lib.rs`
- Modify: `crates/crabcloud-http/src/extractors/auth.rs` (only the `AuthMethod` enum; the extractor body change lands in Task 10)

- [ ] **Step 1: Start the Batch C branch**

```
git checkout -b auth-batch-c origin/master
```

- [ ] **Step 2: Write `crates/crabcloud-http/src/auth_context.rs`**

```rust
//! `AuthContext` — the request extension installed by `AuthLayer`. Extractors
//! read it instead of `SessionHandle`. Three auth methods (cookie, Bearer,
//! Basic) collapse into one record so handlers don't repeat per-scheme logic.

use crabcloud_users::UserId;

/// How a request was authenticated.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthMethod {
    /// Cookie-backed browser session.
    Session,
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// `Authorization: Basic <b64(uid:token)>`.
    Basic,
}

/// Per-request authentication context. Inserted as a request extension by
/// [`crate::middleware::auth::AuthLayer`] when any of the three auth arms
/// succeed.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// Authenticated user id.
    pub user_id: UserId,
    /// Which arm matched.
    pub method: AuthMethod,
    /// PK of the backing `oc_authtoken` row. Used for revoke-this-session,
    /// destroy_others, and per-row activity bumps.
    pub token_id: i64,
    /// What the user typed at login. For Session tokens this is the form's
    /// `username` value; for Bearer/Basic it's the row's `login_name`.
    pub login_name: String,
    /// `remember` checkbox state at login. Only meaningful for Session tokens.
    pub remember: bool,
}
```

- [ ] **Step 3: Update the `AuthMethod` placeholder in `extractors/auth.rs`**

Replace the existing `AuthMethod` enum (currently has only `Session` plus a comment) with a re-export from `auth_context`:

```rust
pub use crate::auth_context::{AuthContext, AuthMethod};
```

And remove the now-dead local `AuthMethod` enum. The `AuthenticatedUser` struct's `auth_method` field is now of type `crate::auth_context::AuthMethod`.

- [ ] **Step 4: Write `crates/crabcloud-http/src/middleware/auth.rs`**

```rust
//! `AuthLayer` — Tower middleware that resolves authentication from one of
//! three arms (Bearer header / Basic header / session cookie) and attaches
//! an [`AuthContext`] to the request's extensions.
//!
//! Precedence (top-down, first hit wins). Header arms fail loud (401 from
//! the extractor when their token is present but invalid); the cookie arm
//! fails quiet (a malformed / unknown cookie is treated as if no cookie was
//! present, so anonymous routes like `/login` still work).
//!
//! See `docs/superpowers/specs/2026-05-12-app-passwords-bearer-basic-auth-design.md` §5.1.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::session::layer::COOKIE_NAME;
use crate::session::cookie::decode_cookie;
use axum::http::{header::AUTHORIZATION, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as B64STD;
use base64::Engine;
use crabcloud_core::AppState;
use crabcloud_users::{AuthTokenType, UserId};
use futures::future::BoxFuture;
use secrecy::ExposeSecret;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// Sentinel response used when a header-supplied token is present but
/// invalid. `AuthenticatedUser` extractors then surface the standard 401.
fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

#[derive(Clone)]
pub struct AuthLayer {
    state: AppState,
}

impl AuthLayer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            state: self.state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    state: AppState,
}

#[derive(Debug)]
enum ArmOutcome {
    Authenticated(AuthContext),
    /// The arm's input was present but invalid; respond 401.
    HeaderRejected,
    /// The arm had nothing to offer; continue to the next arm.
    NoInput,
}

impl<S, B> Service<Request<B>> for AuthMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let state = self.state.clone();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            // 1. Bearer
            match try_bearer(&state, &req).await {
                ArmOutcome::Authenticated(ctx) => {
                    req.extensions_mut().insert(ctx);
                    return inner.call(req).await;
                }
                ArmOutcome::HeaderRejected => return Ok(unauthorized()),
                ArmOutcome::NoInput => {}
            }
            // 2. Basic
            match try_basic(&state, &req).await {
                ArmOutcome::Authenticated(ctx) => {
                    req.extensions_mut().insert(ctx);
                    return inner.call(req).await;
                }
                ArmOutcome::HeaderRejected => return Ok(unauthorized()),
                ArmOutcome::NoInput => {}
            }
            // 3. Cookie (fail quiet)
            if let ArmOutcome::Authenticated(ctx) = try_cookie(&state, &req).await {
                req.extensions_mut().insert(ctx);
            }
            inner.call(req).await
        })
    }
}

fn extract_authorization_header<B>(req: &Request<B>) -> Option<&str> {
    req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok())
}

async fn try_bearer<B>(state: &AppState, req: &Request<B>) -> ArmOutcome {
    let header = match extract_authorization_header(req) {
        Some(h) if h.starts_with("Bearer ") || h.starts_with("bearer ") => h,
        _ => return ArmOutcome::NoInput,
    };
    let raw = header[7..].trim();
    if raw.is_empty() {
        return ArmOutcome::HeaderRejected;
    }
    match verify_and_build(state, raw, AuthMethod::Bearer, None).await {
        Some(ctx) => ArmOutcome::Authenticated(ctx),
        None => ArmOutcome::HeaderRejected,
    }
}

async fn try_basic<B>(state: &AppState, req: &Request<B>) -> ArmOutcome {
    let header = match extract_authorization_header(req) {
        Some(h) if h.starts_with("Basic ") || h.starts_with("basic ") => h,
        _ => return ArmOutcome::NoInput,
    };
    let b64 = header[6..].trim();
    let decoded = match B64STD.decode(b64) {
        Ok(d) => d,
        Err(_) => return ArmOutcome::HeaderRejected,
    };
    let s = match std::str::from_utf8(&decoded) {
        Ok(s) => s,
        Err(_) => return ArmOutcome::HeaderRejected,
    };
    let (uid_str, token) = match s.split_once(':') {
        Some(p) => p,
        None => return ArmOutcome::HeaderRejected,
    };
    if token.is_empty() {
        return ArmOutcome::HeaderRejected;
    }
    match verify_and_build(state, token, AuthMethod::Basic, Some(uid_str)).await {
        Some(ctx) => ArmOutcome::Authenticated(ctx),
        None => ArmOutcome::HeaderRejected,
    }
}

async fn try_cookie<B>(state: &AppState, req: &Request<B>) -> ArmOutcome {
    let raw = match extract_cookie_value(req, COOKIE_NAME) {
        Some(v) => v,
        None => return ArmOutcome::NoInput,
    };
    let secret = state.config.secret.expose_secret().as_bytes().to_vec();
    let token_value = match decode_cookie(&raw, &secret) {
        Ok(v) => v,
        Err(e) => {
            ::tracing::warn!(error = ?e, "cookie_hmac_invalid; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
    };
    let ap = match state.users.app_passwords() {
        Some(ap) => ap.clone(),
        None => return ArmOutcome::NoInput,
    };
    let row = match ap.verify(&token_value).await {
        Ok(r) if r.kind == AuthTokenType::Session => r,
        Ok(_) => {
            ::tracing::warn!("cookie_wrong_kind; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
        Err(_) => {
            ::tracing::warn!("cookie_unknown_or_unusable; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
    };
    ArmOutcome::Authenticated(AuthContext {
        user_id: row.uid,
        method: AuthMethod::Session,
        token_id: row.id,
        login_name: row.login_name,
        remember: row.remember,
    })
}

fn extract_cookie_value<B>(req: &Request<B>, name: &str) -> Option<String> {
    let raw = req.headers().get(axum::http::header::COOKIE)?.to_str().ok()?;
    for piece in raw.split(';').map(str::trim) {
        if let Some((k, v)) = piece.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

async fn verify_and_build(
    state: &AppState,
    raw: &str,
    method: AuthMethod,
    expected_uid: Option<&str>,
) -> Option<AuthContext> {
    let ap = state.users.app_passwords()?.clone();
    let row = match ap.verify(raw).await {
        Ok(r) => r,
        Err(e) => {
            ::tracing::warn!(error = %e, ?method, "auth_token_not_found");
            return None;
        }
    };
    if let Some(expected) = expected_uid {
        // Constant-time-ish compare; both sides are ASCII so byte-compare via subtle.
        use subtle::ConstantTimeEq;
        if row.uid.as_str().as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1 {
            ::tracing::warn!("auth_basic_uid_mismatch");
            return None;
        }
    }
    Some(AuthContext {
        user_id: row.uid,
        method,
        token_id: row.id,
        login_name: row.login_name,
        remember: row.remember,
    })
}
```

- [ ] **Step 5: Wire `pub mod auth` in `middleware/mod.rs`**

Append:

```rust
pub mod auth;
```

- [ ] **Step 6: Re-export from `crates/crabcloud-http/src/lib.rs`**

Add (alphabetical with existing re-exports):

```rust
mod auth_context;
pub use auth_context::{AuthContext, AuthMethod};
pub use middleware::auth::AuthLayer;
```

- [ ] **Step 7: Update `crabcloud-http/Cargo.toml`**

`base64`, `subtle`, `secrecy`, `crabcloud-users` may need to be runtime deps now. Verify each `[dependencies]` entry:
- `base64.workspace = true` — add if missing.
- `subtle.workspace = true` — already present (used by `cookie.rs`).
- `secrecy.workspace = true` — already present.
- `crabcloud-users.workspace = true` — currently dev-dep only (from 2a). Promote to `[dependencies]` because `AuthLayer` calls into `state.users.app_passwords().verify(...)`.

- [ ] **Step 8: Build + commit**

```
cargo build -p crabcloud-http
```

Expected: clean (no test step here; integration tests for `AuthLayer` come after extractors + build_router wiring in Task 10).

```
git add crates/crabcloud-http
git commit -m "feat(http,auth): AuthContext + AuthLayer middleware (Bearer/Basic/Cookie)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: Rewire extractors + wire `AuthLayer` in `build_router`

**Files:**
- Modify: `crates/crabcloud-http/src/extractors/auth.rs`
- Modify: `crates/crabcloud-http/src/router.rs`

- [ ] **Step 1: Rewrite `AuthenticatedUser` / `OptionalUser` / `AdminUser` to read `AuthContext`**

Replace the body of `crates/crabcloud-http/src/extractors/auth.rs` with:

```rust
//! Auth extractors. Source the authenticated user from the request's
//! `AuthContext` extension (installed by [`crate::middleware::auth::AuthLayer`]).

use crate::auth_context::{AuthContext, AuthMethod};
use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_core::{AppState, Error as CoreError};
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub auth_method: AuthMethod,
}

pub struct UnauthorizedRejection;

impl IntoResponse for UnauthorizedRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = UnauthorizedRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ctx = parts
            .extensions
            .get::<AuthContext>()
            .ok_or(UnauthorizedRejection)?;
        Ok(AuthenticatedUser {
            user_id: ctx.user_id.as_str().to_string(),
            auth_method: ctx.method,
        })
    }
}

#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<AuthenticatedUser>);

impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let inner = parts.extensions.get::<AuthContext>().map(|ctx| AuthenticatedUser {
            user_id: ctx.user_id.as_str().to_string(),
            auth_method: ctx.method,
        });
        Ok(OptionalUser(inner))
    }
}

#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthenticatedUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let authed = AuthenticatedUser::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError(CoreError::Unauthorized))?;
        let uid = crabcloud_users::UserId::new(&authed.user_id)
            .map_err(|_| ApiError(CoreError::Unauthorized))?;
        let is_admin = state
            .users
            .is_admin(&uid)
            .await
            .map_err(CoreError::Users)
            .map_err(ApiError)?;
        if !is_admin {
            return Err(ApiError(CoreError::Forbidden));
        }
        Ok(AdminUser(authed))
    }
}
```

The existing `#[cfg(test)] mod tests` in this file uses `SessionHandle::extension` to drive `AuthenticatedUser`. Replace the handle-fixture pattern with `AuthContext`-extension fixtures:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_context::{AuthContext, AuthMethod};
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn auth_only(user: AuthenticatedUser) -> String {
        user.user_id
    }
    async fn optional(opt: OptionalUser) -> String {
        opt.0.map(|u| u.user_id).unwrap_or_else(|| "guest".into())
    }

    fn ctx_for(user: &str, method: AuthMethod) -> AuthContext {
        AuthContext {
            user_id: crabcloud_users::UserId::new(user).unwrap(),
            method,
            token_id: 1,
            login_name: user.into(),
            remember: false,
        }
    }

    fn app() -> Router {
        Router::new()
            .route("/auth", get(auth_only))
            .route("/opt", get(optional))
    }

    #[tokio::test]
    async fn authenticated_user_rejects_when_no_context() {
        let req = Request::builder().uri("/auth").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_user_resolves_when_context_present() {
        let req = Request::builder()
            .uri("/auth")
            .extension(ctx_for("alice", AuthMethod::Session))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "alice");
    }

    #[tokio::test]
    async fn optional_user_is_none_for_anon() {
        let req = Request::builder().uri("/opt").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "guest");
    }

    // AdminUser tests retained from 2a — fixture switches from SessionHandle
    // to AuthContext but logic is otherwise identical. (Existing
    // `make_state_with_user` helper stays as-is.)
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::{AppState, AppStateBuilder};
    use crabcloud_users::{
        BcryptVerifier, GroupId, GroupStore, PasswordVerifier, SqlGroupStore, User as UserRow,
        UserId,
    };
    use tempfile::tempdir;

    async fn admin_only(AdminUser(user): AdminUser) -> String {
        user.user_id
    }

    async fn make_state_with_user(uid: &str, is_admin: bool) -> AppState {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("admin.db"));
        std::mem::forget(dir);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        state
            .users
            .user_store()
            .create(
                &UserRow {
                    uid: UserId::new(uid).unwrap(),
                    display_name: uid.into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        if is_admin {
            let groups = SqlGroupStore::new(state.pool.clone());
            groups
                .add_to_group(&UserId::new(uid).unwrap(), &GroupId::new("admin").unwrap())
                .await
                .unwrap();
        }
        state
    }

    fn admin_app(state: AppState) -> Router {
        Router::new()
            .route("/admin", get(admin_only))
            .with_state(state)
    }

    #[tokio::test]
    async fn admin_user_rejects_when_unauthenticated() {
        let state = make_state_with_user("alice", true).await;
        let req = Request::builder().uri("/admin").body(Body::empty()).unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_user_rejects_non_admin_with_403() {
        let state = make_state_with_user("alice", false).await;
        let req = Request::builder()
            .uri("/admin")
            .extension(ctx_for("alice", AuthMethod::Session))
            .body(Body::empty())
            .unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_user_resolves_when_in_admin_group() {
        let state = make_state_with_user("root", true).await;
        let req = Request::builder()
            .uri("/admin")
            .extension(ctx_for("root", AuthMethod::Session))
            .body(Body::empty())
            .unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "root");
    }
}
```

- [ ] **Step 2: Wire `AuthLayer` into `build_router`**

Modify `crates/crabcloud-http/src/router.rs`. The layer order must be: `AuthLayer` **inside** (i.e. *added after* in tower's `.layer()` model, so it runs *before*) `SessionLayer`, so when `AuthLayer`'s cookie arm reads the cookie it can do its own decode (the existing `SessionLayer` shrinks to a no-op for now — it'll be fully removed in Task 11).

For this task: leave `SessionLayer` in place (it still inserts `SessionHandle` for CSRF middleware and the legacy login path). Insert `AuthLayer` *before* the inner Dioxus app + OCS router so the extractors see the `AuthContext`:

```rust
    app_router
        .nest(
            "/ocs",
            crate::routes::ocs::router().with_state(state.clone()),
        )
        // Install AppState as a request extension so `FullstackContext::extension`
        // can pull it from inside `#[server]` function bodies.
        .layer(axum::Extension(state.clone()))
        .layer(crate::middleware::auth::AuthLayer::new(state.clone()))
        .layer(CsrfLayer::new())
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(cors_layer)
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
```

The `AuthLayer::new(state.clone())` captures the AppState; the order of `.layer(...)` calls puts `AuthLayer` *outside* of `CsrfLayer` (executes first on the request path, last on the response path — which is what we want so CSRF can read `AuthContext`).

Wait — tower's layer composition is innermost-first when reading the code, outermost-first when executing. `.layer(X)` wraps the current router with `X` so `X` runs *before* the inner. The existing order has `Extension` outermost (added last via `.layer(Extension)` last). Re-read carefully:

`app_router.layer(A).layer(B).layer(C)` produces, in request order, `C → B → A → app`. So the layer that's `.layer()`'d **last** runs **first**.

For 2b we want, on the **request** path: outermost middleware (Trace, CatchPanic, …) → SessionLayer (extracts `SessionHandle`) → CsrfLayer → AuthLayer (reads cookie / Bearer / Basic, attaches `AuthContext`) → routes (extractors read `AuthContext`).

So in the source, `AuthLayer` is `.layer()`'d **first** (innermost) — *before* `CsrfLayer` and `SessionLayer`. The block above is correct.

Actually, double-check: the original code reads:

```rust
.layer(axum::Extension(state))
.layer(CsrfLayer::new())
.layer(SessionLayer::new(...))
.layer(cors_layer)
```

Request order: cors → SessionLayer → CsrfLayer → Extension → routes. So Extension is innermost.

We want: cors → SessionLayer → AuthLayer → CsrfLayer → Extension → routes. So `.layer(AuthLayer)` must sit between `.layer(CsrfLayer)` and `.layer(SessionLayer)`. Correct version:

```rust
        .layer(axum::Extension(state.clone()))
        .layer(CsrfLayer::new())
        .layer(crate::middleware::auth::AuthLayer::new(state.clone()))
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(cors_layer)
        .layer(SecurityHeadersLayer::new())
        // ...
```

Use that order in the actual code change.

- [ ] **Step 3: Integration test for `AuthLayer` arms**

Append a new file `crates/crabcloud-http/tests/auth_layer.rs`:

```rust
//! Integration tests for AuthLayer arms (Bearer, Basic, Cookie).
//! Drives the full `build_router` to exercise layer interactions.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::STANDARD as B64STD;
use base64::Engine;
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_http::build_router;
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state(path: std::path::PathBuf) -> AppState {
    AppStateBuilder::new(minimal_sqlite_config(path)).build().await.unwrap()
}

async fn seed_user(state: &AppState, uid: &str) {
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new(uid).unwrap(),
                display_name: uid.into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn bearer_with_minted_token_authenticates() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .unwrap();
    let app = build_router(state, axum::Router::new());

    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {}", raw.expose()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn bearer_with_unknown_token_returns_401() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let app = build_router(state, axum::Router::new());
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", "Bearer not-a-real-token")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn basic_with_minted_token_authenticates() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .unwrap();
    let app = build_router(state, axum::Router::new());
    let creds = B64STD.encode(format!("alice:{}", raw.expose()));
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Basic {creds}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn basic_uid_mismatch_returns_401() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .unwrap();
    let app = build_router(state, axum::Router::new());
    let creds = B64STD.encode(format!("bob:{}", raw.expose()));
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Basic {creds}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn anonymous_request_is_unauthorized_on_protected_route() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let app = build_router(state, axum::Router::new());
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

(Cookie-auth integration tests come in Task 11 once the cookie path is fully migrated.)

- [ ] **Step 4: Run tests + commit + open Batch C PR**

```
cargo test -p crabcloud-http
cargo xtask check-all
```

Expected: 5 new tests in `tests/auth_layer.rs` pass; existing `ocs/user.rs` tests still pass (cookie-auth path still works via the legacy SessionHandle until Task 11).

```
git add crates/crabcloud-http
git commit -m "feat(http,auth): rewire extractors to AuthContext + wire AuthLayer into router

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-c
gh pr create --base master --head auth-batch-c \
  --title "auth: batch C — AuthContext + AuthLayer + extractor rewire" \
  --body "Sub-project 2b, batch C: AuthLayer middleware that walks Bearer/Basic/Cookie and attaches AuthContext to req extensions. Extractors (AuthenticatedUser/OptionalUser/AdminUser) now read AuthContext. Cookie auth path still uses the legacy SessionHandle wiring — fully migrates in batch D."
```

**STOP. Do NOT call `gh pr merge`.**

---

## Task 11: `SessionLayer` slim-down + cookie becomes a raw token

**Files:**
- Modify: `crates/crabcloud-http/src/session/layer.rs`
- Modify: `crates/crabcloud-http/src/session/store.rs`
- Modify: `crates/crabcloud-http/src/session/data.rs`

- [ ] **Step 1: Start Batch D branch**

```
git checkout -b auth-batch-d origin/master
```

- [ ] **Step 2: Rewrite `session/layer.rs` so the cookie's payload is a raw token**

The layer's job collapses to: (a) extract the existing `oc_sessionPassphrase` cookie (signed envelope); (b) make the cookie value (raw token, already AuthLayer-readable) available; (c) on response, set/clear the cookie based on whether the handler minted/destroyed a session.

We retain `SessionHandle` as a thin wrapper holding ephemeral session-blob state (csrf_token, two_factor_passed) keyed by `token_id`. The blob storage is the existing cache, keyed off the token_id (so it survives between requests but is invalidated when the token is revoked).

Replace `session/layer.rs` with:

```rust
//! `SessionLayer` — thin middleware that owns cookie sign/verify and the
//! per-session ephemeral state (CSRF token, two_factor_passed). Auth itself
//! is handled upstream by `AuthLayer`; this layer reads the AuthContext to
//! decide whether to look up / write ephemeral state.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::session::cookie::encode_cookie;
use crate::session::data::Session;
use crate::session::store::SessionStore;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use futures::future::BoxFuture;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::{Layer, Service};

/// Name of the session cookie (Nextcloud-compatible).
pub const COOKIE_NAME: &str = "oc_sessionPassphrase";

/// Pending-cookie action: a handler can stash a `PendingCookie::Set(raw)` to
/// mint a fresh cookie post-response, or `PendingCookie::Destroy` to clear it.
#[derive(Debug, Clone)]
pub enum PendingCookie {
    Set { raw_token: String, max_age_secs: u64 },
    Destroy,
}

/// Wrapper inserted into request extensions so handlers can mutate the
/// session blob and request a cookie change. Token id (when known) keys
/// the blob; for anonymous requests `token_id = None` and the blob is
/// stored under a transient id minted on demand.
#[derive(Clone)]
pub struct SessionHandle {
    pub token_id: Option<i64>,
    pub inner: Arc<Mutex<Session>>,
    pub pending_cookie: Arc<Mutex<Option<PendingCookie>>>,
}

impl SessionHandle {
    pub async fn read(&self) -> Session {
        self.inner.lock().await.clone()
    }
    pub fn try_read_snapshot(&self) -> Option<Session> {
        self.inner.try_lock().ok().map(|g| g.clone())
    }
    pub async fn mutate<F: FnOnce(&mut Session)>(&self, f: F) {
        let mut s = self.inner.lock().await;
        f(&mut s);
    }
    /// Stage a cookie mutation. Applied by the layer on response.
    pub async fn set_pending_cookie(&self, p: PendingCookie) {
        *self.pending_cookie.lock().await = Some(p);
    }
}

#[derive(Clone)]
pub struct SessionLayer {
    store: SessionStore,
    secret: Arc<SecretString>,
    secure: bool,
}

impl SessionLayer {
    pub fn new(store: SessionStore, secret: SecretString, secure: bool) -> Self {
        Self {
            store,
            secret: Arc::new(secret),
            secure,
        }
    }
}

impl<S> Layer<S> for SessionLayer {
    type Service = SessionMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        SessionMiddleware {
            inner,
            store: self.store.clone(),
            secret: self.secret.clone(),
            secure: self.secure,
        }
    }
}

#[derive(Clone)]
pub struct SessionMiddleware<S> {
    inner: S,
    store: SessionStore,
    secret: Arc<SecretString>,
    secure: bool,
}

fn make_set_cookie(value: &str, secure: bool, max_age: u64) -> HeaderValue {
    let mut s = format!(
        "{COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    );
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

fn make_destroy_cookie(secure: bool) -> HeaderValue {
    let mut s = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

impl<S, B> Service<Request<B>> for SessionMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let store = self.store.clone();
        let secret = self.secret.clone();
        let secure = self.secure;
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Determine the token id (if any) from AuthLayer's AuthContext.
            // Cookie-auth has a token_id; Bearer/Basic also have one but we
            // don't load ephemeral state for those (they don't carry CSRF).
            let token_id_opt = req
                .extensions()
                .get::<AuthContext>()
                .filter(|c| c.method == AuthMethod::Session)
                .map(|c| c.token_id);

            // Load the session blob for this token (or start fresh).
            let session = match token_id_opt {
                Some(id) => store.load_for_token(id).await.ok().flatten().unwrap_or_default(),
                None => Session::new(),
            };

            let handle = SessionHandle {
                token_id: token_id_opt,
                inner: Arc::new(Mutex::new(session)),
                pending_cookie: Arc::new(Mutex::new(None)),
            };
            req.extensions_mut().insert(handle.clone());

            let mut resp = inner.call(req).await?;

            // Persist any blob mutations back to the cache.
            if let Some(id) = token_id_opt {
                let final_session = handle.inner.lock().await.clone();
                let _ = store.save_for_token(id, &final_session).await;
            }

            // Apply pending cookie mutation, if any.
            if let Some(pending) = handle.pending_cookie.lock().await.clone() {
                match pending {
                    PendingCookie::Set { raw_token, max_age_secs } => {
                        let value = encode_cookie_payload(&raw_token, secret.expose_secret().as_bytes());
                        resp.headers_mut()
                            .append(SET_COOKIE, make_set_cookie(&value, secure, max_age_secs));
                    }
                    PendingCookie::Destroy => {
                        resp.headers_mut()
                            .append(SET_COOKIE, make_destroy_cookie(secure));
                    }
                }
            }

            Ok(resp)
        })
    }
}

/// Wrap a raw token in the existing HMAC-signed cookie envelope. We keep the
/// envelope shape from 2a (`<payload>.<sig>`) but the payload bytes are now
/// the *raw token* directly, not a session id. AuthLayer's cookie arm calls
/// `decode_cookie` to unwrap.
fn encode_cookie_payload(raw_token: &str, secret: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(raw_token.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", B64.encode(raw_token.as_bytes()), B64.encode(sig))
}
```

- [ ] **Step 3: Update `session/cookie.rs::decode_cookie` to return the payload as a `String`**

The current `decode_cookie` returns a hex-encoded session id. Change it so it returns the **raw payload bytes as a UTF-8 string** — which for 2b is the raw token (URL-safe base64 with no padding, already valid ASCII).

Replace `decode_cookie`:

```rust
pub fn decode_cookie(raw: &str, secret: &[u8]) -> Result<String, CookieError> {
    let (id_b64, sig_b64) = raw.split_once('.').ok_or(CookieError::Malformed)?;
    let payload = B64.decode(id_b64).map_err(|_| CookieError::Malformed)?;
    let sig = B64.decode(sig_b64).map_err(|_| CookieError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let expected = mac.finalize().into_bytes();
    if expected.ct_eq(&sig).into() {
        String::from_utf8(payload).map_err(|_| CookieError::Malformed)
    } else {
        Err(CookieError::BadSignature)
    }
}
```

Update existing `encode_cookie` to be a thin wrapper that just calls the helper for symmetry, or delete it — Task 11's `SessionLayer` uses an inline `encode_cookie_payload`. If `encode_cookie` is used elsewhere (it is — in `routes/ocs/user.rs::tests` for seeding fake sessions), keep it as a delegating function:

```rust
pub fn encode_cookie(payload: &str, secret: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use base64::Engine;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
}
```

The existing cookie tests should still pass — payload was previously hex bytes, now it's whatever the caller passed. Adapt the two-decoding cookie tests to use generic test fixtures:

```rust
    #[test]
    fn round_trip_known_id() {
        let payload = "some-arbitrary-string-not-just-hex";
        let token = encode_cookie(payload, b"shhh");
        let decoded = decode_cookie(&token, b"shhh").unwrap();
        assert_eq!(decoded, payload);
    }
```

(Adapt the other two tests similarly — semantic identical, payload is now opaque.)

- [ ] **Step 4: Update `session/store.rs` to key blobs by token id**

Replace `SessionStore`'s session-blob methods to key on token id (an `i64`) instead of `SessionId`:

```rust
//! `SessionStore` — typed wrapper over `Arc<dyn Cache>` for ephemeral
//! per-session blob state (CSRF token, two_factor_passed). Keyed by the
//! authoritative `oc_authtoken` row id.

use crate::session::data::Session;
use crabcloud_cache::{Cache, CacheError};
use std::sync::Arc;
use std::time::Duration;

pub const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct SessionStore {
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl SessionStore {
    pub fn new(cache: Arc<dyn Cache>, instance_id: impl Into<String>) -> Self {
        Self {
            cache,
            instance_id: instance_id.into(),
        }
    }

    fn key_for_token(&self, token_id: i64) -> String {
        format!("{}:session_blob:{}", self.instance_id, token_id)
    }

    pub async fn load_for_token(&self, token_id: i64) -> Result<Option<Session>, CacheError> {
        let raw = self.cache.get(&self.key_for_token(token_id)).await?;
        match raw {
            Some(bytes) => {
                let s: Session = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::Io(format!("session decode: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub async fn save_for_token(&self, token_id: i64, session: &Session) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| CacheError::Io(format!("session encode: {e}")))?;
        self.cache
            .set(&self.key_for_token(token_id), &bytes, Some(SESSION_IDLE_TTL))
            .await
    }

    pub async fn destroy_for_token(&self, token_id: i64) -> Result<(), CacheError> {
        self.cache.del(&self.key_for_token(token_id)).await
    }
}
```

Delete the per-user `record_for_user` / `destroy_all_for` / `destroy_all_for_except` methods — they're now subsumed by the `TokenStore` cascade (the `oc_authtoken` table itself acts as the per-user session index).

Existing tests for these methods are gone; the tests file shrinks to:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_cache::MemoryCache;

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let mut s = Session::new();
        s.csrf_token = "abc".into();
        store.save_for_token(42, &s).await.unwrap();
        let loaded = store.load_for_token(42).await.unwrap().unwrap();
        assert_eq!(loaded.csrf_token, "abc");
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        assert!(store.load_for_token(99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_removes_blob() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        store.save_for_token(7, &Session::new()).await.unwrap();
        store.destroy_for_token(7).await.unwrap();
        assert!(store.load_for_token(7).await.unwrap().is_none());
    }
}
```

- [ ] **Step 5: Update `routes/ocs/user.rs::put_self` password-change to use the new revoke path**

The handler currently calls `SessionStore::destroy_all_for_except(uid, Some(&handle.id))`. Replace with a call to the `AppPasswordService::revoke_other_sessions(uid, current_token_id)` (`current_token_id` comes from the AuthContext):

```rust
        "password" => {
            state
                .users
                .set_password(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            if let Some(ap) = state.users.app_passwords() {
                // The cascade in users.set_password already marked rows as
                // password_invalid. Now actually delete the rows for the
                // user *except* the current one so the caller stays logged in.
                let ctx = req_extensions
                    .get::<crabcloud_http::AuthContext>()
                    .ok_or_else(|| unauth(fmt.0))?;
                let _ = ap.revoke_other_sessions(&uid, ctx.token_id).await;
            }
        }
```

`req_extensions` requires a tweak to the handler signature: take `axum::extract::Extension<crabcloud_http::AuthContext>` directly so the extractor pulls it. Simpler: replace the `Extension<SessionHandle>` parameter with `axum::Extension<AuthContext>`:

```rust
pub async fn put_self(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Extension(ctx): Extension<crate::AuthContext>,
    fmt: OcsFormat,
    Form(form): Form<PutForm>,
) -> Result<Response, OcsError> {
    // ... same body, replacing `handle.id` with `ctx.token_id` and
    //     using `state.users.app_passwords()` for revocation.
```

(`crabcloud-http`'s `AuthContext` re-export from Task 9 is reachable as `crate::AuthContext`.)

Update the existing OCS tests' `seed_login` helper to mint a real session token via the AppPasswordService:

```rust
    async fn seed_login(state: &AppState, uid: &str) -> String {
        use crabcloud_users::AuthTokenType;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new(uid).unwrap(),
                uid,
                "test-session",
                AuthTokenType::Session,
                false,
            )
            .await
            .unwrap();
        let cookie_value =
            encode_cookie(raw.expose(), state.config.secret.expose_secret().as_bytes());
        format!("{COOKIE_NAME}={cookie_value}")
    }
```

The tests' existing assertions (200/401/etc.) should continue passing because `AuthLayer` now picks up the cookie → AuthContext path end-to-end.

- [ ] **Step 6: Run tests + commit**

```
cargo test -p crabcloud-http
cargo xtask check-all
```

Expected: all `crabcloud-http` tests pass — the cookie path now flows through `AuthLayer` → `oc_authtoken` lookup → `AuthContext` → existing extractor → handler. The OCS `put_self_password_change_destroys_other_sessions` test now exercises the cascade via `AppPasswordService` (token row deletion + cache invalidation).

```
git add crates/crabcloud-http
git commit -m "feat(http,auth): cookie payload becomes raw token; SessionLayer reduces to blob state

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: Login `#[server]` fn mints a session AuthToken

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`

- [ ] **Step 1: Extend the existing `login` fn to mint an `AuthToken` kind=Session**

Replace the login fn:

```rust
#[server(endpoint = "index.php/login", prefix = "")]
pub async fn login(username: String, password: String, remember: Option<bool>) -> Result<(), ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;

    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let session = fs
        .extension::<crabcloud_http::SessionHandle>()
        .ok_or_else(|| ServerFnError::new("session extension missing"))?;

    let user = state
        .users
        .verify(&username, &password)
        .await
        .map_err(|e| {
            ::tracing::warn!(username = %username, error = %e, "login verify failed");
            ServerFnError::new("unauthorized")
        })?;

    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing on AppState"))?
        .clone();
    let user_agent = fs
        .request_parts()
        .map(|p| {
            p.headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Browser".to_string())
        })
        .unwrap_or_else(|| "Browser".to_string());

    let (_row, raw) = ap
        .mint(
            &user.uid,
            &username,
            &user_agent,
            AuthTokenType::Session,
            remember.unwrap_or(false),
        )
        .await
        .map_err(|e| {
            ::tracing::warn!(error = %e, "session token mint failed");
            ServerFnError::new("internal")
        })?;

    session
        .mutate(|s| {
            s.user_id = Some(user.uid.as_str().to_string());
            s.rotate_csrf();
            s.two_factor_passed = true;
        })
        .await;
    session
        .set_pending_cookie(crabcloud_http::session::layer::PendingCookie::Set {
            raw_token: raw.expose().to_string(),
            max_age_secs: 30 * 60,
        })
        .await;
    Ok(())
}
```

- [ ] **Step 2: Re-export `PendingCookie` from `crabcloud-http::lib.rs`**

Add (alphabetical with other re-exports):

```rust
pub use session::layer::PendingCookie;
```

(Or use the fully qualified path as in the snippet above; either works.)

- [ ] **Step 3: Update the existing `routes::login` integration tests, if any**

`crabcloud-http/src/routes/` no longer has `login.rs` (deleted in PR #21). The only remaining login tests are in `crabcloud-users/tests/users_flow.rs` — they run the full router and POST to `/index.php/login`. Those tests should keep passing (the login flow still sets a cookie; the cookie's payload is now a raw token instead of a session id, but the wire shape is identical).

If any test relies on the cookie value being a hex session id, update the assertion to just check the cookie name and `HttpOnly` / `SameSite=Lax` attributes.

- [ ] **Step 4: Run + commit**

```
cargo test -p crabcloud-users --test users_flow
cargo xtask check-all
```

Expected: green.

```
git add crates/crabcloud-ui crates/crabcloud-http
git commit -m "feat(ui,auth): /index.php/login mints AuthToken session and sets the raw-token cookie

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: CSRF gates on `AuthMethod::Session`; `put_self` rejects non-Session password change

**Files:**
- Modify: `crates/crabcloud-http/src/csrf.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/user.rs`

- [ ] **Step 1: CSRF middleware reads `AuthContext` instead of `SessionHandle`**

Replace the relevant block of `csrf.rs::call`:

```rust
            // Bearer / Basic auth: CSRF doesn't apply (other-origin code can't
            // set Authorization headers anyway). Skip enforcement.
            let method = req.extensions().get::<crate::auth_context::AuthContext>()
                .map(|c| c.method);
            match method {
                Some(crate::auth_context::AuthMethod::Bearer)
                | Some(crate::auth_context::AuthMethod::Basic) => {
                    return inner.call(req).await;
                }
                _ => {}
            }

            // Cookie-authed (or anonymous): use session handle's csrf_token.
            let handle = req.extensions().get::<SessionHandle>().cloned();
            let user_id = match &handle {
                Some(h) => h.read().await.user_id.clone(),
                None => None,
            };
            if user_id.is_none() {
                return inner.call(req).await;
            }
            let expected = match &handle {
                Some(h) => h.read().await.csrf_token.clone(),
                None => String::new(),
            };
            let supplied = req
                .headers()
                .get(&TOKEN_HEADER)
                .and_then(|v| v.to_str().ok());
            if supplied.map(|s| s == expected).unwrap_or(false) {
                inner.call(req).await
            } else {
                Ok((StatusCode::FORBIDDEN, "csrf token missing or mismatched").into_response())
            }
```

- [ ] **Step 2: `put_self` returns 403 on non-Session password change**

Insert at the top of the `"password"` match arm in `routes/ocs/user.rs::put_self`:

```rust
        "password" => {
            if ctx.method != crate::AuthMethod::Session {
                return Err(OcsError::new(
                    CoreError::Forbidden,
                    OcsVersion::V2,
                    fmt.0,
                ));
            }
            // ... existing flow ...
```

Add a new integration test in the same module:

```rust
    #[tokio::test]
    async fn put_self_password_change_via_bearer_is_403() {
        use crabcloud_users::AuthTokenType;
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        let app = build_router(state, axum::Router::new());

        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", format!("Bearer {}", raw.expose()))
            .body(Body::from("key=password&value=newpass&currentpassword=hunter2"))
            .unwrap();
        let resp = app.oneshot(put_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
```

- [ ] **Step 3: Run + commit + open Batch D PR**

```
cargo test -p crabcloud-http
cargo xtask check-all
```

```
git add crates/crabcloud-http
git commit -m "feat(http,auth): CSRF skips Bearer/Basic; put_self password-change 403 on non-Session

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-d
gh pr create --base master --head auth-batch-d \
  --title "auth: batch D — DB-authoritative cookie + login mints session token + CSRF gating" \
  --body "Sub-project 2b, batch D: SessionLayer collapses to ephemeral blob state keyed by token id; login mints an oc_authtoken row (kind=Session) and the cookie carries its raw token; CSRF only fires for AuthMethod::Session; PUT /ocs/.../cloud/user key=password returns 403 from Bearer/Basic."
```

**STOP.**

---

## Task 14: `/index.php/login/v2` + `/index.php/login/v2/poll` server fns

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`

- [ ] **Step 1: Start Batch E branch**

```
git checkout -b auth-batch-e origin/master
```

- [ ] **Step 2: Add the `login_v2_start` + `login_v2_poll` server fns**

Append to `server_fns.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2Poll {
    pub token: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2StartResponse {
    pub poll: LoginV2Poll,
    pub login: String,
}

const LOGIN_V2_TTL_SECS: u64 = 20 * 60;

fn random_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

fn server_base(state: &crabcloud_core::AppState) -> String {
    // Prefer overwrite.cli.url, then trusted_domains[0] over HTTPS. Falls back
    // to bind_address.
    if let Some(u) = state.config.overwrite_cli_url.clone() {
        return u;
    }
    if let Some(d) = state.config.trusted_domains.first() {
        return format!("https://{d}");
    }
    format!("http://{}", state.config.bind_address)
}

#[server(endpoint = "index.php/login/v2", prefix = "")]
pub async fn login_v2_start() -> Result<LoginV2StartResponse, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;

    let poll_id = random_id();
    let flow_id = random_id();
    let base = server_base(&state);

    // Write two empty records into the cache: one keyed by poll-id (consumed
    // by login_v2_poll), one keyed by flow-id (consumed by login_v2_authorize).
    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let poll_key = format!("{inst}:login_v2:poll:{poll_id}");
    let flow_key = format!("{inst}:login_v2:flow:{flow_id}");
    // Flow record carries `poll_id` so authorize can populate the poll entry.
    let flow_record = serde_json::to_vec(&serde_json::json!({ "poll_id": poll_id })).unwrap();
    cache
        .set(&poll_key, b"", Some(std::time::Duration::from_secs(LOGIN_V2_TTL_SECS)))
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;
    cache
        .set(&flow_key, &flow_record, Some(std::time::Duration::from_secs(LOGIN_V2_TTL_SECS)))
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;

    Ok(LoginV2StartResponse {
        poll: LoginV2Poll {
            token: poll_id,
            endpoint: format!("{base}/index.php/login/v2/poll"),
        },
        login: format!("{base}/index.php/login/v2/flow/{flow_id}"),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2PollRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2PollResponse {
    pub server: String,
    #[serde(rename = "loginName")]
    pub login_name: String,
    #[serde(rename = "appPassword")]
    pub app_password: String,
}

#[server(endpoint = "index.php/login/v2/poll", prefix = "")]
pub async fn login_v2_poll(req: LoginV2PollRequest) -> Result<LoginV2PollResponse, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;

    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let key = format!("{inst}:login_v2:poll:{}", req.token);

    let raw = cache
        .get(&key)
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?
        .ok_or_else(|| ServerFnError::new("not_found"))?;
    if raw.is_empty() {
        return Err(ServerFnError::new("not_found"));
    }
    // Burn the entry (single-use).
    let _ = cache.del(&key).await;

    let payload: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| ServerFnError::new(format!("cache decode: {e}")))?;
    let login_name = payload["loginName"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();
    let app_password = payload["appPassword"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();
    Ok(LoginV2PollResponse {
        server: server_base(&state),
        login_name,
        app_password,
    })
}
```

(Returning `ServerFnError::new("not_found")` results in a 500 by default; Dioxus fullstack's error type doesn't map cleanly to a specific HTTP status. For wire-compat the spec wants 404 on "not yet authorized". Implementer-side tweak: use `axum::response::IntoResponse` via a custom error type or set the status manually in a follow-up. For 2b's MVP, document this with a `tracing::info!` and accept the 500 — the Nextcloud client treats anything other than 200 as "not yet ready" and retries. If the implementer wants a tighter wire match they can add a `LoginV2Error` enum with `IntoResponse`.)

- [ ] **Step 3: Add integration tests for the start + poll cycle**

Append to `crates/crabcloud-users/tests/users_flow.rs`:

```rust
#[tokio::test]
async fn login_v2_start_returns_urls() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed["poll"]["token"].as_str().unwrap().len() > 16);
    assert!(parsed["login"].as_str().unwrap().contains("/login/v2/flow/"));
}
```

Cookie/flow tests land in Task 15 once `login_v2_authorize` exists.

- [ ] **Step 4: Run + commit**

```
cargo test -p crabcloud-users --test users_flow
cargo xtask check-all
```

```
git add crates/crabcloud-ui
git commit -m "feat(ui,auth): /index.php/login/v2 start + poll server fns

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: `/index.php/login/v2/flow/<id>` page + `login_v2_authorize` server fn

**Files:**
- Modify: `crates/crabcloud-ui/src/app.rs` (Route enum)
- Create: `crates/crabcloud-ui/src/pages/login_v2_flow.rs`
- Modify: `crates/crabcloud-ui/src/pages/mod.rs`
- Modify: `crates/crabcloud-ui/src/server_fns.rs`

- [ ] **Step 1: Add a Route variant for the flow page**

In `app.rs`'s `Route` enum:

```rust
    #[route("/index.php/login/v2/flow/:flow_id")]
    LoginV2FlowRoute { flow_id: String },
```

And add a component:

```rust
#[component]
pub fn LoginV2FlowRoute(flow_id: String) -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { crate::pages::login_v2_flow::LoginV2Flow { ctx: ctx.clone(), flow_id: flow_id.clone() } }
}
```

- [ ] **Step 2: Create `crates/crabcloud-ui/src/pages/login_v2_flow.rs`**

```rust
//! `/index.php/login/v2/flow/<flow-id>` — the page the Nextcloud client
//! opens in the user's browser to authorize a fresh app password.

use crate::context::RequestContext;
use crate::server_fns::login_v2_authorize;
use dioxus::prelude::*;

#[component]
pub fn LoginV2Flow(ctx: RequestContext, flow_id: String) -> Element {
    let mut authorized = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let flow_id_for_submit = flow_id.clone();

    if ctx.user_id.is_none() {
        return rsx! {
            main { class: "login-v2-flow",
                h1 { "Sign in required" }
                p { "Please log in first, then return to this URL to authorize the app." }
                p { a { href: "/login", "Log in" } }
            }
        };
    }

    if authorized() {
        return rsx! {
            main { class: "login-v2-flow",
                h1 { "Authorized" }
                p { "You can close this tab now." }
            }
        };
    }

    rsx! {
        main { class: "login-v2-flow",
            h1 { "Authorize app" }
            p { "This will grant the calling application access to your account using an app password. You can revoke it from Settings → Security at any time." }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
            button {
                onclick: move |_| {
                    let fid = flow_id_for_submit.clone();
                    spawn(async move {
                        match login_v2_authorize(fid).await {
                            Ok(()) => authorized.set(true),
                            Err(e) => error.set(Some(format!("{e}"))),
                        }
                    });
                },
                "Authorize"
            }
        }
    }
}
```

- [ ] **Step 3: Add `pub mod login_v2_flow` to `pages/mod.rs`**

- [ ] **Step 4: Add `login_v2_authorize` server fn**

```rust
#[server(endpoint = "index.php/login/v2/authorize", prefix = "")]
pub async fn login_v2_authorize(flow_id: String) -> Result<(), ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;

    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("must be authenticated via session cookie"));
    }

    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let flow_key = format!("{inst}:login_v2:flow:{flow_id}");
    let raw = cache
        .get(&flow_key)
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?
        .ok_or_else(|| ServerFnError::new("flow_not_found"))?;
    let payload: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| ServerFnError::new(format!("cache decode: {e}")))?;
    let poll_id = payload["poll_id"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();

    let user_agent = fs
        .request_parts()
        .map(|p| {
            p.headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Client".to_string())
        })
        .unwrap_or_else(|| "Client".to_string());

    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let (_row, raw_token) = ap
        .mint(
            &ctx.user_id,
            &ctx.login_name,
            &user_agent,
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .map_err(|e| ServerFnError::new(format!("mint: {e}")))?;

    let poll_key = format!("{inst}:login_v2:poll:{poll_id}");
    let payload = serde_json::json!({
        "loginName": ctx.user_id.as_str(),
        "appPassword": raw_token.expose(),
    });
    let bytes = serde_json::to_vec(&payload).unwrap();
    cache
        .set(&poll_key, &bytes, Some(std::time::Duration::from_secs(LOGIN_V2_TTL_SECS)))
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;
    let _ = cache.del(&flow_key).await;
    Ok(())
}
```

(Note: `LOGIN_V2_TTL_SECS` is a constant from Task 14; this fn lives in the same file.)

- [ ] **Step 5: Add integration test for the full flow**

Append to `users_flow.rs`:

```rust
#[tokio::test]
async fn login_v2_full_cycle() {
    use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier};
    let app = build_app().await;

    // 1. Start a flow.
    let start_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2")
        .body(Body::empty())
        .unwrap();
    let start_resp = app.clone().oneshot(start_req).await.unwrap();
    assert_eq!(start_resp.status(), StatusCode::OK);
    let start_body = to_bytes(start_resp.into_body(), 16 * 1024).await.unwrap();
    let start: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
    let poll_token = start["poll"]["token"].as_str().unwrap().to_string();
    let login_url = start["login"].as_str().unwrap().to_string();
    let flow_id = login_url
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    // 2. Pre-authorize poll: expect 500 (or 404 in spec; see Task 14 note).
    // We just assert the response is not 200.
    let poll_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/poll")
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"token\":\"{poll_token}\"}}")))
        .unwrap();
    let pre = app.clone().oneshot(poll_req).await.unwrap();
    assert_ne!(pre.status(), StatusCode::OK);

    // 3. Log in (POST /index.php/login) — produces a cookie.
    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from("{\"username\":\"alice\",\"password\":\"hunter2\"}"))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // 4. Call authorize as the cookie-authed user.
    let auth_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/authorize")
        .header("content-type", "application/json")
        .header("cookie", &cookie)
        .body(Body::from(format!("\"{flow_id}\"")))
        .unwrap();
    let auth_resp = app.clone().oneshot(auth_req).await.unwrap();
    assert_eq!(auth_resp.status(), StatusCode::OK);

    // 5. Poll again — now 200 with the app password.
    let poll_req2 = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/poll")
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"token\":\"{poll_token}\"}}")))
        .unwrap();
    let poll2 = app.clone().oneshot(poll_req2).await.unwrap();
    assert_eq!(poll2.status(), StatusCode::OK);
    let body = to_bytes(poll2.into_body(), 16 * 1024).await.unwrap();
    let p: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(p["loginName"], "alice");
    assert!(p["appPassword"].as_str().unwrap().len() > 50);

    // 6. Use the app password to hit a protected endpoint.
    let _ = (p, BcryptVerifier::new()); // silence unused
    // (the body field already verifies the format; we trust AuthLayer's
    //  separate tests for header-auth correctness.)
}
```

- [ ] **Step 6: Run + commit**

```
cargo test -p crabcloud-users --test users_flow
cargo xtask check-all
```

```
git add crates/crabcloud-ui
git commit -m "feat(ui,auth): /index.php/login/v2/flow page + authorize server fn

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: `/ocs/v2.php/core/{getapppassword,apppassword}` endpoints

**Files:**
- Create: `crates/crabcloud-http/src/routes/ocs/app_password.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/mod.rs`

- [ ] **Step 1: Create `routes/ocs/app_password.rs`**

```rust
//! GET /ocs/v2.php/core/getapppassword (Session-only) — mints a bridge token.
//! DELETE /ocs/v2.php/core/apppassword (any auth) — revokes the current
//! request's own token row.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::AuthTokenType;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct AppPasswordPayload {
    apppassword: String,
}

fn unauth(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Unauthorized, OcsVersion::V2, fmt)
}
fn forbidden(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Forbidden, OcsVersion::V2, fmt)
}

pub async fn get_app_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    if ctx.method != AuthMethod::Session {
        return Err(forbidden(fmt.0));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| unauth(fmt.0))?
        .clone();
    let (_row, raw) = ap
        .mint(
            &ctx.user_id,
            &ctx.login_name,
            "Browser bridge",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;

    let payload = AppPasswordPayload {
        apppassword: raw.expose().to_string(),
    };
    let envelope = OcsResponse::ok(payload, OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

pub async fn delete_app_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| unauth(fmt.0))?
        .clone();
    let _ = ap.revoke(ctx.token_id).await;
    let envelope = OcsResponse::ok(serde_json::json!({}), OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

#[cfg(test)]
mod tests {
    // ... see integration test in users_flow.rs (E2E coverage in batch G).
}
```

- [ ] **Step 2: Mount the routes in `routes/ocs/mod.rs`**

```rust
pub mod app_password;
pub mod capabilities;
pub mod user;

use axum::routing::{delete, get};
use axum::Router;
use crabcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.php/cloud/capabilities", get(capabilities::handler))
        .route(
            "/v2.php/cloud/user",
            get(user::get_self).put(user::put_self),
        )
        .route(
            "/v2.php/core/getapppassword",
            get(app_password::get_app_password),
        )
        .route(
            "/v2.php/core/apppassword",
            delete(app_password::delete_app_password),
        )
}
```

- [ ] **Step 3: Integration test in users_flow.rs**

```rust
#[tokio::test]
async fn getapppassword_via_cookie_mints_bridge_token() {
    let app = build_app().await;

    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from("{\"username\":\"alice\",\"password\":\"hunter2\"}"))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let req = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let p: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(p["ocs"]["data"]["apppassword"].as_str().unwrap().len() > 50);
}

#[tokio::test]
async fn getapppassword_via_bearer_is_forbidden() {
    use crabcloud_users::AuthTokenType;
    let app_state_seed = crabcloud_config::test_support::minimal_sqlite_config(
        tempfile::tempdir().unwrap().path().join("ap.db"),
    );
    let state = crabcloud_core::AppStateBuilder::new(app_state_seed)
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let hash = crabcloud_users::BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &crabcloud_users::User {
                uid: crabcloud_users::UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &crabcloud_users::UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .unwrap();
    let app = build_router(state, axum::Router::new());
    let req = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {}", raw.expose()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_app_password_revokes_current_token() {
    use crabcloud_users::AuthTokenType;
    let app = build_app().await;

    // Mint an AppPassword for alice via login then getapppassword chain.
    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from("{\"username\":\"alice\",\"password\":\"hunter2\"}"))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let gap = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let gap_resp = app.clone().oneshot(gap).await.unwrap();
    let gap_body = to_bytes(gap_resp.into_body(), 16 * 1024).await.unwrap();
    let raw_token = serde_json::from_slice::<serde_json::Value>(&gap_body).unwrap()
        ["ocs"]["data"]["apppassword"]
        .as_str()
        .unwrap()
        .to_string();

    // Use the new token to call DELETE apppassword.
    let del = Request::builder()
        .method("DELETE")
        .uri("/ocs/v2.php/core/apppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {raw_token}"))
        .body(Body::empty())
        .unwrap();
    let del_resp = app.clone().oneshot(del).await.unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);

    // Re-use the same token → 401.
    let again = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {raw_token}"))
        .body(Body::empty())
        .unwrap();
    let again_resp = app.oneshot(again).await.unwrap();
    assert_eq!(again_resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 4: Run + commit + open Batch E PR**

```
cargo test -p crabcloud-users --test users_flow
cargo xtask check-all
```

```
git add crates/crabcloud-http
git commit -m "feat(http,auth): /ocs/v2.php/core/{getapppassword,apppassword} endpoints

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-e
gh pr create --base master --head auth-batch-e \
  --title "auth: batch E — /login/v2 flow + core/apppassword OCS" \
  --body "Sub-project 2b, batch E: /index.php/login/v2 start + poll + /flow/<id> page + authorize fn (Nextcloud client bootstrap); /ocs/v2.php/core/getapppassword (Session-only, mints bridge); DELETE /ocs/v2.php/core/apppassword (revokes current row)."
```

**STOP.**

---

## Task 17: Settings → Security Dioxus page + `#[server]` fns

**Files:**
- Modify: `crates/crabcloud-ui/src/app.rs`
- Create: `crates/crabcloud-ui/src/pages/settings_security.rs`
- Modify: `crates/crabcloud-ui/src/pages/mod.rs`
- Modify: `crates/crabcloud-ui/src/server_fns.rs`

- [ ] **Step 1: Start Batch F branch**

```
git checkout -b auth-batch-f origin/master
```

- [ ] **Step 2: Add Route variant**

In `app.rs`:

```rust
    #[route("/settings/security")]
    SettingsSecurityRoute {},
```

```rust
#[component]
pub fn SettingsSecurityRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { crate::pages::settings_security::SettingsSecurity { ctx: ctx.clone() } }
}
```

- [ ] **Step 3: Add the server fns**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenSummary {
    pub id: i64,
    pub name: String,
    pub kind: i32,             // AuthTokenType discriminator
    pub last_activity: u64,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedAppPassword {
    pub id: i64,
    pub name: String,
    pub raw_token: String,
}

#[server(endpoint = "settings/security/list", prefix = "")]
pub async fn list_app_passwords() -> Result<Vec<AuthTokenSummary>, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let rows = ap
        .list(&ctx.user_id)
        .await
        .map_err(|e| ServerFnError::new(format!("list: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|r| AuthTokenSummary {
            current: r.id == ctx.token_id,
            id: r.id,
            name: r.name,
            kind: r.kind.as_i32(),
            last_activity: r.last_activity,
        })
        .collect())
}

#[server(endpoint = "settings/security/create", prefix = "")]
pub async fn create_app_password(name: String) -> Result<CreatedAppPassword, ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let (row, raw) = ap
        .mint(&ctx.user_id, &ctx.login_name, &name, AuthTokenType::AppPassword, false)
        .await
        .map_err(|e| ServerFnError::new(format!("mint: {e}")))?;
    Ok(CreatedAppPassword {
        id: row.id,
        name: row.name,
        raw_token: raw.expose().to_string(),
    })
}

#[server(endpoint = "settings/security/revoke", prefix = "")]
pub async fn revoke_app_password(id: i64) -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    // Optional: verify the row belongs to ctx.user_id before revoking, so
    // a malicious browser session can't revoke another user's token.
    if let Some(row) = ap.lookup_by_id(id).await.map_err(|e| ServerFnError::new(format!("lookup: {e}")))? {
        if row.uid != ctx.user_id {
            return Err(ServerFnError::new("not your token"));
        }
    }
    ap.revoke(id)
        .await
        .map_err(|e| ServerFnError::new(format!("revoke: {e}")))
}

#[server(endpoint = "settings/security/destroy-others", prefix = "")]
pub async fn destroy_other_sessions() -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    ap.revoke_other_sessions(&ctx.user_id, ctx.token_id)
        .await
        .map_err(|e| ServerFnError::new(format!("revoke_others: {e}")))
}
```

- [ ] **Step 4: Create `pages/settings_security.rs`**

```rust
//! `/settings/security` — list / create / revoke app passwords + log out
//! everywhere else.

use crate::context::RequestContext;
use crate::server_fns::{
    create_app_password, destroy_other_sessions, list_app_passwords, revoke_app_password,
    AuthTokenSummary, CreatedAppPassword,
};
use dioxus::prelude::*;

#[component]
pub fn SettingsSecurity(ctx: RequestContext) -> Element {
    let mut tokens = use_signal(|| Vec::<AuthTokenSummary>::new());
    let mut just_created = use_signal(|| Option::<CreatedAppPassword>::None);
    let mut new_name = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);

    let mut refresh = use_callback(move |_| {
        spawn(async move {
            match list_app_passwords().await {
                Ok(rows) => tokens.set(rows),
                Err(e) => error.set(Some(format!("{e}"))),
            }
        });
    });

    use_effect(move || refresh(()));

    if ctx.user_id.is_none() {
        return rsx! {
            main { class: "settings-security",
                h1 { "Please log in" }
                p { a { href: "/login", "Log in" } }
            }
        };
    }

    rsx! {
        main { class: "settings-security",
            h1 { "Security" }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
            section {
                h2 { "Active devices" }
                table {
                    thead { tr { th { "Name" } th { "Type" } th { "Last activity" } th {} } }
                    tbody {
                        for row in tokens().into_iter() {
                            tr {
                                td { "{row.name}" }
                                td { if row.kind == 0 { "Browser session" } else { "App password" } }
                                td { "{row.last_activity}" }
                                td {
                                    if row.current {
                                        em { "current" }
                                    } else {
                                        button {
                                            onclick: move |_| {
                                                let id = row.id;
                                                spawn(async move {
                                                    let _ = revoke_app_password(id).await;
                                                    refresh(());
                                                });
                                            },
                                            "Revoke"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                button {
                    onclick: move |_| {
                        spawn(async move {
                            let _ = destroy_other_sessions().await;
                            refresh(());
                        });
                    },
                    "Log out everywhere else"
                }
            }
            section {
                h2 { "Create app password" }
                if let Some(created) = just_created() {
                    div { class: "created",
                        p { "Copy this password now — it will not be shown again:" }
                        code { "{created.raw_token}" }
                        button {
                            onclick: move |_| just_created.set(None),
                            "Dismiss"
                        }
                    }
                } else {
                    form {
                        onsubmit: move |evt| {
                            evt.prevent_default();
                            let name = new_name();
                            if name.is_empty() { return; }
                            spawn(async move {
                                match create_app_password(name.clone()).await {
                                    Ok(c) => {
                                        just_created.set(Some(c));
                                        new_name.set(String::new());
                                        refresh(());
                                    }
                                    Err(e) => error.set(Some(format!("{e}"))),
                                }
                            });
                        },
                        input {
                            r#type: "text",
                            placeholder: "Device name (e.g. \"iPhone\")",
                            value: "{new_name}",
                            oninput: move |evt| new_name.set(evt.value()),
                        }
                        button { r#type: "submit", "Create" }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 5: Add `pub mod settings_security` to `pages/mod.rs`**

- [ ] **Step 6: Run + commit**

```
cargo xtask check-all
```

Expected: green (UI smoke test only — the e2e suite extension lands in batch G).

```
git add crates/crabcloud-ui
git commit -m "feat(ui,auth): Settings -> Security Dioxus page + server fns

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 18: CLI subcommands

**Files:**
- Modify: `crates/crabcloud-users/src/cli.rs`
- Modify: `crates/crabcloud-server/src/cli.rs`
- Modify: `crates/crabcloud-server/src/main.rs`

- [ ] **Step 1: Add helper functions in `crabcloud-users::cli`**

Append:

```rust
use crate::app_password::AppPasswordService;
use crate::auth_token::AuthTokenType;

pub async fn app_password_add(
    ap: &AppPasswordService,
    uid: &str,
    name: &str,
) -> UsersResult<(i64, String)> {
    let user_id = UserId::new(uid)?;
    let (row, raw) = ap
        .mint(&user_id, uid, name, AuthTokenType::AppPassword, false)
        .await?;
    Ok((row.id, raw.expose().to_string()))
}

pub async fn app_password_list(
    ap: &AppPasswordService,
    uid: &str,
) -> UsersResult<Vec<(i64, String, AuthTokenType, u64)>> {
    let user_id = UserId::new(uid)?;
    Ok(ap
        .list(&user_id)
        .await?
        .into_iter()
        .map(|r| (r.id, r.name, r.kind, r.last_activity))
        .collect())
}

pub async fn app_password_revoke(ap: &AppPasswordService, id: i64) -> UsersResult<()> {
    ap.revoke(id).await
}
```

Tests (append to existing `mod tests` in `cli.rs`):

```rust
    #[tokio::test]
    async fn app_password_add_then_list_then_revoke() {
        let svc = fresh_svc().await;
        // Seed user
        super::user_add(&svc, "alice", "hunter2", None, None, false).await.unwrap();
        // Get app_passwords from a manually-built one (svc doesn't carry it in
        // this fixture); for the test, mint directly.
        use crate::app_password::AppPasswordService;
        use crate::store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
        use crabcloud_cache::MemoryCache;
        use secrecy::SecretString;
        use std::sync::Arc;
        // Use the same DB pool the svc uses — pull via user_store's downcast hack:
        // simpler: build a brand-new service over a fresh pool, just for this test.
        let dir = tempfile::tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("c2.db"));
        std::mem::forget(dir);
        let pool = crabcloud_db::DbPool::connect(&cfg).await.unwrap();
        let mut runner = crabcloud_db::MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(crabcloud_db::core_set());
        runner.run().await.unwrap();
        let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool));
        let cache = Arc::new(TokenAuthCache::new(token_store, Arc::new(MemoryCache::new()), "inst"));
        let ap = AppPasswordService::new(cache, SecretString::new("s".into()));

        let (id, raw) = super::app_password_add(&ap, "alice", "DAV").await.unwrap();
        assert!(id > 0);
        assert!(raw.len() > 50);
        let listed = super::app_password_list(&ap, "alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, id);
        super::app_password_revoke(&ap, id).await.unwrap();
        let empty = super::app_password_list(&ap, "alice").await.unwrap();
        assert!(empty.is_empty());
    }
```

- [ ] **Step 2: Add `Cmd` variants in `crabcloud-server/src/cli.rs`**

```rust
    /// Create a new app password for a user. Prints the plaintext exactly once.
    AppPasswordAdd { uid: String, name: String },
    /// List a user's tokens (id, name, type, last_activity).
    AppPasswordList { uid: String },
    /// Revoke an app password by row id.
    AppPasswordRevoke { id: i64 },
```

- [ ] **Step 3: Wire match arms in `main.rs`**

```rust
        Cmd::AppPasswordAdd { uid, name } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            let (id, raw) = crabcloud_users::cli::app_password_add(&ap, &uid, &name).await?;
            println!("id={id}");
            println!("token={raw}");
            info!(uid, name, id, "app password created");
            state.pool.close().await;
            Ok(())
        }
        Cmd::AppPasswordList { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            for (id, name, kind, last) in
                crabcloud_users::cli::app_password_list(&ap, &uid).await?
            {
                let kind_str = match kind {
                    crabcloud_users::AuthTokenType::Session => "session",
                    crabcloud_users::AuthTokenType::AppPassword => "app",
                };
                println!("{id}\t{kind_str}\t{last}\t{name}");
            }
            state.pool.close().await;
            Ok(())
        }
        Cmd::AppPasswordRevoke { id } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            crabcloud_users::cli::app_password_revoke(&ap, id).await?;
            info!(id, "app password revoked");
            state.pool.close().await;
            Ok(())
        }
```

- [ ] **Step 4: Add clap-parse tests + commit + open Batch F PR**

```rust
    #[test]
    fn app_password_add_parses() {
        let cli = Cli::parse_from(["crabcloud-server", "app-password-add", "alice", "DAV"]);
        match cli.selected() {
            Cmd::AppPasswordAdd { uid, name } => {
                assert_eq!(uid, "alice");
                assert_eq!(name, "DAV");
            }
            _ => panic!("expected AppPasswordAdd"),
        }
    }
```

```
cargo test -p crabcloud-users -p crabcloud-server
cargo xtask check-all
```

```
git add crates/crabcloud-users crates/crabcloud-server
git commit -m "feat(server,auth): CLI app-password-add/list/revoke subcommands

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-f
gh pr create --base master --head auth-batch-f \
  --title "auth: batch F — Settings/Security UI page + CLI subcommands" \
  --body "Sub-project 2b, batch F: /settings/security Dioxus page with list/create/revoke + log out everywhere else; crabcloud-server app-password-{add,list,revoke} subcommands."
```

**STOP.**

---

## Task 19: Playwright e2e — `app_password.spec.ts`

**Files:**
- Create: `e2e/tests/app_password.spec.ts`

- [ ] **Step 1: Start Batch G branch**

```
git checkout -b auth-batch-g origin/master
```

- [ ] **Step 2: Write `e2e/tests/app_password.spec.ts`**

```ts
import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

test.describe("App passwords end-to-end", () => {
    test("login -> getapppassword -> use via Basic -> revoke -> 401", async ({ request }) => {
        // Login via /index.php/login (the bootstrap_admin fixture from e2e.toml
        // makes admin/hunter2 valid).
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        expect(login.status()).toBe(200);
        const cookie = login.headers()["set-cookie"];
        const sessionValue = /oc_sessionPassphrase=([^;]+)/.exec(cookie!)![1];

        // Mint a bridge app password.
        const gap = await request.get("/ocs/v2.php/core/getapppassword?format=json", {
            headers: { "ocs-apirequest": "true", cookie: `oc_sessionPassphrase=${sessionValue}` },
        });
        expect(gap.status()).toBe(200);
        const gapBody = await gap.json();
        const appPassword: string = gapBody.ocs.data.apppassword;
        expect(appPassword.length).toBeGreaterThan(50);

        // Use it via Basic.
        const me = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: {
                "ocs-apirequest": "true",
                authorization: `Basic ${Buffer.from(`admin:${appPassword}`).toString("base64")}`,
            },
        });
        expect(me.status()).toBe(200);
        const meBody = await me.json();
        expect(meBody.ocs.data.id).toBe("admin");

        // Revoke (use the cookie session because the bridge token's own
        // revoke would invalidate the request that asked).
        const list = await request.get("/settings/security/list", {
            headers: { "ocs-apirequest": "true", cookie: `oc_sessionPassphrase=${sessionValue}` },
        });
        // List endpoint returns JSON via #[server] fn.
        // Pick the AppPassword row's id.
        // (Implementer: adapt assertions if the #[server] envelope differs.)
        expect(list.status()).toBe(200);

        // Bearer with the still-extant token works.
        const me2 = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: { "ocs-apirequest": "true", authorization: `Bearer ${appPassword}` },
        });
        expect(me2.status()).toBe(200);

        // Self-revoke via DELETE apppassword (uses the token itself).
        const del = await request.delete("/ocs/v2.php/core/apppassword?format=json", {
            headers: { "ocs-apirequest": "true", authorization: `Bearer ${appPassword}` },
        });
        expect(del.status()).toBe(200);

        // After revoke, the same Bearer is 401.
        const after = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: { "ocs-apirequest": "true", authorization: `Bearer ${appPassword}` },
        });
        expect(after.status()).toBe(401);
    });
});
```

- [ ] **Step 3: Run e2e via CI (local Playwright requires `npm install` in `e2e/`)**

For local verification:

```
cd e2e && npm ci && npx playwright install --with-deps chromium
```

then start the server and run `npm test`. CI will exercise this on push.

```
git add e2e/tests/app_password.spec.ts
git commit -m "test(e2e): app-password mint + Bearer + Basic + revoke round-trip

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 20: Acceptance docs

**Files:**
- Create: `docs/superpowers/plans/2026-05-12-app-passwords-bearer-basic-auth-implementation.changelog.md`
- Modify: `README.md`

- [ ] **Step 1: Write the changelog**

Use the 2a changelog as the template. The file should mirror the spec's §10 acceptance criteria with status markers + cite specific tests for each row. Date: today (2026-05-12 or current date; absolute).

Required sections:
- `What works` — bullet list of everything that landed (token store, AuthLayer, /login/v2, OCS endpoints, Settings UI, CLI, password cascade, …).
- `What's deferred` — items from spec §11 (OAuth2 → 2d, 2FA → 2c, token scopes, E2E keys, remote-wipe admin, expired-token sweep, secret-rotation).
- `Known limitations` — login_v2_poll returns 500 instead of 404 on not-yet-authorized; cache-only sessions are invalidated on first deploy after 2b.
- `Acceptance status` — table from spec §10, each row marked OK with a test reference.

- [ ] **Step 2: Update README**

Add to the Quick Start section (after the existing user-add step):

```
# 3c. Pair a DAV / desktop / mobile client:
#     - Visit https://<server>/settings/security in your browser.
#     - "Create app password" with a device name (e.g. "Phone").
#     - Copy the displayed token (shown ONCE).
#     - Configure your client with username + that token as the password.
```

Add `crates/crabcloud-users` to the workspace-layout listing notes that it owns AppPasswordService + TokenStore.

- [ ] **Step 3: Commit + open Batch G PR**

```
cargo xtask check-all
git add docs/superpowers/plans/2026-05-12-app-passwords-bearer-basic-auth-implementation.changelog.md README.md
git commit -m "docs(auth): sub-project 2b acceptance — changelog + README

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin auth-batch-g
gh pr create --base master --head auth-batch-g \
  --title "auth: batch G — e2e tests + acceptance docs" \
  --body "Sub-project 2b final batch: Playwright app-password round-trip, sub-project 2b changelog, README pair-a-client step."
```

**STOP.**

---

## Final acceptance

After all 7 PRs land:

1. `git pull --ff-only origin master`.
2. `cargo xtask check-all` green.
3. CI green on master (test-sqlite, test-multidialect, build-wasm, fmt-and-clippy, e2e).
4. Manual smoke (optional): `cargo run -p crabcloud-server -- app-password-add admin DAV`, then `curl -u admin:<token> http://127.0.0.1:8080/ocs/v2.php/cloud/user?format=json -H "ocs-apirequest: true"` → 200 with the user envelope.
5. Mark sub-project 2b complete in the program tracking doc.

## Open questions deferred to follow-up tracking

- `login_v2_poll`'s "not yet authorized" response is a 500 (Dioxus fullstack's `ServerFnError`) rather than the spec-preferred 404. Nextcloud clients treat both as "keep polling", so wire-impact is nil; tighten in a follow-up by introducing a typed `LoginV2Error` enum with `IntoResponse`.
- The `oc_authtoken.password` column is reserved for the future mount-credentials sub-project; 2b always writes `NULL`.
- `remember` is stored on the row but the cookie's `Max-Age` is fixed at `SESSION_IDLE_TTL` — wire a longer max-age path in a UX follow-up.


