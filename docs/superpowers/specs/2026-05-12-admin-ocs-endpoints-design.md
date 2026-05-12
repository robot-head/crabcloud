# Admin OCS Endpoints Design (sub-project 2-admin)

**Status:** approved 2026-05-12.
**Parent:** sub-projects 2a (`crabcloud-users` — `UserStore` / `GroupStore` / `BootstrapAdminBackend`) and 2b (`AppPasswordService` + `oc_authtoken`), both shipped to master.
**Predecessor:** the `AdminUser` extractor (Batch 2a Task 11) is in place; `state.users.app_passwords()` returns `Some(_)` on the default builder path.

## 1. Goal

Implement Nextcloud-compatible admin OCS endpoints so the existing Nextcloud Admin app, occ-style tooling, and any wire-compatible client can administer users + groups against Crabcloud. Fourteen handlers (10 user-related + 4 group, see §5) live in two new files under `crates/crabcloud-http/src/routes/ocs/`. All gated by the existing `AdminUser` extractor.

After 2-admin ships:

- `POST /ocs/v2.php/cloud/users` provisions a new user (validated uid, password, optional email, optional initial groups).
- `GET /ocs/v2.php/cloud/users?search=&limit=&offset=` returns a paginated uid list with case-insensitive substring search across `uid`, `displayname`, `email`.
- `PUT /ocs/v2.php/cloud/users/{uid}` rotates password (admin override; no `currentpassword`), display name, or email.
- `DELETE /ocs/v2.php/cloud/users/{uid}` removes the user and cascades to `oc_authtoken`, `oc_group_user`, `oc_preferences`.
- `PUT /ocs/v2.php/cloud/users/{uid}/enable` flips the enabled flag back on.
- `PUT /ocs/v2.php/cloud/users/{uid}/disable` flips it off AND deletes every `oc_authtoken` row for the user (forced logout everywhere).
- `GET/POST/DELETE /ocs/v2.php/cloud/users/{uid}/groups` lists or mutates the target's group memberships.
- `GET/POST /ocs/v2.php/cloud/groups` lists groups (same search shape as users) or creates one.
- `GET /ocs/v2.php/cloud/groups/{gid}` returns the group's members.
- `DELETE /ocs/v2.php/cloud/groups/{gid}` deletes a non-`admin` group.

## 2. Scope

**In:**

- Fourteen admin OCS handlers (10 user-related + 4 group; see §5).
- New trait methods on `UserStore` and `GroupStore` (`list_users(filter)`, `list_groups(filter)`, `exists_in_storage(uid)`).
- New façade methods on `UsersService` (`disable_user(uid)`, `delete_user(uid)`).
- New helper on `AppPasswordService` (`revoke_all_for_user(uid)`).
- `BootstrapAdminBackend::exists_in_storage` override that forwards to the inner SQL store (so the virtual admin is invisible to admin OCS).
- Self-action guards: admin can't delete themselves / disable themselves / remove themselves from `admin` group.
- Structural guards: `admin` group cannot be deleted; last admin cannot be removed.
- `POST /cloud/users` validates the optional `groups[]` against `oc_groups` *before* creating the user (no partial creates).
- Acceptance tests (unit + integration + Playwright e2e).

**Out (deferred):**

- Sub-admins (`/users/{uid}/subadmins`, per-group admin permission model).
- Quota management (`PUT /cloud/users/{uid}` with `key=quota`).
- Email-verification on `PUT email`.
- Rate-limiting on admin write endpoints.
- LDAP/SAML "can this user be edited by OCS?" predicate (lands with those sub-projects).
- DB-backed audit log (we emit `tracing` events; no audit table).

## 3. Architecture

```
                           ┌────────────────────────────────────┐
   request ────────────────│ AuthLayer (existing)               │
                           │   attaches AuthContext{method,uid} │
                           └───────────┬────────────────────────┘
                                       ▼
                           ┌────────────────────────────────────┐
                           │ AdminUser extractor (existing)     │
                           │   401 unauth / 403 non-admin       │
                           └───────────┬────────────────────────┘
                                       ▼
                           ┌────────────────────────────────────┐
                           │ admin OCS handlers (NEW)           │
                           │   - admin_users.rs                 │
                           │   - admin_groups.rs                │
                           └───────────┬────────────────────────┘
                                       ▼
                           ┌────────────────────────────────────┐
                           │ UsersService (extended)            │
                           │   + disable_user / delete_user     │
                           └──┬─────────────────────────┬───────┘
                              │                         │
                              ▼                         ▼
                ┌──────────────────┐         ┌──────────────────┐
                │ UserStore /      │         │ AppPassword-     │
                │ GroupStore (ext) │         │ Service (ext)    │
                │   + list_users   │         │ + revoke_all_    │
                │   + list_groups  │         │   for_user       │
                │   + exists_      │         └──────────────────┘
                │     in_storage   │
                └──────────────────┘
```

No new middleware. No new migration. No new error variants on `UsersError` or `crabcloud-core::Error`.

### 3.1 File layout

```
crates/
├── crabcloud-http/                                # MODIFIED
│   └── src/routes/ocs/
│       ├── admin_users.rs                  (NEW)  # 10 user/user-groups handlers + tests
│       ├── admin_groups.rs                 (NEW)  # 4 group handlers + tests
│       └── mod.rs                                 # mount new routers
│
└── crabcloud-users/                               # MODIFIED
    └── src/
        ├── store/
        │   ├── mod.rs                             # trait additions
        │   ├── sql.rs                             # SqlUserStore + SqlGroupStore additions
        │   └── bootstrap_shim.rs                  # exists_in_storage override
        ├── app_password.rs                        # revoke_all_for_user helper
        └── service.rs                             # disable_user + delete_user façades
```

`crabcloud-ui` is untouched (no admin UI ships in this sub-project; the Settings admin pages are out of scope). `crabcloud-server` CLI is untouched (existing `user-add` / `user-set-password` / `user-delete` / `group-add-member` / `group-remove-member` cover the operator CLI surface; the new HTTP endpoints are for clients).

### 3.2 Layer responsibilities

- **Existing `AuthLayer` + `AdminUser` extractor**: unchanged. The extractor returns 401 (no auth context), 403 (auth but not admin), or `AdminUser(AuthenticatedUser)` on success.
- **New admin handlers**: validate inputs, dispatch to `UsersService` or its store accessors, build OCS envelopes via the existing `render(&OcsResponse, fmt)` path.
- **`UsersService::disable_user` / `delete_user`**: orchestrate cross-table state changes (set_enabled + revoke tokens; revoke tokens + delete user). Document non-atomicity + retry-idempotence like `set_password`.
- **`UserStore::exists_in_storage`**: a trait method that bypasses the bootstrap shim's synthesizing `lookup`. Default impl delegates to `lookup`; `BootstrapAdminBackend` overrides to delegate to `inner.exists_in_storage(uid)` so the virtual admin returns `false`.

## 4. Data model

No new tables, columns, or migrations. Reuses:

- `oc_users` (uid, displayname, email, enabled, last_seen, password).
- `oc_groups` (gid, displayname).
- `oc_group_user` (gid, uid).
- `oc_authtoken` (all 17 cols; the relevant ones for cascades are uid, password_invalid).
- `oc_preferences` (userid).

### 4.1 New Rust surface

```rust
// crabcloud-users/src/store/mod.rs

pub struct UserListFilter<'a> {
    pub search: Option<&'a str>,   // case-insensitive substring; LIKE %term% on uid/displayname/email
    pub limit:  u32,                // clamped to [1, 500] by the handler
    pub offset: u32,
}

pub struct GroupListFilter<'a> {
    pub search: Option<&'a str>,   // LIKE %term% on gid/displayname
    pub limit:  u32,
    pub offset: u32,
}

#[async_trait]
pub trait UserStore: /* existing bounds */ {
    // existing methods …
    async fn list_users(&self, filter: UserListFilter<'_>) -> UsersResult<Vec<User>>;
    /// True iff a real row exists in `oc_users`. The default impl delegates to
    /// `lookup`; `BootstrapAdminBackend` overrides so a synthesized virtual
    /// admin returns `false`.
    async fn exists_in_storage(&self, uid: &UserId) -> UsersResult<bool> {
        Ok(self.lookup(uid).await?.is_some())
    }
}

#[async_trait]
pub trait GroupStore: /* existing bounds */ {
    // existing methods …
    async fn list_groups(&self, filter: GroupListFilter<'_>) -> UsersResult<Vec<Group>>;
}

// crabcloud-users/src/app_password.rs

impl AppPasswordService {
    // existing methods …
    /// Delete every token row owned by `uid`. Used by `UsersService::{disable,delete}_user`.
    /// Implementation forwards to `TokenAuthCache::revoke_all_for_user_except(uid, i64::MIN)`
    /// — no row's id is `MIN`, so nothing is preserved.
    pub async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()> { … }
}

// crabcloud-users/src/service.rs

impl UsersService {
    // existing methods …

    /// Flip enabled=false AND delete every token row for `uid`. Non-atomic:
    /// the two SQL statements run separately. If the cascade fails, the user
    /// is left `enabled=false` with some token rows still present — retry is
    /// idempotent (both target tables converge on the desired state).
    pub async fn disable_user(&self, uid: &UserId) -> UsersResult<()> { … }

    /// Delete every token row for `uid`, THEN delete the `oc_users` row
    /// (which cascades to `oc_group_user` + `oc_preferences`). Token cascade
    /// runs first so a racing auth lookup against a deleted user can't
    /// authenticate. Non-atomic; retry is idempotent.
    pub async fn delete_user(&self, uid: &UserId) -> UsersResult<()> { … }
}
```

## 5. Wire format & HTTP surface

### 5.1 Auth + envelope conventions (unchanged from prior OCS surfaces)

Every endpoint takes `AdminUser` as a request extractor (returns 401 anonymous, 403 if authed-but-not-admin). Every response is rendered through `crabcloud_ocs::render` with the existing `OcsFormat` extractor honoring `?format=json` (default JSON in this sub-project; XML supported via `?format=xml` per the OCS spec). Error responses route through `OcsError`.

### 5.2 User endpoints

#### `GET /ocs/v2.php/cloud/users?search=&limit=100&offset=0`

Query params: `search` (optional, default `""`), `limit` (1..=500, default 100), `offset` (0..=∞, default 0).

Search is case-insensitive substring: `LIKE %term%` on `uid OR displayname OR email`. Empty search returns all rows in `uid` order.

Response:

```json
{ "ocs": { "meta": { "statuscode": 200, "status": "ok" },
          "data": { "users": ["alice", "bob", …] } } }
```

Bare uid list — matches Nextcloud's wire format. Clients GET `/cloud/users/{uid}` per row for details.

#### `POST /ocs/v2.php/cloud/users`

Form body:

| field         | required | shape                                 |
|---------------|----------|---------------------------------------|
| `userid`      | yes      | `UserId::new` accepts (1–64 chars, `[A-Za-z0-9._@-]`) |
| `password`    | yes      | 1–72 bytes, `BcryptVerifier::hash` validates |
| `email`       | no       | `Email::parse` accepts (or empty/omitted) |
| `displayName` | no       | 1–64 chars, no control chars; default = `userid` |
| `groups[]`    | no       | each validated as `GroupId` AND existing in `oc_groups` |

Flow:

1. Validate every field and resolve every `groups[]` member via `group_store().lookup` — reject the whole request with 400 if any gid is unknown.
2. Hash the password.
3. `user_store().create(&user, Some(&hash))` (rejects duplicate uid / email per 2a's existing checks).
4. For each `gid` in `groups[]`: `group_store().add_to_group(&uid, &gid)`.
5. Return 200 with `{ ocs.data.id: "<uid>" }`.

Failure modes:

- 400 `InvalidUid` / `InvalidEmail` / `InvalidDisplayName` / `PasswordTooWeak` for field validation.
- 400 `BadRequest("unknown group: <gid>")` for unresolved `groups[]` member.
- 409 `UidAlreadyExists`.
- 409 `EmailAlreadyTaken`.

#### `GET /ocs/v2.php/cloud/users/{uid}`

Returns the full per-user record (same shape as the existing self-OCS `GET /cloud/user` endpoint added in 2a):

```json
{ "ocs": { "meta": { "statuscode": 200, "status": "ok" },
          "data": { "id": "alice",
                    "display-name": "Alice Smith",
                    "email": "alice@example.com",
                    "groups": ["admin"],
                    "enabled": true,
                    "last-login": 0 } } }
```

404 if `exists_in_storage(uid) == false` (covers both nonexistent uid and bootstrap virtual admin).

#### `PUT /ocs/v2.php/cloud/users/{uid}`

Form body: `key`, `value`. `key` ∈ `{ "password", "displayname", "email" }`. No `currentpassword` — admin can rotate without knowing the user's current password.

- `password` → `users.set_password(uid, value)`. Cascades `password_invalid=1` on the target's tokens (existing UsersService cascade from 2b). The admin's own session is unaffected because the target uid is not the admin's uid (and the cascade only touches the target's rows). Self-action guard: if `uid == authed.user_id`, return 400 — admins must use the self-service `PUT /cloud/user` (which requires `currentpassword`) to rotate their own password. This preserves the spec §6.3 boundary from 2b.
- `displayname` → `user_store().set_display_name(uid, value)`.
- `email` → `user_store().set_email(uid, Some(value))` if non-empty, else `None`.

Failure modes: 400 unknown key, 400 invalid value (per existing validators), 409 email duplicate.

#### `DELETE /ocs/v2.php/cloud/users/{uid}`

Guards:

- 404 if `!exists_in_storage(uid)`.
- 400 if `uid == authed.user_id` (admin can't delete themselves).
- 400 if `uid` is the only admin (would lock out the cluster).

On success: `users.delete_user(uid)` — token-cascade-then-row-delete (see §4.1).

#### `PUT /ocs/v2.php/cloud/users/{uid}/enable`

`user_store().set_enabled(uid, true)`. No cascade — tokens that were cascade-revoked during disable stay deleted; the user re-pairs from scratch. 404 if `!exists_in_storage`.

#### `PUT /ocs/v2.php/cloud/users/{uid}/disable`

Guards:

- 404 if `!exists_in_storage`.
- 400 if `uid == authed.user_id`.
- 400 if `uid` is the only admin.

On success: `users.disable_user(uid)` — set_enabled(false) + revoke_all_for_user. Every browser session and app password for the target is gone.

#### `GET /ocs/v2.php/cloud/users/{uid}/groups`

404 on virtual admin / unknown. Returns `{ ocs.data.groups: ["admin", "developers", …] }`.

#### `POST /ocs/v2.php/cloud/users/{uid}/groups`

Form body: `groupid`. Validates `gid` via `GroupId::new`, checks it exists in `oc_groups` (400 if not), then `group_store().add_to_group(uid, gid)`. The add is idempotent (`INSERT OR IGNORE` / `ON CONFLICT DO NOTHING`). 404 on virtual admin / unknown uid.

#### `DELETE /ocs/v2.php/cloud/users/{uid}/groups?groupid=...`

Guards:

- 404 if `!exists_in_storage(uid)` or unknown gid.
- 400 if `gid == "admin"` AND `uid == authed.user_id` (admin can't remove themselves from admin group).
- 400 if `gid == "admin"` AND target uid is the last admin.

On success: `group_store().remove_from_group(uid, gid)`. Idempotent (delete-on-empty is fine).

### 5.3 Group endpoints

#### `GET /ocs/v2.php/cloud/groups?search=&limit=100&offset=0`

Same shape as `GET /users`: LIKE on `gid OR displayname`. Returns `{ ocs.data.groups: ["admin", "developers", …] }`.

#### `POST /ocs/v2.php/cloud/groups`

Form body:

| field         | required | shape                                       |
|---------------|----------|---------------------------------------------|
| `groupid`     | yes      | `GroupId::new` accepts (1–64 chars, charset) |
| `displayname` | no       | defaults to `groupid` if empty/omitted       |

`group_store().create(&Group { gid, display_name })`. 409 if gid exists.

#### `GET /ocs/v2.php/cloud/groups/{gid}`

Returns `{ ocs.data.users: ["alice", "bob", …] }` — the group's members. 404 if gid unknown.

#### `DELETE /ocs/v2.php/cloud/groups/{gid}`

Guards:

- 400 if `gid == "admin"` (structural).
- 404 if gid unknown.

On success: `group_store().delete(gid)` (already cascades `oc_group_user`).

## 6. Security

### 6.1 Auth boundary

Every handler takes `AdminUser` — the 2a extractor that wraps `AuthenticatedUser` and runs `state.users.is_admin(uid)`. Unauthenticated → 401; authenticated-but-not-admin → 403. Bearer/Basic/Cookie all reach this point identically thanks to 2b's `AuthLayer` unifying them.

### 6.2 Self-action guards

The calling admin's uid is `authed.0.user_id`. Four boundaries:

- **Self-delete forbidden.** `DELETE /cloud/users/{uid}` with `uid == authed.0.user_id` → 400 `cannot delete the calling admin`.
- **Self-disable forbidden.** `PUT /cloud/users/{uid}/disable` with `uid == authed.0.user_id` → 400.
- **Self-remove-from-admin forbidden.** `DELETE /cloud/users/{uid}/groups?groupid=admin` with `uid == authed.0.user_id` → 400.
- **Self-password-rotation forbidden via admin endpoint.** `PUT /cloud/users/{uid}` with `uid == authed.0.user_id` AND `key == "password"` → 400 with a message pointing to the self-service endpoint.

These are belt-and-suspenders against accident; not security-class. The structural last-admin guard below is the real one.

### 6.3 Last-admin guard

Before every action that could remove the last admin from the `admin` group:

```rust
let admins = state.users.group_store().members_of(&GroupId::new("admin")?).await?;
if admins.len() == 1 && admins.contains(&uid) {
    return Err(BadRequest("at least one admin must remain"));
}
```

Fires on:

- `DELETE /cloud/users/{uid}/groups?groupid=admin` when `uid` is the sole admin.
- `PUT /cloud/users/{uid}/disable` when `uid` is the sole admin.
- `DELETE /cloud/users/{uid}` when `uid` is the sole admin.

One extra `SELECT uid FROM oc_group_user WHERE gid='admin'` per affected call; acceptable.

### 6.4 Structural guards

- `DELETE /cloud/groups/admin` → 400 `the admin group is structural`. Always.
- `POST /cloud/groups` with `groupid == "admin"` already exists → 409 via the unique key. Not a guard, just behaviour.

### 6.5 Cascade behaviour (recap from §4.1)

- **Delete:** token-revoke FIRST, then row-delete. Tokens-first means a racing in-flight auth either misses (row already gone via the AuthLayer cookie path) or 401s because the token row is gone.
- **Disable:** `set_enabled=false` first, then `revoke_all_for_user`. If revoke fails, the user is `enabled=false` but holds live tokens — but the AuthLayer's `service.verify` path for cookie auth checks `user.enabled`, so subsequent cookie auth fails anyway. Bearer/Basic auth via AuthLayer doesn't currently re-check `user.enabled` after token-lookup (see §6.6) — relying on revoke as the primary mechanism is correct.

### 6.6 Known auth-path gap (out of scope, tracked)

The 2b `AuthLayer` resolves a token row → `AuthContext` and DOES NOT re-check `user.enabled` (it doesn't query `oc_users` on the hot path; it trusts the token-lookup cache). This means a previously-issued Bearer/Basic token for a `enabled=false` user could continue to authenticate until the row is revoked.

Disable's cascade (revoke_all_for_user) closes this by deleting the token. **The acceptance test verifies this end-to-end.** A future hardening step (likely in 2c or as a follow-up) is to add `user.enabled` to the `AuthLayer` post-lookup check — but that requires either a join in `lookup_for_auth` or a second query on every auth, neither of which 2b plumbed. Documented in the changelog.

### 6.7 Bootstrap virtual admin is invisible

The shim's `lookup` synthesizes a virtual user from `config.bootstrap_admin` when the inner store misses. Every admin handler that targets a specific `{uid}` calls `state.users.user_store().exists_in_storage(&uid)` and 404s on `false`. The new trait method is overridden on `BootstrapAdminBackend` to delegate to `inner.exists_in_storage(uid)` — the inner `SqlUserStore` uses the default impl, which queries `oc_users` via `lookup`. The virtual admin therefore returns `false` and is invisible to all admin operations. List endpoints (`GET /users`) call the inner `list_users` directly via the shim's pass-through, which also returns only real DB rows.

The operator's options for "remove the bootstrap admin":

1. Log in with the virtual admin, change the password (which promotes the row into `oc_users`), then administer normally via admin OCS.
2. Remove the `[bootstrap_admin]` block from `config.toml` and restart.

Both paths are documented in the existing 2a quick-start.

### 6.8 Admin write audit trail

Every admin write (POST/PUT/DELETE on `/cloud/users/**` and `/cloud/groups/**`) emits:

```
tracing::info!(
    actor = %authed.0.user_id,
    action = "create_user",          // or delete_user, set_password, disable, etc.
    target_uid = %uid,                // or target_gid for group endpoints
    "admin OCS write"
)
```

No PII (no email body, no password). Failures additionally emit `tracing::warn!` with the error.

### 6.9 Rate-limiting

Out of scope. Same posture as 2b's `getapppassword` / `create_app_password`. A future cross-cutting rate-limit middleware can cover this surface and the 2b surface together.

## 7. Error model

No new variants on `UsersError` or `crabcloud-core::Error`. The existing mapping in `crabcloud-core::Error::users_status` covers everything we need:

| Failure scenario                                | UsersError variant         | HTTP |
|--------------------------------------------------|----------------------------|------|
| Unknown uid / unknown gid / virtual-admin target | (handled via 404)          | 404  |
| Duplicate uid on POST users                      | `UidAlreadyExists`         | 409  |
| Duplicate email                                  | `EmailAlreadyTaken`        | 409  |
| Invalid field shape                              | `Invalid{Uid,Email,DisplayName}` / `PasswordTooWeak` | 400 |
| Self-action / last-admin / admin-group / unknown-key | `CoreError::BadRequest(msg)` | 400 |
| Storage / cache backend failure                  | `Db` / `Cache` / `Internal` | 500  |

OCS envelope status codes follow `crabcloud_ocs::OcsResponse::ok` for success and `OcsError::new(CoreError::*, OcsVersion::V2, fmt)` for failure.

## 8. Implementation skeleton (illustrative)

### 8.1 `routes/ocs/admin_users.rs`

```rust
use crate::extractors::auth::AdminUser;
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::{Email, GroupId, User, UserId, UserListFilter};
use serde::{Deserialize, Serialize};

// shared envelope helpers (analogous to existing user.rs)
fn ocs_ok<T: Serialize>(payload: T, fmt: crabcloud_ocs::Format) -> Response { … }
fn users_err(e: crabcloud_users::UsersError, fmt: crabcloud_ocs::Format) -> OcsError { … }
fn not_found(fmt: crabcloud_ocs::Format) -> OcsError { … }
fn bad_request(msg: String, fmt: crabcloud_ocs::Format) -> OcsError { … }

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)] pub search: Option<String>,
    #[serde(default = "default_limit")] pub limit: u32,
    #[serde(default)] pub offset: u32,
}
fn default_limit() -> u32 { 100 }

#[derive(Debug, Serialize)]
struct ListPayload { users: Vec<String> }

pub async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Query(q): Query<ListQuery>,
) -> Result<Response, OcsError> {
    let limit = q.limit.clamp(1, 500);
    let filter = UserListFilter {
        search: q.search.as_deref().filter(|s| !s.is_empty()),
        limit,
        offset: q.offset,
    };
    let rows = state.users.user_store().list_users(filter).await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(ListPayload { users: rows.into_iter().map(|u| u.uid.into_inner()).collect() }, fmt.0))
}

// remaining handlers: create_user, get_user, edit_user, delete_user,
// enable_user, disable_user, list_user_groups, add_user_to_group,
// remove_user_from_group
```

### 8.2 `routes/ocs/admin_groups.rs`

Same shape: `list_groups`, `create_group`, `list_group_members`, `delete_group`. Reuses the envelope helpers (extract them into a `routes/ocs/util.rs` if helpful, or duplicate the four-line helpers; defer the dedup decision to plan time).

### 8.3 Mount in `routes/ocs/mod.rs`

```rust
pub mod admin_users;
pub mod admin_groups;
pub mod app_password;
pub mod capabilities;
pub mod user;

use axum::routing::{delete, get, post, put};
use axum::Router;
use crabcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // existing routes …
        .route("/v2.php/cloud/users",
            get(admin_users::list_users).post(admin_users::create_user))
        .route("/v2.php/cloud/users/{uid}",
            get(admin_users::get_user)
                .put(admin_users::edit_user)
                .delete(admin_users::delete_user))
        .route("/v2.php/cloud/users/{uid}/enable",
            put(admin_users::enable_user))
        .route("/v2.php/cloud/users/{uid}/disable",
            put(admin_users::disable_user))
        .route("/v2.php/cloud/users/{uid}/groups",
            get(admin_users::list_user_groups)
                .post(admin_users::add_user_to_group)
                .delete(admin_users::remove_user_from_group))
        .route("/v2.php/cloud/groups",
            get(admin_groups::list_groups).post(admin_groups::create_group))
        .route("/v2.php/cloud/groups/{gid}",
            get(admin_groups::list_group_members)
                .delete(admin_groups::delete_group))
}
```

### 8.4 `UsersService::disable_user` / `delete_user`

```rust
impl UsersService {
    pub async fn disable_user(&self, uid: &UserId) -> UsersResult<()> {
        self.users.set_enabled(uid, false).await?;
        if let Some(ap) = &self.app_passwords {
            ap.revoke_all_for_user(uid).await?;
        }
        Ok(())
    }

    pub async fn delete_user(&self, uid: &UserId) -> UsersResult<()> {
        if let Some(ap) = &self.app_passwords {
            ap.revoke_all_for_user(uid).await?;
        }
        self.users.delete(uid).await?;
        Ok(())
    }
}
```

Both share the cascade-retry semantics documented on `set_password`. The handler-side wrapping is small:

```rust
state.users.delete_user(&uid).await.map_err(|e| users_err(e, fmt.0))?;
tracing::info!(
    actor = %admin.0.user_id,
    action = "delete_user",
    target_uid = %uid,
    "admin OCS write"
);
```

## 9. Testing

### 9.1 Unit (`crabcloud-users`)

- `SqlUserStore::list_users`: 4 tests
  - empty-search returns all rows in `uid` ASC order
  - substring matches across `uid`, then `displayname`, then `email` (3 sub-cases, parameterized or repeated)
  - `limit` clamps; pagination via `offset` returns disjoint windows
  - empty-string `search` (after the handler's optional-strip) is equivalent to "no filter"
- `SqlGroupStore::list_groups`: 3 tests (analogous, no email).
- `UserStore::exists_in_storage` default impl: 2 tests
  - real `oc_users` row → `true`
  - missing row → `false`
- `BootstrapAdminBackend::exists_in_storage`: 2 tests
  - virtual admin (no DB row) → `false` (even though `lookup` would synthesize)
  - promoted admin (real DB row) → `true`
- `UsersService::disable_user`: cascade test
  - mint a token for `uid`
  - call `disable_user(uid)`
  - assert `users.lookup(uid).unwrap().enabled == false` AND `app_passwords.verify(raw)` returns `TokenNotFound`
- `UsersService::delete_user`: cascade test
  - mint a token for `uid`, add to a group, write a preference
  - call `delete_user(uid)`
  - assert: `users.lookup(uid)` is `None`; group membership is gone; preference is gone; `app_passwords.verify(raw)` returns `TokenNotFound`

### 9.2 Integration (`crabcloud-http`)

- `routes/ocs/admin_users.rs::tests` and `routes/ocs/admin_groups.rs::tests` with focused tests per endpoint. Pattern follows the existing `routes/ocs/user.rs::tests` (build a real `AppState`, seed an admin user, drive the router via `build_router(state, axum::Router::new()).oneshot(req)`).
- Cross-cutting: anonymous → 401, non-admin → 403 (one test per endpoint, parameterized).
- Bootstrap virtual admin: build state with `[bootstrap_admin]` set, do not promote, hit every `{uid}` endpoint with the virtual uid, assert 404.
- Self-action guards: 4 tests (delete-self 400, disable-self 400, self-remove-from-admin 400, self-password-rotation-via-admin 400).
- Last-admin guards: 3 tests (disable last admin 400, delete last admin 400, remove last admin from group 400).
- Disable cascade: mint a Bearer token for a non-admin user → token authenticates `GET /cloud/user`; call `PUT /cloud/users/{uid}/disable` as admin; same Bearer → 401.
- Delete cascade: similar + verify `oc_group_user` and `oc_preferences` rows are gone (direct SQL check).
- POST users with `groups[]`: valid → user created + membership added; unknown gid → 400 AND user not created (assert via follow-up GET `/cloud/users/{uid}` returning 404).
- Admin PUT password on a non-self target: target's other tokens become `password_invalid=1`; admin's own session still works.
- Search: 4 tests (uid match, displayname match, email match, pagination).
- Group list members: returns the right uids.
- Delete admin group: 400.

### 9.3 E2E (Playwright)

One new spec `e2e/tests/admin_ocs.spec.ts`:

1. Login as `admin/hunter2`. Extract cookie.
2. POST `/cloud/users` `{userid: "bob", password: "bobpw", email: "bob@example.com"}` → 200.
3. GET `/cloud/users/bob` → 200 with the right shape.
4. PUT `/cloud/users/bob` `{key: "displayname", value: "Bob B."}` → 200.
5. GET `/cloud/users/bob` → displayname updated.
6. Login as `bob/bobpw` in a second request context; verify token works.
7. PUT `/cloud/users/bob/disable` as admin → 200.
8. Reuse bob's token → 401.
9. PUT `/cloud/users/bob/enable` → 200. (Bob still needs to re-pair; that's expected.)
10. DELETE `/cloud/users/bob` → 200.
11. GET `/cloud/users/bob` → 404.

## 10. Acceptance criteria

| #  | Criterion                                                                                                              | Source of truth        |
|----|------------------------------------------------------------------------------------------------------------------------|------------------------|
| 1  | `cargo xtask check-all` clean against SQLite + MySQL + Postgres                                                        | CI                     |
| 2  | Non-admin caller → 403 on every endpoint; anonymous → 401                                                              | Integration test       |
| 3  | POST `/cloud/users` with valid body creates a row; GET `/cloud/users/{uid}` returns the full record; DELETE removes + cascades tokens + group memberships + preferences | Integration test       |
| 4  | PUT `/cloud/users/{uid}/disable` immediately invalidates all token rows for the target; subsequent Bearer-auth from that user → 401 | Integration test (covers §6.6) |
| 5  | PUT `/cloud/users/{uid}` with `key=password&value=…` rotates the password AND marks the target's other tokens `password_invalid=1`; the calling admin's own session keeps working | Integration test       |
| 6  | GET `/cloud/users?search=...` returns the right uids; LIKE matches uid + displayname + email; pagination works         | Integration test       |
| 7  | Self-delete / self-disable / self-remove-from-admin / self-password-via-admin → 400 with the expected message          | Integration test       |
| 8  | Bootstrap virtual admin is invisible to all `{uid}`-path operations (404) and to list                                  | Integration test       |
| 9  | POST `/cloud/groups` creates; GET `/cloud/groups/{gid}` lists members; DELETE removes; DELETE admin → 400              | Integration test       |
| 10 | Playwright e2e `admin_ocs.spec.ts` green                                                                               | CI                     |
| 11 | `-D warnings` clippy/clean for `crabcloud-users` and `crabcloud-http`                                                  | CI                     |
| 12 | `git grep -i rustcloud` empty                                                                                          | CI                     |

## 11. Deferred / open questions

- **Sub-admins.** `/users/{uid}/subadmins` + the per-group admin permission model. Adds ~200 LOC of permission checks across every endpoint. Defer to a follow-up sub-project; the `AdminUser` extractor stays simple in 2-admin.
- **Quota management.** `PUT /cloud/users/{uid}` with `key=quota`. Needs a quota subsystem (storage layer). Tracked into the files sub-project.
- **Email verification.** Nextcloud sends a confirmation email on `PUT email`. We just write the column; needs the mail sub-project to add verification.
- **Rate-limiting.** Cross-cutting; same as 2b. A future middleware (token-bucket per-admin) covers admin OCS + getapppassword + create_app_password together.
- **LDAP / SAML interaction.** Once external user-store backends land (2e / 2f), admin OCS will need a `UserStore::is_writable_via_ocs(uid)` predicate so external-origin users can't be edited via OCS (typically they're read-only).
- **DB-backed audit log.** We emit `tracing` events; persisting them to `oc_admin_audit` is a follow-up.
- **AuthLayer re-check of `user.enabled`.** Documented gap (§6.6); the disable cascade closes it for known callers but doesn't catch the case where an admin marks a user disabled and an in-flight request with a still-valid token races. The fix is a post-lookup `user.enabled` check in `AuthLayer`, which requires a second query per auth. Defer; revisit alongside 2c.
- **`PUT /cloud/users/{uid}` extensibility.** Nextcloud's `key` accepts more than `{password, displayname, email}` (e.g., `additional_mail`, `phone`, `address`, `website`, `twitter`). We ship the three core keys and reject the rest with 400. Adding more is purely additive — track per request.
- **Cross-batch ergonomics.** The shared OCS-envelope helpers (`ocs_ok`, `users_err`, `not_found`, `bad_request`) are inlined in `admin_users.rs` and `admin_groups.rs`. If a fourth admin-OCS file ever lands, hoist into `routes/ocs/util.rs`.
