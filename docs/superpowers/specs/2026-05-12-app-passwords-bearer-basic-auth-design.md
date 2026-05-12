# App Passwords + Bearer/Basic Auth Design (sub-project 2b)

**Status:** approved 2026-05-12.
**Parent:** sub-project 2a (`crabcloud-users` — UserStore / GroupStore / PreferenceStore / UsersService / BootstrapAdminBackend), shipped at master `1407d8c`.
**Predecessor decision:** Dioxus 0.7 fullstack landed (PR #21); the SSR + login flow already moved to `#[server]` fns in `crabcloud-ui::server_fns`. CI green on master.

## 1. Goal

Stand up the second half of authentication: a database-backed `oc_authtoken` store that serves both **long-lived app passwords** (used by DAV / desktop / mobile clients via Basic or Bearer auth) and **browser session tokens** (so every active device is listable and revocable from a single "Security" settings page). After 2b ships:

- Desktop / DAV clients can log in with `Authorization: Basic <base64(uid:apppassword)>` or `Authorization: Bearer <apppassword>`.
- Browser cookies stop being opaque cache-only session ids: they become tokens whose hash is the primary key on a row in `oc_authtoken`.
- Users can mint, list, and revoke app passwords from a `/settings/security` page and from a Nextcloud-client-compatible `/index.php/login/v2` flow.
- Changing the user's primary password marks every other token `password_invalid=1` (cascading invalidation).
- App-password-authenticated requests cannot reset the user's primary password (`PUT /ocs/v2.php/cloud/user key=password` → 403).

## 2. Scope

**In:**

- `oc_authtoken` table (full upstream schema, even columns 2b doesn't read/write).
- `AuthToken` types + `TokenStore` trait + `SqlTokenStore` (multi-dialect).
- `TokenAuthCache` (read-through `MemoryCache` wrapping the SQL store, with a 5s negative entry on miss).
- `AppPasswordService` facade: mint / list / revoke / verify.
- `AuthLayer` Axum middleware: cookie / Bearer / Basic in order; attaches `AuthContext` to request extensions.
- Existing extractors (`AuthenticatedUser`, `AdminUser`, `OptionalUser`) rewired to read `AuthContext` instead of `SessionHandle`.
- `POST /index.php/login` now mints an `AuthToken` (kind=Session) and sets the cookie to its raw token.
- `POST /index.php/login/v2` + `/index.php/login/v2/flow/<id>` + `POST /index.php/login/v2/poll` — Nextcloud-client bootstrap.
- `/ocs/v2.php/core/getapppassword` (GET) + `/ocs/v2.php/core/apppassword` (DELETE).
- Settings → Security page (Dioxus) with `list_app_passwords` / `create_app_password` / `revoke_app_password` `#[server]` fns.
- `PUT /ocs/v2.php/cloud/user key=password` returns 403 when `AuthContext.method != Session`.
- Password change cascade: `UPDATE oc_authtoken SET password_invalid=1 WHERE uid=?` for every other token.
- `crabcloud-server` CLI subcommands: `app-password-add <uid> <name>`, `app-password-list <uid>`, `app-password-revoke <id>`.
- Migration 0003.

**Out (tracked into later sub-projects):**

- OAuth2 server (`/apps/oauth2/api/v1/token`, RFC 6749 client registration) — 2d.
- 2FA framework — 2c. The `Session.two_factor_passed` flag from 2a already exists; 2b uses it as "always passed".
- Token scopes (filesystem-only, etc.) — schema column exists but always-null in 2b.
- E2E encryption key pairs in `oc_authtoken` — schema columns exist but always-null.
- Remote-wipe admin endpoints — schema column exists; auth-path already honours `remote_wipe=1`, but admin endpoints to *set* it ship later.
- Expired-token background sweep — rows linger until manual revoke.
- Migration of pre-2b sessions — existing cache-only sessions are invalidated on deploy; users re-log-in once.

## 3. Architecture

```
                          ┌────────────────────────────────┐
                          │ AuthLayer (axum middleware)    │
                          │                                │
   request ───────────────┤  1. Cookie? → hash, lookup     │
                          │  2. Bearer? → hash, lookup     │
                          │  3. Basic?  → hash pw, verify  │
                          │                  uid matches   │
                          │  AuthContext → req.extensions  │
                          └───────────┬────────────────────┘
                                      │
                                      ▼
                         existing extractors
                         (AuthenticatedUser / AdminUser /
                          OptionalUser) read AuthContext
                                      │
                                      ▼
                              handler runs

                          ┌────────────────────────────────┐
                          │  TokenAuthCache (Memory)       │  hot read-through
                          │       │                        │  TTL = min(30s, expires-now)
                          │       ▼                        │
                          │  TokenStore (trait)            │
                          │       │                        │
                          │       ▼                        │
                          │  SqlTokenStore                 │  oc_authtoken
                          └────────────────────────────────┘

                          ┌────────────────────────────────┐
                          │  AppPasswordService (facade)   │  mint / list / revoke / verify
                          │   composes TokenStore +        │
                          │   secret + bcrypt + rand       │
                          └────────────────────────────────┘
```

### 3.1 New files / modules

```
crates/
├── crabcloud-users/                                  # MODIFIED
│   └── src/
│       ├── auth_token.rs              (NEW)          # AuthToken, AuthTokenType, RawToken,
│       │                                             #   hash_token, token-name derivation
│       ├── store/
│       │   ├── auth_token.rs          (NEW)          # TokenStore trait + SqlTokenStore +
│       │   │                                         #   TokenAuthCache (memory read-through)
│       │   └── mod.rs                                # adds `pub mod auth_token`
│       ├── app_password.rs            (NEW)          # AppPasswordService façade
│       ├── service.rs                                # UsersService.app_passwords() helper
│       └── lib.rs                                    # re-exports
│
├── crabcloud-http/                                   # MODIFIED
│   └── src/
│       ├── middleware/
│       │   ├── auth.rs                (NEW)          # AuthLayer (cookie/Bearer/Basic → AuthContext)
│       │   └── csrf.rs                               # gate-on AuthMethod::Session
│       ├── extractors/auth.rs                        # AuthenticatedUser / OptionalUser /
│       │                                             #   AdminUser read AuthContext, not Session
│       ├── routes/ocs/
│       │   └── app_password.rs        (NEW)          # /ocs/v2.php/core/{getapppassword,apppassword}
│       └── session/layer.rs                          # shrinks to cookie sign/verify only
│
├── crabcloud-core/                                   # MODIFIED
│   └── src/state.rs                                  # AppState.tokens: TokenAuthCache;
│                                                     #   AppStateBuilder::with_tokens()
│
├── crabcloud-ui/                                     # MODIFIED
│   └── src/
│       ├── pages/
│       │   ├── mod.rs
│       │   ├── settings_security.rs   (NEW)          # Settings → Security page
│       │   └── login_v2_flow.rs       (NEW)          # GET /index.php/login/v2/flow/<id> SSR
│       └── server_fns.rs                             # +list/create/revoke_app_password,
│                                                     #  destroy_other_sessions,
│                                                     #  login_v2_start, login_v2_authorize,
│                                                     #  login_v2_poll
│
├── crabcloud-server/                                 # MODIFIED
│   └── src/{cli.rs, main.rs}                         # app-password-* CLI subcommands
│
└── migrations/core/0003_auth_tokens/                 # NEW
    ├── sqlite.sql
    ├── mysql.sql
    └── postgres.sql
```

### 3.2 Layer responsibilities (recap)

- **`SessionLayer`** (existing, simplified): signs/verifies the `oc_sessionPassphrase` cookie's HMAC envelope. No longer touches the cache.
- **`AuthLayer`** (new): reads the (already-verified) cookie body, or the `Authorization` header, performs the appropriate lookup against `TokenAuthCache`, builds an `AuthContext` on success, attaches it to `req.extensions`. On failure, attaches nothing — extractors decide whether that's a 401 (`AuthenticatedUser`) or fine (`OptionalUser`).
- **`CsrfLayer`** (existing, gated): only enforces the CSRF token check when `AuthContext.method == Session`. Bearer/Basic skip CSRF — matching upstream Nextcloud (cross-site requests can't read other-origin headers, so CSRF is not a vector).

## 4. Data model

### 4.1 `oc_authtoken` schema (full upstream column set)

| Column            | Type (canonical)     | Nullable | Notes                                                              |
|-------------------|----------------------|----------|--------------------------------------------------------------------|
| `id`              | BIGINT auto-incr PK  | no       | Row identifier; used for revoke endpoints + `AuthContext.token_id` |
| `uid`             | VARCHAR(64)          | no       | Owning user                                                        |
| `login_name`      | VARCHAR(64)          | no       | What the user typed at login (uid or email)                        |
| `password`        | LONGTEXT / TEXT      | yes      | Encrypted primary password — for remote-wipe / push (unused in 2b) |
| `name`            | VARCHAR(128)         | no       | Human label: "Firefox on Linux", "DAV client"                      |
| `token`           | VARCHAR(200)         | no       | Hash of the raw token (lowercase hex of SHA-512). UNIQUE.          |
| `type`            | SMALLINT             | no       | `AuthTokenType` discriminator: 0=Session, 1=AppPassword            |
| `remember`        | SMALLINT             | no       | 0/1; sessions only; controls cookie max-age                        |
| `last_activity`   | BIGINT (unix secs)   | no       | Bumped on auth hits (rate-limited to one write per 30s)            |
| `last_check`      | BIGINT               | no       | Used by remote-wipe / future cleanup; bumped alongside activity    |
| `public_key`      | LONGTEXT             | yes      | E2E encryption (unused in 2b)                                      |
| `private_key`     | LONGTEXT             | yes      | E2E encryption (unused in 2b)                                      |
| `version`         | SMALLINT             | no       | Row-schema version; default `2` (matches upstream's current rev)   |
| `scope`           | LONGTEXT             | yes      | JSON-encoded scope object (unused in 2b)                           |
| `expires`         | BIGINT (unix secs)   | yes      | NULL = never                                                       |
| `password_invalid`| SMALLINT             | no       | 1 → auth path treats row as expired (401)                          |
| `remote_wipe`     | SMALLINT             | no       | 1 → auth path treats row as expired (401)                          |

Indexes:
- `UNIQUE (token)` — primary lookup.
- `INDEX (uid, type)` — for listing + cascade revocation.
- `INDEX (last_activity)` — for future expired-row sweep.

Per-dialect: SQLite uses `INTEGER` for ints, Postgres uses `BIGINT` / `SMALLINT`, MySQL uses `BIGINT` / `TINYINT` / `LONGTEXT` as the canonical type column suggests. `text` columns are `TEXT` on SQLite/PG and `LONGTEXT` on MySQL.

### 4.2 Rust types

```rust
pub struct AuthToken {
    pub id: i64,
    pub uid: UserId,
    pub login_name: String,
    pub password: Option<String>,
    pub name: String,
    pub token: String,                 // hash, 128 hex chars
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

#[repr(i32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthTokenType {
    Session = 0,
    AppPassword = 1,
}

/// Constructed via [`RawToken::generate`]; wraps a 72-byte base62 string in
/// `Secret<String>`. Displayed via [`expose`] exactly once at mint time.
pub struct RawToken(secrecy::SecretString);
```

### 4.3 Token format

- **Raw token:** 72 random bytes from `rand::rngs::OsRng`, base62-encoded → ~97 ASCII chars (alphabet `[A-Za-z0-9]`, URL- and Basic-auth-safe, no padding ambiguity).
- **Hashed token (DB):** `lowercase_hex(sha512(raw_token.as_bytes() ++ config.secret.as_bytes()))`. Deterministic, unique-indexable, fast on every dialect. Stored as 128-char hex (`VARCHAR(200)` to leave headroom for upstream variations).
- **Cookie value:** the raw token, wrapped by the existing `crabcloud_http::session::cookie::encode_cookie` HMAC-SHA256 signer so a tampered cookie short-circuits before the DB lookup. Same cookie name (`oc_sessionPassphrase`), same TLS expectations.

### 4.4 `AuthContext` (request extension)

```rust
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: UserId,
    pub method: AuthMethod,            // Session | Bearer | Basic
    pub token_id: i64,                 // row PK; used for "revoke this session"
    pub login_name: String,            // for sessions; what the user typed
    pub remember: bool,                // sessions only
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthMethod {
    Session,
    Bearer,
    Basic,
}
```

The existing `AuthenticatedUser` shape stays:

```rust
pub struct AuthenticatedUser {
    pub user_id: String,
    pub auth_method: AuthMethod,
}
```

— it's just sourced from the request extension now.

## 5. Wire format & HTTP surface

### 5.1 Auth precedence on every request

`AuthLayer` walks these arms top-down; first hit wins. The header arms (Bearer, Basic) **fail loud** — a present-but-invalid header short-circuits the layer with 401 (no fall-through). The cookie arm **fails quiet** — a malformed cookie (bad HMAC) or one whose hash misses the DB is treated as if the cookie were absent, so the request becomes anonymous and `OptionalUser` continues to work for unauthenticated pages (`/login`, `/`, public OCS). This split matches user expectations: a stale browser cookie from a pre-secret-rotation deploy shouldn't lock the user out of the login form.

1. **`Authorization: Bearer <token>`** → hash, lookup, accept any `kind`. Present-but-unknown ⇒ 401.
2. **`Authorization: Basic <b64>`** → decode `uid:token`, hash the `token` portion, lookup, *also* verify the row's `uid` matches the decoded `uid` (constant-time string compare). Present-but-unknown / uid-mismatch ⇒ 401.
3. **`Cookie: oc_sessionPassphrase=<signed>`** → HMAC-verify, hash the raw token portion, lookup, accept only `kind=Session`. Bad HMAC, unknown hash, or wrong-kind ⇒ no `AuthContext` attached; request continues anonymously.

A row that's `expires < now`, `password_invalid=1`, or `remote_wipe=1` is treated by all three arms as a miss → same behaviour as "unknown" for that arm (401 for Bearer/Basic, anonymous for cookie). Every dropped-auth path emits a `tracing::warn!` carrying the dropped-reason for operator debugging. The raw token is never logged; only the row id (when known) and first 8 hex chars of its hash.

### 5.2 Endpoint catalog

#### `POST /index.php/login`

Existing endpoint, behavior change: after `users.verify(...)` succeeds the handler now also mints an `AuthToken` row (kind=Session), name derived from the `User-Agent` (e.g., `"Firefox 132 on macOS"`), `remember=` the form's checkbox, `last_activity=now`. The cookie value becomes that token's raw value (HMAC-signed via the existing cookie encoder).

#### `POST /index.php/login/v2`

Implemented as the `login_v2_start` `#[server]` fn (Dioxus fullstack), matching the existing `/index.php/login` and `/status.php` pattern. Body: none. Returns:

```json
{
  "poll":  { "token": "<poll-id>", "endpoint": "<base>/index.php/login/v2/poll" },
  "login": "<base>/index.php/login/v2/flow/<flow-id>"
}
```

`poll-id` and `flow-id` are unrelated 32-byte random ids (cached separately). The flow record lives in `MemoryCache` under `login_v2:flow:<flow-id>` and `login_v2:poll:<poll-id>`, 20-minute TTL.

#### `GET /index.php/login/v2/flow/<flow-id>`

Dioxus page (`pages/login_v2_flow.rs`), Session-auth only. Asks the (cookie-authed) user "Authorize `<UA-derived-name>`?" If anonymous, redirect to `/login?return=<this-url>`. The page exposes a single submit button; on POST it calls the `login_v2_authorize(flow_id)` `#[server]` fn which:

1. Reads the AuthContext (cookie auth).
2. Mints an `AuthToken` kind=AppPassword, name=`<UA-derived-name>`, `remember=false`, `expires=NULL`.
3. Writes the freshly-minted `raw_token` + `uid` into the flow's cache record.
4. Returns success; the page then renders a "you can close this tab" confirmation.

#### `POST /index.php/login/v2/poll`

Implemented as the `login_v2_poll` `#[server]` fn. Body: `{"token": "<poll-id>"}`. Behavior:

- Flow record absent or not yet authorized → **404 Not Found**, empty body.
- Flow record present + authorized → **200 OK** with:
  ```json
  {
    "server":      "<base>",
    "loginName":   "<uid>",
    "appPassword": "<raw_token>"
  }
  ```
  The flow record is *read-and-deleted* on first successful poll (single-use).

#### `GET /ocs/v2.php/core/getapppassword`

Cookie-only (returns 403 otherwise). For a cookie-authed user, mints a *bridge* `AuthToken` kind=AppPassword, name=`"Browser bridge"`, returns:

```json
{
  "ocs": {
    "meta": { "status": "ok", "statuscode": 200 },
    "data": { "apppassword": "<raw_token>" }
  }
}
```

The raw token is browser-readable so inline JS can use it for downstream DAV calls without smuggling the cookie. Matches Nextcloud's `getapppassword` semantics. Subsequent calls always mint a *new* bridge token (no idempotence).

#### `DELETE /ocs/v2.php/core/apppassword`

Any auth method. Revokes the calling request's own token row (`AuthContext.token_id`). 200 OK with an empty OCS envelope. If the row is already gone (race), still 200 — idempotent.

#### `PUT /ocs/v2.php/cloud/user` (existing endpoint update)

With `key=password`, returns **403 Forbidden** when `AuthContext.method != Session`. The cookie-auth path keeps the existing `currentpassword`-gated flow; after `set_password` succeeds, all other tokens are now `password_invalid=1` (see §6.2). The current row is preserved.

#### Settings UI server fns (Dioxus `#[server]`)

All require `AuthMethod::Session` (the `#[server]` fn checks and returns 403 otherwise — they're only meant to be called from the in-browser settings page).

```rust
#[server]
async fn list_app_passwords() -> Result<Vec<AuthTokenSummary>, ServerFnError>;

#[server]
async fn create_app_password(name: String) -> Result<CreatedAppPassword, ServerFnError>;

#[server]
async fn revoke_app_password(id: i64) -> Result<(), ServerFnError>;

#[server]
async fn destroy_other_sessions() -> Result<(), ServerFnError>;     // "Log out everywhere else"

pub struct AuthTokenSummary {
    pub id: i64,
    pub name: String,
    pub kind: AuthTokenType,
    pub last_activity: u64,
    pub current: bool,                  // is this the row backing the current cookie?
}

pub struct CreatedAppPassword {
    pub id: i64,
    pub name: String,
    pub raw_token: String,              // shown exactly once on the page
}
```

### 5.3 Settings → Security page

Route: `/settings/security`. Single Dioxus page rendering:

- A table of `AuthTokenSummary` rows: name, kind ("Browser session" vs "App password"), last_activity (relative), and a per-row Revoke button. The current cookie row is flagged + has a "Log out" instead of "Revoke" wording (UX nicety).
- A "Create new app password" form (name input + Submit). On Submit → `create_app_password(name)` → renders the returned `raw_token` in a copy-to-clipboard box with a one-shot dismissal. Never re-displayable.
- A "Log out everywhere else" button → calls a `destroy_other_sessions()` `#[server]` fn that runs `destroy_all_for_except(current_token_id)` over the user's token rows (and clears the cache mirror).

### 5.4 CLI subcommands

`crabcloud-server`:

- `app-password-add <uid> <name>` — mints an AppPassword token for `uid`, prints the plaintext exactly once. For operator-side provisioning (e.g., scripting an ingest job).
- `app-password-list <uid>` — prints a table of the user's tokens (id, name, kind, last_activity).
- `app-password-revoke <id>` — revokes by row id.

## 6. Security

### 6.1 Constant-time lookup + enumeration defense

- The DB lookup is a single hashed-equality query (`SELECT … WHERE token = ?`), index-backed, dialect-uniform. Lookup time is constant-ish regardless of hit/miss.
- On miss, `TokenAuthCache` records a 5-second negative entry keyed by `hash:{hex}` so a bruteforce flood is absorbed by RAM rather than the DB.
- For Basic auth, we **hash the password before comparing the uid** (the comparison is `row.uid == decoded_uid`, but the hash work happens unconditionally so a wrong-uid request takes the same wall-clock time as a wrong-password request).
- Failed auth responses are uniform 401 with `{"error":"Unauthorized"}` — no enumeration channel between "token unknown", "uid mismatch", "expired", "password_invalid", or "remote_wipe".

### 6.2 `password_invalid` cascade

`UsersService::set_password` is amended to:

```rust
pub async fn set_password(&self, uid: &UserId, new: &str) -> UsersResult<()> {
    let hash = self.verifier.hash(new)?;
    self.users.set_password(uid, &hash).await?;
    self.tokens.invalidate_all_for(uid).await?;     // sets password_invalid=1 on every row
    Ok(())
}
```

Combined with the existing `destroy_all_for_except` (which now also deletes other token rows), the password-change flow at `PUT /ocs/v2.php/cloud/user key=password` does:

1. Verify `currentpassword`.
2. `users.set_password(uid, new)` → rehashes + invalidates all rows.
3. `users.destroy_other_sessions(uid, current_token_id)` → deletes other token rows + cache mirror entries.

Result: the current request keeps its cookie alive (its row is the only survivor and its `password_invalid` is reset back to 0), every other browser session and app password is invalidated. Mirrors Nextcloud.

### 6.3 App-password capability boundary

Endpoints whose handlers check `AuthContext.method`:

| Endpoint                              | Allowed methods                  | Rationale                                  |
|---------------------------------------|----------------------------------|--------------------------------------------|
| `PUT /ocs/v2.php/cloud/user key=password` | `Session` only (returns 403)   | App pw can't escalate to primary-pw rotate |
| `GET /ocs/v2.php/core/getapppassword`   | `Session` only (returns 403)   | Bridge tokens only minted from a real login |
| `/settings/security` page + its server fns | `Session` only                | Settings is a browser UX                   |
| All other authenticated endpoints     | any                              | DAV / OCS / files all neutral              |

All other capability checks (admin-group membership for OCS user admin endpoints, etc.) remain orthogonal: they apply to any auth method.

### 6.4 CSRF gating

`CsrfLayer` (existing) only enforces the `requesttoken` header / `request_token` form field check when `AuthContext.method == Session`. Bearer/Basic skip it entirely. Matches upstream — CSRF tokens defend against cross-origin cookie smuggling; header-based auth isn't reachable from other origins.

### 6.5 Token lifecycle

- **Mint**: `RawToken::generate()` produces 72 fresh OSRng bytes; `hash_token(raw, secret)` derives the storage hash; row written with `last_activity = last_check = now()`, `password_invalid = remote_wipe = 0`, `version = 2`, `expires = NULL` (no expiry in 2b).
- **Use**: lookup by hash → row → bump `last_activity` if it's been ≥30s since the last bump (the rate-limit prevents one-write-per-request). Cache holds `(token_hash → AuthToken)` for `min(30s, expires - now)`.
- **Revoke**: `DELETE FROM oc_authtoken WHERE id = ?` + invalidate cache entry. Idempotent (delete-from-empty is fine).
- **Cascade**: `password_invalid` + cookie destruction on `set_password`. No `expires` sweep in 2b — rows linger until manual revoke or `password_invalid` cascade. Future cron job lands separately.

### 6.6 Auth path failure modes

| Failure                                  | HTTP                                          | Logged at                          |
|------------------------------------------|-----------------------------------------------|------------------------------------|
| No Cookie and no Authorization           | 401 (extractor) or anonymous (OptionalUser)   | —                                  |
| Bearer/Basic token unknown               | 401                                           | `warn` `auth_token_not_found`      |
| Basic uid ≠ row.uid                      | 401                                           | `warn` `auth_basic_uid_mismatch`   |
| Bearer/Basic row expired / invalid / wiped | 401                                         | `warn` `auth_token_unusable`       |
| Cookie HMAC bad                          | anonymous (request continues, cookie dropped) | `warn` `cookie_hmac_invalid`       |
| Cookie hash unknown                      | anonymous                                     | `warn` `cookie_unknown`            |
| Cookie row expired / invalid / wiped     | anonymous                                     | `warn` `cookie_unusable`           |
| Cookie token's kind ≠ Session            | anonymous                                     | `warn` `cookie_wrong_kind`         |
| DB / cache error                         | 500                                           | `error` `auth_backend_error`       |

All `warn` log lines include `token_id` (i64, never the raw value or full hash) so operators can correlate without leaking secrets.

## 7. Error model

Two new `UsersError` variants:

```rust
pub enum UsersError {
    // ... existing variants ...
    #[error("token not found")]
    TokenNotFound,
    #[error("token already revoked")]
    TokenAlreadyRevoked,
}
```

Mapping (in `crabcloud-core::Error::users_status`):

- `TokenNotFound` → 401 (same as `InvalidCredentials`).
- `TokenAlreadyRevoked` → 410 Gone.

No new variants on `crabcloud-core::Error` itself. The existing `Db / Cache / Internal` arms absorb infrastructure failures.

## 8. Implementation skeleton (illustrative, plan owns the details)

### 8.1 `auth_token.rs`

```rust
pub struct RawToken(SecretString);

impl RawToken {
    pub fn generate() -> Self { /* 72 OsRng bytes → base62 */ }
    pub fn expose(&self) -> &str { self.0.expose_secret() }
}

pub fn hash_token(raw: &str, secret: &str) -> String {
    let mut hasher = sha2::Sha512::new();
    hasher.update(raw.as_bytes());
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}
```

### 8.2 `store/auth_token.rs`

```rust
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn create(&self, row: &AuthToken) -> UsersResult<i64>;
    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>>;
    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>>;
    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>>;
    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()>;
    async fn revoke(&self, id: i64) -> UsersResult<()>;
    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()>;
    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()>;
    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()>;  // set password_invalid=1
}

pub struct SqlTokenStore { pool: DbPool }

pub struct TokenAuthCache {
    inner: Arc<dyn TokenStore>,
    cache: Arc<dyn Cache>,
    instance_id: String,
}
```

The cache key is `{instance_id}:tokens:hash:{hex}`; negative entries are an empty `Option<AuthToken>` (sentinel) with 5s TTL; positive entries have 30s TTL.

### 8.3 `app_password.rs`

```rust
pub struct AppPasswordService {
    tokens: Arc<TokenAuthCache>,
    secret: SecretString,
}

impl AppPasswordService {
    pub async fn mint(&self, uid: &UserId, name: &str, kind: AuthTokenType,
                     login_name: &str, remember: bool) -> UsersResult<(AuthToken, RawToken)>;
    pub async fn list(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>>;
    pub async fn revoke(&self, id: i64) -> UsersResult<()>;
    pub async fn revoke_other_sessions(&self, uid: &UserId, current: i64) -> UsersResult<()>;
    pub async fn verify(&self, raw: &str) -> UsersResult<AuthToken>;     // hashes + lookup
}
```

### 8.4 `middleware/auth.rs`

```rust
pub struct AuthLayer { /* holds AppState handle for AppState extension lookup */ }

impl<S> tower::Layer<S> for AuthLayer { /* ... */ }

impl<S> tower::Service<Request> for AuthMiddleware<S> {
    async fn call(&mut self, mut req: Request) -> Result<Response, Error> {
        if let Some(ctx) = try_bearer(&req).await? { req.extensions_mut().insert(ctx); }
        else if let Some(ctx) = try_basic(&req).await? { req.extensions_mut().insert(ctx); }
        else if let Some(ctx) = try_cookie(&req).await? { req.extensions_mut().insert(ctx); }
        self.inner.call(req).await
    }
}
```

`try_bearer / try_basic / try_cookie` each: extract the candidate token → hash → `state.tokens.verify(hash)` → build `AuthContext` on success, return `Ok(None)` on benign absence, surface 500 on backend errors only.

### 8.5 Extractor updates

```rust
impl<S> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = UnauthorizedRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ctx = parts.extensions.get::<AuthContext>().ok_or(UnauthorizedRejection)?;
        Ok(AuthenticatedUser {
            user_id: ctx.user_id.as_str().to_string(),
            auth_method: ctx.method,
        })
    }
}
```

`OptionalUser` is the same with `Option<&AuthContext>`. `AdminUser` composes `AuthenticatedUser` then runs `users.is_admin` as today.

## 9. Testing

### 9.1 Unit (`crabcloud-users`)

- `RawToken::generate` returns 72-byte base62, never logs via `Debug`.
- `hash_token` is deterministic and changes with `secret`.
- `AuthTokenType` round-trips through SQL.
- `SqlTokenStore` round-trip per dialect: `create → lookup_by_hash → bump_activity → revoke`.
- `SqlTokenStore::list_for_user`, `revoke_all_for_user_except`, `invalidate_all_for_user` correctness.
- `TokenAuthCache` negative-entry behaviour: a miss caches; subsequent miss within 5s is cache-served.
- `AppPasswordService::mint` returns a `RawToken` whose hash matches the stored row's `token`.
- `AppPasswordService::verify` succeeds, fails on wrong/expired/`password_invalid`/`remote_wipe`.
- `password_invalid` cascade: `set_password(uid)` flips every other row's `password_invalid` to 1; the cascading row's `verify` now returns `TokenNotFound`.

### 9.2 Integration (`crabcloud-http`)

- `AuthLayer` arms: cookie / Bearer / Basic / mixed (all combinations 401 or auth-as-expected).
- `AuthLayer` honours precedence (Bearer before Basic before Cookie).
- Basic with uid-mismatch returns 401.
- CSRF middleware lets Bearer through without a `requesttoken` header.
- CSRF middleware still blocks cookie-auth requests lacking `requesttoken` on non-GET.
- `PUT /ocs/v2.php/cloud/user key=password` returns 403 under Bearer / Basic.
- `PUT /ocs/v2.php/cloud/user key=password` under Session: still works; other tokens become `password_invalid=1`.
- `/login/v2` happy path: client posts start → polls (404) → user authorizes via flow page → client polls (200, gets token) → token authenticates a follow-up Bearer request.
- `/login/v2/poll` is single-use: a second poll for the same id returns 404.
- `getapppassword` mints a fresh AppPassword every call (no idempotence).
- `DELETE apppassword` revokes the calling token.

### 9.3 E2E (Playwright)

- Update `e2e/tests/hydration.spec.ts` to assert that login still produces a working cookie + the hydration marker still flips. Cookie change is invisible to the suite.
- New `e2e/tests/app_password.spec.ts`:
  - Login as `admin`. Open `/settings/security`. Mint a token named "Test". Assert the raw_token is shown once.
  - `curl -u admin:<token>` against `/ocs/v2.php/cloud/user` returns 200 (drive via Playwright's `request` context).
  - Revoke the token. `curl` returns 401.
  - "Log out everywhere else" survives the current session but invalidates a second-browser cookie.

### 9.4 CLI (assert_cmd)

- `app-password-add` mints a usable token (verified via in-process `AppPasswordService::verify`).
- `app-password-list` round-trips after `add`.
- `app-password-revoke` removes the row.

## 10. Acceptance criteria

| #  | Criterion                                                                                                                | Source of truth          |
|----|--------------------------------------------------------------------------------------------------------------------------|--------------------------|
| 1  | `cargo xtask check-all` clean against SQLite + MySQL + Postgres                                                          | CI                       |
| 2  | `crabcloud-server user-add alice && app-password-add alice "DAV"` returns a token; `curl -u alice:<token>` → 200 on `/ocs/v2.php/cloud/user` | CLI + integration test   |
| 3  | Same token via `Authorization: Bearer <token>` → 200                                                                     | Integration test         |
| 4  | Wrong-token Basic → 401; wrong-uid+right-token Basic → 401                                                               | Integration test         |
| 5  | `POST /index.php/login` still works; the cookie is now an `oc_authtoken` row's raw_token; second request reads the row   | Integration test         |
| 6  | `/login/v2` poll cycle: client receives a token after the user clicks Authorize                                          | Integration test         |
| 7  | Settings UI lists active tokens (cookie + app passwords); revoke + re-list works; "Log out everywhere else" preserves current | E2E test                 |
| 8  | `PUT /ocs/v2.php/cloud/user key=password` returns 403 when authenticated via Basic/Bearer                                | Integration test         |
| 9  | `PUT /ocs/v2.php/cloud/user key=password` under cookie: marks every other token row `password_invalid=1` AND destroys their cache | Integration test         |
| 10 | Playwright `hydration.spec.ts` still green                                                                               | CI                       |
| 11 | `[workspace.lints]` `-D warnings` clean for `crabcloud-users` and `crabcloud-http`                                       | CI                       |
| 12 | `git grep -i rustcloud` empty                                                                                            | CI                       |

## 11. Deferred / open questions

- **OAuth2** — sub-project 2d. Will add OAuth client registration + `/apps/oauth2/api/v1/token` endpoint; storage reuses `oc_authtoken` with `kind=Bearer-OAuth` (new variant) or `kind=AppPassword` with a `scope` JSON noting the OAuth client id. Decision deferred.
- **2FA** — sub-project 2c. The `Session.two_factor_passed` flag already exists; 2b sets it to `true` on every successful login. 2c will gate it behind a per-user 2FA-required check.
- **Token scopes** (filesystem-only, etc.) — schema column `scope` exists; in 2b it's always written as `NULL`. The auth path ignores it. A future sub-project introduces `enum TokenScope` + per-endpoint enforcement.
- **E2E encryption keys** — `public_key`/`private_key` columns exist; always-null in 2b. The E2E sub-project will populate them at mint time and surface a key-exchange protocol.
- **Remote-wipe initiator** — `remote_wipe` column exists; the auth path already honours it (forcing 401 → client de-provisions itself per the Nextcloud protocol). The endpoint to *trigger* a remote wipe (admin OCS) lands separately.
- **Expired-token sweep** — no background cron in 2b; rows linger until manual revoke. A future cleanup job runs periodically.
- **Pre-2b session migration** — cache-only sessions become invalid on first deploy after 2b. Documented in the changelog; users see one re-login.
- **`oc_authtoken.password` column** — Nextcloud uses this to store the user's password encrypted by the token (so a session can decrypt mount credentials, etc.). 2b leaves it `NULL`. A later mount-credentials sub-project will fill it.
- **`remember` cookie behaviour** — 2b reads the `remember` checkbox at login and stores it on the row, but the cookie still uses the existing `SESSION_IDLE_TTL`. Wiring up a longer `Max-Age` when `remember=true` is a small UX follow-up.
- **Secret rotation** — `hash_token` mixes in `config.secret`, so rotating the secret invalidates every stored row's lookup hash. 2b deliberately falls *quietly* to anonymous on a stale cookie (so users hit `/login` rather than 401), but Bearer/Basic clients holding pre-rotation tokens will start seeing 401. Operators rotating the secret should expect to re-distribute app passwords. A future sub-project can introduce a per-secret-version hash prefix so old rows can be re-hashed at first auth.
