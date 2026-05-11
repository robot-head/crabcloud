# Crabcloud — Users / Core User Store Design

**Status:** Draft for review
**Date:** 2026-05-11
**Sub-project:** 2a (Core User Store) — first slice of the "users / auth / sessions" sub-program
**Parent program:** Port Nextcloud server to Rust, with a Dioxus frontend.
**Depends on:** Platform Core (sub-project 1, complete) — `AppState`, `BootstrapHook`, session machinery.

---

## 1. Program context

Crabcloud is a Rust port of [nextcloud/server](https://github.com/nextcloud/server). Platform Core (the substrate) is complete. The users space — auth, identity, group membership, app passwords, OAuth2, LDAP/SAML, 2FA — is too large for a single sub-project; it's been decomposed into:

| # | Sub-project | Scope |
|---|---|---|
| **2a** | **Core user store (this spec)** | `oc_users` + `oc_groups` + `oc_group_user` + `oc_preferences`. `UserStore` / `GroupStore` traits, SQL backend. bcrypt verification. Replace `bootstrap_admin` stand-in. Self-service password change + self-info OCS endpoints. CLI subcommands for user/group management. |
| 2b | App passwords + token auth | `oc_authtoken` table; Bearer / Basic / app-password extractors |
| 2c | 2FA framework | `twofactor_*` tables; provider trait; TOTP backend |
| 2d | OAuth2 server | `oc_oauth2_clients` + token endpoint |
| 2e | LDAP backend | Read-only `UserStore` impl backed by LDAP |
| 2f | SAML backend | SAML SP integration |

This document specs **only 2a**. Each later sub-project gets its own spec.

### Compatibility commitment

Inherited from the platform-core spec: wire + storage + DB compatible with upstream Nextcloud. For 2a this means:

- `oc_users` / `oc_groups` / `oc_group_user` / `oc_preferences` schemas mirror upstream column-for-column.
- Password hashes use bcrypt — the format Nextcloud writes by default (`$2y$...` / `$2a$...` / `$2b$...`). Legacy formats (sha1, sha256, argon2 via PHP) are NOT supported in 2a; pointing Crabcloud at a very old Nextcloud DB with non-bcrypt hashes would require force-resetting affected passwords.
- The `admin` group is canonical and seeded by the migration.
- Wire endpoints match upstream: `GET /ocs/v2.php/cloud/user` (self info), `PUT /ocs/v2.php/cloud/user` (self-service changes).
- Login URL (`POST /index.php/login`) and session cookie name (`oc_sessionPassphrase`) unchanged from Phase 3.

---

## 2. Goals

- A real user store replaces the `bootstrap_admin` config stand-in.
- `UserStore` / `GroupStore` traits abstract the storage backend so sub-projects 2e (LDAP) and 2f (SAML) can plug in without reshaping the auth path.
- bcrypt-verified login with constant-time username treatment (no enumeration oracle).
- Self-service: change own password, view own info.
- CLI tooling for admin user/group management (no admin OCS API in 2a).
- A graceful bootstrap-admin transition: existing fresh installs can still boot with just config, and the shim retires itself once a real DB user exists.

## 3. Non-goals (out of this spec)

Each item below is its own future sub-project or planned follow-up:

- **Admin OCS endpoints** (`POST` / `PUT` / `DELETE /ocs/v2.php/cloud/users`).
- **Groups OCS endpoints** (`/ocs/v2.php/cloud/groups`).
- **App passwords / Bearer / Basic auth** — sub-project 2b.
- **2FA framework** — sub-project 2c.
- **OAuth2 server** — sub-project 2d.
- **LDAP / SAML backends** — sub-projects 2e / 2f.
- **Password reset via email** — needs the mail-sending sub-project.
- **"Manage your account" UI page** — needs the settings UI sub-project.
- **Multi-backend composition** (`CompositeUserStore` stacking SQL + LDAP) — lands in 2e when there's a second backend to stack.
- **Sub-admins** (group-level admin delegation), **per-user storage quotas**, **file-system mappings** — all later.
- **Case-insensitive `uid` matching** (would need a generated `uid_lower` column).
- **Password strength policy** (Nextcloud's `password_policy` app equivalent).
- **Legacy password hash formats** (sha1 / sha256 / argon2-via-PHP).
- **Account self-deletion**.

---

## 4. Architecture

### 4.1 New crate: `crabcloud-users`

Consistent with the platform-core pattern: one crate per concern.

```
crates/
├── crabcloud-cache, -config, -core, -db, -http, -i18n, -ocs, -server, -ui   # existing
└── crabcloud-users/                                                          # NEW
    ├── Cargo.toml
    └── src/
        ├── lib.rs                     # re-exports
        ├── user.rs                    # User + UserId (newtype with validation)
        ├── group.rs                   # Group + GroupId
        ├── email.rs                   # Email (newtype with validation)
        ├── password.rs                # PasswordVerifier trait + BcryptVerifier
        ├── store/
        │   ├── mod.rs                 # UserStore + GroupStore traits
        │   ├── sql.rs                 # SqlUserStore / SqlGroupStore
        │   └── bootstrap_shim.rs      # BootstrapAdminBackend wrapper
        ├── error.rs                   # UsersError
        ├── service.rs                 # UsersService (composes store + verifier)
        └── cli.rs                     # Reusable helpers for the server-bin subcommands
```

### 4.2 Public API surface

**Traits** (all `async`, all `Send + Sync`):

- `UserStore` — `lookup(uid)`, `lookup_by_login(login)`, `set_password(uid, hash)`, `set_display_name`, `set_email`, `create(user, password_hash)`, `delete(uid)`, `touch_last_seen(uid)`, `set_enabled(uid, bool)`. Each method may return `UsersError::ReadOnly` if the backend doesn't support it.
- `GroupStore` — `lookup(gid)`, `is_in_group(uid, gid)`, `groups_of(uid)`, `members_of(gid)`, `add_to_group(uid, gid)`, `remove_from_group(uid, gid)`, `create(group)`, `delete(gid)`.
- `PasswordVerifier` — `verify(password, hash) -> bool`. Default impl: `BcryptVerifier` using the `bcrypt` crate. Sentinel-hash constant-time fake-verify when caller passes `None` for the hash.
- `PreferenceStore` — `get(uid, app, key)`, `set(uid, app, key, value)`, `delete(uid, app, key)`, `list(uid, app) -> Vec<(key, value)>`. Backed by `oc_preferences`.

**Concrete types**:

- `UserId(String)` — newtype with validating constructor. `UserId::new(s) -> Result<Self, UsersError>` checks length 1–64 and chars `[A-Za-z0-9._@-]`.
- `Email(String)` — newtype validated via the `email_address` crate. Stored lowercased + trimmed.
- `User { uid: UserId, display_name: String, email: Option<Email>, enabled: bool, last_seen: u64 }`. The password hash is NOT a field on `User` — `UserStore::lookup` returns `User` (no hash); the auth path uses an internal method that returns hash + user together to keep the hash off the wider type's surface.
- `Group { gid: GroupId, display_name: String }`.
- `UsersService` — concrete struct holding `Arc<dyn UserStore> + Arc<dyn GroupStore> + Arc<dyn PasswordVerifier> + Arc<dyn PreferenceStore>`. Lives on `AppState.users`.

### 4.3 Wiring into platform-core

**`AppState` gains four fields, exposed through one façade**:

```rust
pub struct AppState {
    pub config: Arc<FileConfig>,
    pub pool: DbPool,
    pub cache: Arc<dyn Cache>,
    pub i18n: Arc<I18n>,
    pub appconfig: AppConfigService,
    pub capability_providers: Arc<Mutex<Vec<Arc<dyn CapabilityProvider>>>>,
    pub users: UsersService,            // NEW
}
```

`UsersService` exposes typed accessors so callers don't reach into the four sub-traits directly:

```rust
impl UsersService {
    pub async fn verify(&self, login: &str, password: &str) -> Result<User, UsersError>;
    pub async fn lookup(&self, uid: &UserId) -> Result<Option<User>, UsersError>;
    pub async fn lookup_by_login(&self, login: &str) -> Result<Option<User>, UsersError>;
    pub async fn set_password(&self, uid: &UserId, new: &str) -> Result<(), UsersError>;
    pub async fn is_admin(&self, uid: &UserId) -> Result<bool, UsersError>;
    pub async fn groups_of(&self, uid: &UserId) -> Result<Vec<GroupId>, UsersError>;
    pub async fn preferences(&self) -> &Arc<dyn PreferenceStore>;
}
```

**`AppStateBuilder` changes**:

- New `with_users(service: UsersService)` builder method — lets future sub-projects inject a custom composition (e.g., LDAP + SQL stacked).
- Default `build()` constructs the SQL backend wired against the existing `DbPool`, wraps it in `BootstrapAdminBackend` if `config.bootstrap_admin` is set, and pairs it with `BcryptVerifier`.

**`config.bootstrap_admin` is deprecated but not removed**. The deprecation behavior is below in §6.

### 4.4 HTTP layer changes

**`/index.php/login`** (existing Phase 3 handler) — swap the bootstrap-only verification for `state.users.verify(login, password).await`. The 303 redirect + session mutation logic stays.

**New OCS endpoints** under `/ocs/v2.php/cloud/`:

- `GET /ocs/v2.php/cloud/user` — returns the authenticated user's `{ id, display-name, email, groups, enabled, last-login }`. Requires `AuthenticatedUser`.
- `PUT /ocs/v2.php/cloud/user` — body `{ key: "password"|"displayname"|"email", value, currentpassword }`. Requires `AuthenticatedUser` AND `currentpassword` must verify against the stored hash. `password` updates additionally trigger `SessionStore::destroy_all_for_except(uid, current_session_id)`.

**New auth extractor** in `crabcloud-http`:

- `AdminUser(pub User)` — wraps `AuthenticatedUser` and additionally verifies `state.users.is_admin(&user.uid)`. Rejects 403 otherwise. Ready for future admin endpoints; 2a doesn't ship any admin OCS handlers but the extractor lands so 2b/etc. don't re-invent it.

**Existing extractors unchanged**: `AuthenticatedUser` still resolves from the session cookie only (no Bearer/Basic — those are 2b). Internal: handlers that need the full `User` call `state.users.lookup(&authed.user_id).await`.

### 4.5 Session lifecycle on user changes

- **Password change** → `state.sessions.destroy_all_for_except(uid, current_id).await`. Forces other devices off; keeps the current session.
- **User disabled** → `state.sessions.destroy_all_for(uid).await` (no exception).
- **User deleted** → same as disabled, plus cascade-delete `oc_group_user` + `oc_preferences` rows.

This requires extending `SessionStore` (from `crabcloud-http::session`) with two new methods:

```rust
async fn destroy_all_for(&self, uid: &str) -> Result<(), CacheError>;
async fn destroy_all_for_except(&self, uid: &str, except: &SessionId) -> Result<(), CacheError>;
```

Implementation strategy for `MemoryCache`: maintain a side-index cache entry `{instance_id}:sessions_by_user:{uid}` whose value is a serialized `Vec<SessionId>`. Each successful login appends; each session destruction removes. The Redis backend (when it lands) will use a Redis SET keyed the same way.

---

## 5. Data model & migrations

A single new core migration `0002_users` ships in `migrations/core/0002_users/{sqlite,mysql,postgres}.sql`. Schema mirrors Nextcloud upstream column-for-column.

```sql
-- oc_users
CREATE TABLE oc_users (
    uid          VARCHAR(64)   NOT NULL,
    password     LONGTEXT,
    displayname  VARCHAR(64),
    email        VARCHAR(255),
    last_seen    BIGINT  NOT NULL DEFAULT 0,
    enabled      <bool>  NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
);
-- oc_groups
CREATE TABLE oc_groups (
    gid          VARCHAR(64)  NOT NULL,
    displayname  VARCHAR(64),
    PRIMARY KEY (gid)
);
-- oc_group_user
CREATE TABLE oc_group_user (
    gid  VARCHAR(64) NOT NULL,
    uid  VARCHAR(64) NOT NULL,
    PRIMARY KEY (gid, uid)
);
CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);
-- oc_preferences
CREATE TABLE oc_preferences (
    userid       VARCHAR(64) NOT NULL,
    appid        VARCHAR(32) NOT NULL,
    configkey    VARCHAR(64) NOT NULL,
    configvalue  LONGTEXT,
    PRIMARY KEY (userid, appid, configkey)
);
CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

-- Seed the canonical admin group
INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
```

**Per-dialect notes**:

- **`<bool>` placeholder**: `TINYINT(1)` on MySQL, `SMALLINT` on Postgres, `INTEGER` on SQLite — all storing `0`/`1`.
- **`LONGTEXT`**: MySQL native; Postgres uses `TEXT`; SQLite ignores the length and stores TEXT.
- **`VARCHAR(N)` semantics**: MySQL/Postgres enforce length; SQLite stores any length but accepts the declaration.
- **Email uniqueness**: SQLite and Postgres get a partial unique index `WHERE email IS NOT NULL`. MySQL doesn't support partial unique indexes pre-8.0 reliably, so it gets a non-unique index and `set_email` enforces uniqueness via an explicit `SELECT COUNT(*)` pre-check + `UsersError::EmailAlreadyTaken` mapped to HTTP 409.
- **`last_seen`**: Unix seconds, denormalized. Updated by `UserStore::touch_last_seen(uid)` on every successful auth. 0 = never logged in.
- **`enabled`**: Phase-1 add. Disabled users cannot log in (401 with the same masked message as wrong-password).

### 5.1 `oc_preferences` ownership

Owned by `UsersService::preferences()` for read/write of user-scoped key-value pairs. Distinct from the platform's `AppConfigService` (which owns `oc_appconfig` for global app values). Future sub-projects (e.g., per-user calendar settings) read/write here via `state.users.preferences().get(uid, "calendar", "default_view")` style.

### 5.2 Username canonicalization

`uid` is stored exactly as the admin provided it. `UsersService::lookup_by_login(login)` tries:

1. Exact match on `oc_users.uid`.
2. If `login` contains `@`, match on `LOWER(oc_users.email) = LOWER(login)`.

No third "case-insensitive uid" path in 2a; that would need a generated `uid_lower` column and is a follow-up.

---

## 6. Authentication flow

### 6.1 Login

`POST /index.php/login` handler in `crabcloud-http::routes::login`:

```
1. Parse form: { username, password }.
2. user = state.users.verify(username, password).await
   On Err: 401 ApiError (mask: "invalid credentials").
3. handle.mutate(|s| {
     s.user_id = Some(user.uid.into_inner());
     s.rotate_csrf();
     s.two_factor_passed = true;     // placeholder for 2c
   }).await;
4. SessionStore: record the SessionId in `sessions_by_user:{uid}` index.
5. 303 SEE_OTHER + Location: /.
```

### 6.2 `UsersService::verify` internals

```
1. user_with_hash = self.store.lookup_for_auth(login).await?  // returns User + hash + enabled
2. If user_with_hash is None OR !user_with_hash.user.enabled:
     verifier.verify(password, SENTINEL_HASH);  // constant-time fake check
     return Err(InvalidCredentials);
3. matches = verifier.verify(password, &user_with_hash.hash);
4. If !matches:
     return Err(InvalidCredentials);
5. self.store.touch_last_seen(&user_with_hash.user.uid).await?;
6. Ok(user_with_hash.user)
```

The sentinel hash is `bcrypt::hash("invalid", DEFAULT_COST)` computed once at `UsersService::new` and stored in an `OnceLock<String>`. This prevents the user-enumeration timing oracle.

### 6.3 Session model gains one field

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: Option<String>,
    pub csrf_token: String,
    pub last_activity: u64,
    pub two_factor_passed: bool,     // NEW
}
```

The field is always `true` in 2a (no 2FA enforcement yet); sub-project 2c's middleware will gate authenticated routes on it. Baking the field in now means 2c doesn't touch the cache-stored session serde shape.

### 6.4 Self-service password change

`PUT /ocs/v2.php/cloud/user` with body `{ key: "password", value: "new", currentpassword: "old" }`:

```
1. AuthenticatedUser extractor (401 if no session).
2. Verify currentpassword via state.users.verify(user.uid, currentpassword).
   On Err: 401 OcsError 997 — masked "current password incorrect".
3. state.users.set_password(&user.uid, "new").await?
   - Validates length (bcrypt 72-byte cap; min 1 char in 2a).
   - bcrypt::hash(new, DEFAULT_COST)
   - UPDATE oc_users SET password = ? WHERE uid = ?
4. state.sessions.destroy_all_for_except(&user.uid, &current_session_id).await?
5. OcsResponse::ok({}) — body is empty per Nextcloud's convention.
```

### 6.5 Self info

`GET /ocs/v2.php/cloud/user`:

```
1. AuthenticatedUser extractor.
2. user = state.users.lookup(&authed.user.uid).await?  // None → 401 (session points at deleted user)
3. groups = state.users.groups_of(&user.uid).await?
4. OcsResponse::ok({
     id: user.uid,
     "display-name": user.display_name,
     email: user.email,
     groups: groups,
     enabled: user.enabled,
     "last-login": user.last_seen,
   })
```

Field naming uses hyphens (`display-name`, `last-login`) — matches Nextcloud's wire shape. Internally Rust fields use snake_case; the serializer uses `#[serde(rename = "display-name")]`.

### 6.6 No new auth methods

Bearer / Basic / app-password are deferred to 2b. The `AuthenticatedUser` extractor in 2a still resolves only from the session cookie. Sub-project 2b adds the additional resolution paths without changing 2a's flow.

---

## 7. Bootstrap admin transition

### 7.1 Behavior matrix

| `bootstrap_admin` set? | `oc_users` empty? | Behavior |
|---|---|---|
| No | Yes | Server starts; `tracing::warn!` "no users configured; run `crabcloud-server user-add` or set `[bootstrap_admin]`". Login attempts return 401. |
| No | No | Normal. |
| Yes | Yes (or user not in DB) | `BootstrapAdminBackend` synthesizes a virtual admin from config. Login + verify work. Implicit admin-group membership. |
| Yes | Yes AND user IS in DB | `tracing::warn!` "ignoring `bootstrap_admin` in config; user exists in `oc_users` — remove the `[bootstrap_admin]` section". Use the DB user. |

### 7.2 `BootstrapAdminBackend`

Wraps any `UserStore`. Adds one fall-through path: if the wrapped backend's `lookup_by_login(login)` returns `None` AND `login == config.bootstrap_admin.username` AND no DB user exists with that uid, return a synthesized `User` and use `config.bootstrap_admin.password_hash` for verification.

**Self-service password change against the virtual user** is special-cased: `set_password` checks if the user exists in the wrapped backend's underlying DB; if not, it performs an `INSERT INTO oc_users` (creating the real DB row) AND `INSERT INTO oc_group_user (gid='admin', uid=...)`, effectively retiring the shim. Next boot's "ignoring bootstrap_admin" warning will then fire, prompting the operator to delete the config entry.

This preserves the fresh-install UX: drop a `[bootstrap_admin]` section into config.toml, boot, log in, change password — and you're now running on a real DB user.

---

## 8. CLI subcommands

New variants on `crabcloud-server`'s `Cmd` enum. All share the existing `serve` / `migrate` infrastructure: load config, build transient `AppState`, perform the operation, exit.

```
crabcloud-server user-add <uid> [--admin] [--email <addr>] [--display-name <name>]
crabcloud-server user-set-password <uid>
crabcloud-server user-delete <uid>
crabcloud-server group-add-member <gid> <uid>
crabcloud-server group-remove-member <gid> <uid>
```

- **`user-add`**: Prompts for password via stdin with hidden echo (`rpassword` crate). Hashes with `bcrypt::hash(password, DEFAULT_COST)`. Errors with non-zero exit if uid already exists or invalid.
- **`user-set-password`**: Prompts for new password.
- **`user-delete`**: Confirmation prompt: `"Delete user <uid> and all their preferences? (yes/no)"`. Cascades to `oc_group_user`, `oc_preferences`, and `SessionStore::destroy_all_for(uid)`.
- **`group-add-member` / `group-remove-member`**: For the rare cases where 2a's lack of admin OCS API would otherwise force direct DB editing.

**Sole new workspace dep**: `rpassword = "7"` — small, mature, used by `cargo login` itself.

---

## 9. Error handling

### 9.1 `UsersError` (in `crabcloud-users::error`)

```rust
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
```

### 9.2 Mapping to `crabcloud-core::Error`

A new `Error::Users(#[from] UsersError)` variant. `Error::http_status` extension:

| `UsersError` variant | HTTP |
|---|---|
| `NotFound` | 404 |
| `InvalidCredentials` / `Disabled` | 401 (don't distinguish) |
| `InvalidUid` / `InvalidEmail` / `PasswordTooWeak` | 400 |
| `UidAlreadyExists` / `EmailAlreadyTaken` | 409 |
| `ReadOnly` | 403 |
| `Db` / `Cache` / `Internal` | 500 |

`Error::client_message` follows the established pattern — pass-through for 4xx, masked "Internal Server Error" for 5xx.

---

## 10. Validation

| Field | Rule |
|---|---|
| `uid` | 1–64 chars; `[A-Za-z0-9._@-]`. Rejects whitespace, slashes, control. Matches Nextcloud's `OC_User::isValidUserId`. |
| `email` | Validated via `email_address` crate (small, well-tested). Trimmed + lowercased before storage. |
| `password` | 1–72 bytes. bcrypt itself caps at 72; reject longer with `PasswordTooWeak("max 72 bytes")`. No strength requirement in 2a. |
| `display_name` | 1–64 chars, no control. Empty defaults to `uid`. |

Validation lives in newtype constructors (`UserId::new`, `Email::parse`) that return `Result<Self, UsersError>`. Storage methods take `&UserId` not `&str`, making it impossible to insert an unvalidated value.

**Sole new dep for validation**: `email_address = "0.2"` — small, well-tested RFC 5321/5322 validator.

---

## 11. Testing strategy

| Layer | Count | Notes |
|---|---|---|
| Unit — `user.rs`, `password.rs`, `store/sql.rs`, `service.rs` | ~25 | UserId/Email validation; bcrypt verify; SqlUserStore CRUD against in-process SQLite; group membership; preferences round-trip; constant-time fake-verify proves equal latency for known + unknown users (statistical check, not bit-exact). |
| Integration — `crates/crabcloud-users/tests/users_flow.rs` | ~6 | End-to-end via `build_router` + `AppStateBuilder`: login with real user; login with disabled user (401); wrong password (401); password change with wrong current (401); password change destroys other sessions but keeps current; `GET /ocs/v2.php/cloud/user` shape matches Nextcloud. |
| Multi-dialect | shared | `SqlUserStore` tests run against MySQL/Postgres via the existing testcontainers / CI service-container setup. |
| Migration smoke | 1 | Apply 0002 against fresh DB; assert tables exist + `admin` group seeded. |
| CLI smoke — `assert_cmd`-based | ~3 | `user-add`, `user-set-password`, `user-delete` against a temp SQLite. |
| Bootstrap transition | 2 | (a) Boot with `bootstrap_admin` + empty DB → admin can log in. (b) Self-service password change against the virtual user creates `oc_users` row + `admin` group membership. |

**Sole new dev-dep**: `assert_cmd = "2"` — standard CLI testing tool.

---

## 12. Acceptance criteria

The sub-project is complete when **all** of the following hold:

1. `cargo xtask check-all` passes against SQLite + MySQL + Postgres.
2. `crabcloud-server user-add alice --admin` creates `oc_users` + `oc_group_user` rows. Subsequent `POST /index.php/login` with alice's credentials succeeds.
3. With only `[bootstrap_admin]` configured and no `oc_users` rows, the configured admin can still log in.
4. Self-service password change against the bootstrap-admin virtual user persists into `oc_users` and creates the `(admin, <uid>)` group-membership row.
5. A user with `enabled = 0` cannot log in (401, same masked message as wrong password).
6. `PUT /ocs/v2.php/cloud/user` with `key=password` updates the hash AND destroys other devices' sessions for the same uid AND keeps the current device's session.
7. `GET /ocs/v2.php/cloud/user` returns `{ id, display-name, email, groups, enabled, last-login }`.
8. Existing Playwright E2E suite still passes — the bootstrap-admin login path remains the smoke-test path.
9. No `rustcloud_*` references — `git grep -i rustcloud` empty (carryover hygiene).
10. `crabcloud-users` crate appears in `[workspace.lints]`'s reach with zero warnings under `-D warnings`.

---

## 13. Open questions (deferred, not blockers)

- **bcrypt cost**: 12 is the current default. Phase 5+ may want to bump to 13 once profiled. The constant lives in `crabcloud-users::password::BCRYPT_COST`.
- **Email uniqueness on MySQL**: enforced application-side. A future MySQL 8.0+ upgrade may allow a partial unique index; revisit when the DB matrix changes.
- **Session storage scaling**: `MemoryCache`'s `sessions_by_user` index grows linearly. When Redis becomes the cache backend, swap to a Redis SET. Documented as a known follow-up.
- **`oc_preferences` LIST API**: 2a ships `get`/`set`/`delete`/`list-by-app` but no bulk-clear-all-prefs-for-user. The `user-delete` CLI does this via direct SQL; a public bulk method may land with the GDPR-export sub-project.

---

## 14. Glossary

- **`uid`** — the user's immutable login identifier. Nextcloud convention: short string like `"alice"`.
- **`gid`** — group identifier. The canonical admin group's gid is `"admin"`.
- **Bootstrap admin shim** — the `BootstrapAdminBackend` that synthesizes a virtual user from `config.bootstrap_admin` when no DB user matches. Retires itself when a real DB user is created.
- **Constant-time fake-verify** — running bcrypt against a sentinel hash on lookup-miss so successful and unsuccessful logins take the same time, blocking user-enumeration timing attacks.
- **`destroy_all_for_except`** — kicks all of a user's other sessions but keeps the current one. Used on self-service password change.
- **`PasswordVerifier`** — trait for password-hash verification. 2a's only impl is `BcryptVerifier`. Future multi-format verifiers layer on this without changing call sites.
