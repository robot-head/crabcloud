# Crabcloud — Platform Core Design

**Status:** Draft for review
**Date:** 2026-05-10
**Sub-project:** 1 of N (Platform Core)
**Parent program:** Port Nextcloud server to Rust, with a Dioxus frontend.

---

## 1. Program context

Crabcloud is a Rust port of [nextcloud/server](https://github.com/nextcloud/server) targeting full feature parity over multiple sub-projects. Nextcloud is ~500k+ lines of PHP across the core platform, hundreds of bundled apps (Files, Calendar, Contacts, Talk, Mail, etc.), a Vue.js frontend, a plugin/app framework, and multi-database support. Porting it is a program, not a project.

This document specs **only the first sub-project: platform core** — the substrate every later sub-project will build on. It deliberately stops short of users, storage, WebDAV, sharing, and the full app/plugin framework. Each of those gets its own spec.

### Compatibility commitment

Crabcloud is **wire + storage + DB compatible** with upstream Nextcloud:

- Existing Nextcloud desktop, iOS, and Android sync clients work unchanged against a Crabcloud server.
- URLs match upstream: `/remote.php/dav/...`, `/ocs/v2.php/...`, `/status.php`, `/index.php/login`.
- OCS API response envelopes match upstream (XML + JSON forms, statuscode conventions, OCS-APIRequest header handling).
- DB schema mirrors upstream's `oc_*` tables with the configurable `oc_` prefix; a Crabcloud server can be pointed at an existing Nextcloud's MySQL/Postgres/SQLite database.
- Capabilities endpoint matches upstream so client feature detection works.
- Config file format is modernized (TOML in place of PHP arrays), but config keys and semantics map 1:1 to Nextcloud's `config.php`.

### Roadmap of sub-projects (for context only)

The program decomposes roughly as: **(1) platform core (this spec)** → (2) users / auth / sessions → (3) full app/plugin framework → (4) storage abstraction → (5) WebDAV + Files API → (6) Files web UI → (7) sharing → (8) activity / notifications → (9) settings UI → (10) CalDAV + Calendar app → (11) CardDAV + Contacts app → (12+) Talk, Mail, Notes, Deck, Photos, Office integration, federation, E2EE, theming. Cross-cutting frontend framework decisions ride alongside.

---

## 2. Goals

- A single Rust binary that boots, runs migrations, and serves an HTTP surface that real Nextcloud sync clients can probe successfully.
- A workspace structured so later sub-projects plug in without churning core.
- Compile-time SQL safety against each of the three supported database dialects (SQLite, MySQL, Postgres).
- A Dioxus Fullstack UI surface coexisting with the Nextcloud-compatible client API surface in the same process.
- Honest, explicit boundaries: each crate has one purpose, well-defined interfaces, can be understood and tested independently.

## 3. Non-goals (out of this spec)

Each item below is its own future sub-project:

- Users, groups, password hashing, app passwords, 2FA, OAuth2 server, LDAP, SAML.
- Storage abstraction (virtual filesystem, local/S3/external storage adapters, encryption hooks).
- WebDAV, CalDAV, CardDAV protocol implementations.
- File sharing (internal, public links, federated).
- Full app/plugin framework (lifecycle hooks beyond `BootstrapHook`, dependency resolution, settings pages, navigation entries, app store).
- Background job runner / cron.
- Redis cache backend (trait defined; only memory backend implemented).
- Mail sending.
- UI beyond `/`, `/login`, and an error page (stubs only).
- Theming engine, installer, federation, Talk, Mail, Calendar, Contacts, Notes, Deck, Photos, Office, E2EE.
- E2E test infrastructure.
- Prometheus metrics endpoint (tracing logs only in this spec).

---

## 4. Architecture

### 4.1 Two HTTP surfaces, one process

The binary exposes two coexisting HTTP surfaces under one axum router:

- **Client API surface** — `/remote.php/*`, `/ocs/*`, `/status.php`, `/index.php/login`. JSON / XML / PROPFIND payloads. Authenticated via session cookie, Bearer token, Basic auth, or app password. Consumed by Nextcloud desktop / iOS / Android / CLI clients. No SSR.
- **Browser UI surface** — Dioxus Fullstack, mounted at `/` (catches everything the API surface does not match). SSR + hydration + server functions. Authenticated via session cookie, CSRF-protected.

Both surfaces share the same `AppState`: DB pool, cache, config snapshot, i18n catalogs, capability providers, tracing dispatcher.

### 4.2 Stack

- **Async runtime:** `tokio`.
- **HTTP:** `axum` + `tower` middleware.
- **Database:** `sqlx` with three concrete pool types (see §6).
- **Frontend:** `dioxus` (Fullstack mode) — SSR + WASM hydration + server functions.
- **Tracing:** `tracing` + `tracing-subscriber` with `EnvFilter` + JSON output when stdout is not a TTY.
- **Config:** `figment` (TOML + env-var overrides + CLI overrides + profile overlays).
- **Caching:** custom `Cache` trait with `MemoryCache` implementation in this spec.
- **i18n:** `polib` for gettext `.po` parsing.
- **Secrets:** `secrecy::SecretString` for sensitive config fields.

### 4.3 Cargo workspace layout

```
crabcloud/
├── Cargo.toml                  # workspace manifest
├── crates/
│   ├── crabcloud-config        # config loading (file + DB) + types
│   ├── crabcloud-db            # DbPool enum, db_dispatch! macro, migration runner
│   ├── crabcloud-cache         # Cache trait + MemoryCache impl
│   ├── crabcloud-i18n          # .po loader + locale resolver
│   ├── crabcloud-ocs           # OCS envelope + capabilities aggregator
│   ├── crabcloud-http          # axum router composition, middleware, extractors
│   ├── crabcloud-ui            # Dioxus Fullstack root, layout shell, server fns
│   ├── crabcloud-core          # facade re-exporting the above; defines AppState
│   └── crabcloud-server        # the binary; main(), bootstrap, signal handling
├── xtask/                      # cargo xtask build/dev/prepare/check-all
├── migrations/
│   └── core/                   # core's own migrations (apps add their own dirs later)
│       └── 0001_initial/{sqlite,mysql,postgres}.sql
├── l10n/
│   └── core/<locale>.po        # core's own translations (apps add their own dirs later)
├── dev/docker-compose.yml      # SQLite (file), MySQL, Postgres for local dev
└── docs/superpowers/specs/
```

**Rationale for the split:** the two crates that change most often (`-http`, `-ui`) recompile fast because their dependencies are stable. Each crate has one clear purpose. Future apps land as `crates/apps/<appid>/` and depend on `crabcloud-core`.

**Extension point for the deferred app framework:** `crabcloud-core` exposes a `BootstrapHook` registration vector. Today it's used only by core's own setup. When the app-framework sub-project lands, apps register via this same mechanism — no churn expected in the existing core crates; only the `-server` bootstrap and `crabcloud-core::AppState` grow.

---

## 5. Configuration

Two layers, mirroring Nextcloud.

### 5.1 File config — `config/config.toml`

Replaces `config/config.php`. Loaded once at startup; never reloaded mid-process (restart to change). Validated into a strongly-typed `FileConfig` struct on boot; startup fails fast on missing or invalid required keys.

Keys mirror Nextcloud 1:1:

- `dbtype`, `dbhost`, `dbname`, `dbuser`, `dbpassword`, `dbtableprefix`
- `datadirectory`
- `trusted_domains`, `trusted_proxies`
- `overwrite.cli.url`, `overwrite.protocol`, `overwrite.host`
- `loglevel`, `logfile`
- `secret`, `passwordsalt` (`SecretString`)
- `installed`, `version`, `instanceid`
- `default_language`
- Crabcloud-specific: `bind_address`, `db_pool_max`, `cache.backend`

Layered loading order (each overrides the previous):

1. `config/config.toml`
2. `config/config.local.toml` (gitignored; for dev secrets — figment profile overlay)
3. `CRABCLOUD_*` environment variables (container-friendly)
4. CLI overrides (`--config-set key=value`)

### 5.2 Runtime config — `oc_appconfig` table

Schema-compatible with Nextcloud: `(appid, configkey, configvalue)`. Mutable at runtime via admin UI. Accessed through `AppConfig` service backed by the cache (write-through, invalidated on update). Used for feature flags, app-specific settings, and the `enabled-apps` list.

### 5.3 Access pattern

`AppState` carries an `Arc<FileConfig>` (immutable snapshot) and an `AppConfig` handle (cache-backed, hot):

```rust
state.config().db_type()                                  // file value
state.appconfig().get("files", "max_upload_size").await   // runtime value
```

No global statics, no `lazy_static`. Everything flows through `AppState` so tests can inject.

### 5.4 Secrets handling

`secret`, `passwordsalt`, `dbpassword` use `secrecy::SecretString`. Debug printing is masked. Generated on first install by the future installer sub-project; never logged.

---

## 6. Data layer

### 6.1 Driver strategy — compile-time safety per dialect

`DbPool` is an enum over three concrete sqlx pools:

```rust
pub enum DbPool {
    Sqlite(sqlx::SqlitePool),
    MySql(sqlx::MySqlPool),
    Postgres(sqlx::PgPool),
}
```

Constructed once at boot from `config.dbtype`. Every repository method dispatches via `match`, with each arm using `sqlx::query!` / `sqlx::query_as!` against the concrete pool type. The macro validates SQL, parameter types, and output columns at compile time against the actual dialect.

Per-query pattern:

```rust
impl UserRepository {
    pub async fn get(&self, uid: &str) -> Result<Option<User>> {
        Ok(match self.pool {
            DbPool::Sqlite(ref p)   => sqlx::query_as!(User, "SELECT uid, displayname FROM oc_users WHERE uid = ?",  uid).fetch_optional(p).await?,
            DbPool::MySql(ref p)    => sqlx::query_as!(User, "SELECT uid, displayname FROM oc_users WHERE uid = ?",  uid).fetch_optional(p).await?,
            DbPool::Postgres(ref p) => sqlx::query_as!(User, "SELECT uid, displayname FROM oc_users WHERE uid = $1", uid).fetch_optional(p).await?,
        })
    }
}
```

### 6.2 The `db_dispatch!` macro

To keep repository code readable, `crabcloud-db` exposes a `macro_rules!` helper:

```rust
db_dispatch!(self.pool, User, fetch_optional, [uid],
    sqlite:   "SELECT uid, displayname FROM oc_users WHERE uid = ?",
    mysql:    "SELECT uid, displayname FROM oc_users WHERE uid = ?",
    postgres: "SELECT uid, displayname FROM oc_users WHERE uid = $1",
)
```

Expands to the three-arm match above. Keeps each query's three dialect forms visible side-by-side. The macro is the single tool that all repositories use; no other dispatch idiom is sanctioned.

### 6.3 Pool

One enum-wrapped pool per process. Size from `config.db_pool_max` (default 16). Held in `AppState`. No connection-per-request: extractors borrow from the pool on demand.

### 6.4 Migrations

`crabcloud-db::MigrationRunner` scans registered migration sources. Each source is a `(namespace, &'static [Migration])` tuple — e.g. `("core", CORE_MIGRATIONS)`. Apps register their own namespaces via the same mechanism when the app framework lands.

A single `oc_migrations(namespace TEXT, version INT, applied_at TIMESTAMP, PRIMARY KEY (namespace, version))` table tracks applied versions deterministically per namespace.

Migration files are dialect-split: `migrations/core/0001_initial/{sqlite,mysql,postgres}.sql`. The runner picks the file matching the active driver. Triple the SQL surface is the accepted cost of multi-DB parity.

### 6.5 Schema compatibility

Core migrations produce Nextcloud-shaped `oc_users`, `oc_groups`, `oc_appconfig`, `oc_migrations`, `oc_jobs` (stub), `oc_authtoken`, `oc_preferences` tables with the configurable `dbtableprefix` (default `oc_`). Indexes match upstream. Storage / file / share tables land with their respective sub-projects.

### 6.6 Repository layout

Repositories live in the crate that owns the concern — `AppConfigRepository` in `crabcloud-config`, future `UserRepository` in `crabcloud-users`. `crabcloud-db` only provides the pool enum, `db_dispatch!`, migration runner, and shared dialect helpers (identifier quoting, timestamp formatting).

### 6.7 Transactions

`DbTransaction` enum mirrors `DbPool`:

```rust
pub enum DbTransaction<'c> {
    Sqlite(sqlx::Transaction<'c, sqlx::Sqlite>),
    MySql(sqlx::Transaction<'c, sqlx::MySql>),
    Postgres(sqlx::Transaction<'c, sqlx::Postgres>),
}
```

Repository methods that need to participate in a transaction take `&mut DbTransaction`. Multi-step service operations open a `DbTransaction` from the pool, pass it through, and `commit()` (or drop for rollback) at the end.

### 6.8 Operational cost of compile-time multi-dialect safety

- The `query!` macros need a live DB or an offline cache (`.sqlx/`) at compile time. With three dialects per query, the cache must contain entries from all three.
- CI runs `cargo sqlx prepare` against a live SQLite, MySQL, and Postgres in turn (via `dev/docker-compose.yml`), all writing into the shared `.sqlx/` cache. The cache is committed to the repo.
- Contributors run `cargo xtask prepare` to regenerate the cache after adding or modifying a query. The xtask command starts the docker-compose stack and runs prepare against each dialect.
- CI matrix runs `cargo xtask check-all` against all three backends.

This cost is accepted in exchange for true compile-time SQL safety on each dialect.

---

## 7. HTTP layer

### 7.1 Router composition

`crabcloud-http::build_router(state: AppState) -> axum::Router` composes nested routers:

```rust
Router::new()
    .nest("/ocs",          ocs_router())                 // /ocs/v1.php/*, /ocs/v2.php/*
    .nest("/remote.php",   remote_php_router())          // /remote.php/dav/* — stub here
    .route("/status.php",  get(status::handler))
    .route("/index.php/login", post(login::api))         // legacy URL clients use
    .merge(ui_router())                                  // Dioxus Fullstack — fall-through
    .layer(global_middleware_stack())
    .with_state(state)
```

In this spec, `ocs_router()` ships only the routes required for client probing (`/ocs/v1.php/cloud/capabilities`, `/ocs/v2.php/cloud/capabilities`). `remote_php_router()` is a stub returning 501 for everything except a trivial probe. Real route population lands in later sub-projects.

### 7.2 Middleware stack

Applied via `tower::ServiceBuilder`, outermost to innermost:

1. **`TraceLayer`** — per-request `tracing` span with request ID.
2. **`CatchPanic`** — turns panics into 500s without killing the process.
3. **`RequestBodyLimit`** — 512 MiB default; WebDAV PUT will override per-route later.
4. **`ProxyHeadersLayer`** (custom) — honors `X-Forwarded-{Proto,Host,For}` only from `config.trusted_proxies`; rewrites the request's effective scheme/host. Rust analog of Nextcloud's `overwriteprotocol` / `overwritehost`.
5. **`TrustedDomainLayer`** (custom) — rejects with 400 if effective `Host` isn't in `config.trusted_domains`. Skipped for CLI/loopback.
6. **`SecurityHeadersLayer`** (custom) — `Strict-Transport-Security`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`, `X-Frame-Options: SAMEORIGIN`, and a Content-Security-Policy. CSP differs between API responses (restrictive) and UI responses (allows the WASM bundle origin).
7. **`CorsLayer`** — disabled same-origin; allow-list configurable for dev.
8. **`SessionLayer`** (custom) — per-router below this point.
9. **`CsrfLayer`** (custom) — per-router below this point.

Session and CSRF layers attach to the UI and OCS sub-routers separately because the two surfaces have different auth + CSRF rules.

### 7.3 Session

- Cookie name: `oc_sessionPassphrase` (Nextcloud-compatible so reverse-proxy stickiness rules keep working).
- Cookie value: a signed opaque session ID, not the session contents.
- Server-side session data lives in `crabcloud-cache`, keyed by the session ID. Memory backend for single-node dev; Redis (future) for multi-node.
- Session value: `user_id`, `login_credentials_hash`, `last_activity`, `lockout_until`, plus a scratchpad for flash messages / OAuth state.
- TTL: 30 min idle, 24 h absolute (both from config). Sliding window on each authenticated request.
- Cookie attributes: `HttpOnly`, `Secure` (when effective scheme is https), `SameSite=Lax`, `Path=/`.

### 7.4 CSRF

Matches Nextcloud's request-token scheme:

- Each session gets a request token (rotated when the user logs in / out). Exposed to the frontend via a `<head>` meta tag at SSR time and via the hydration payload.
- Authenticated browser requests must carry the token in a `requesttoken` header or form field.
- Token-authenticated API requests (Bearer / Basic / app-password) skip CSRF.
- Requests carrying `OCS-APIRequest: true` on the OCS surface bypass CSRF — matching upstream behavior. All Nextcloud clients send this header. This is documented and intentional.

### 7.5 Auth extractors

Two axum extractors:

- `AuthenticatedUser` — 401 if no valid auth. Yields `UserId` and `auth_method`.
- `OptionalUser` — returns `Option<AuthenticatedUser>`.

In this spec, both resolve only against a `bootstrap_admin` config key (username + bcrypt hash). The real user-store integration lands in the users sub-project; the extractor interface is final, only the resolution logic is stubbed.

### 7.6 Error → response mapping

`crabcloud-core::Error`:

```rust
pub enum Error {
    NotFound,
    Unauthorized,
    Forbidden,
    BadRequest(String),
    Conflict(String),
    Locked,                       // WebDAV 423 (future)
    Internal(anyhow::Error),
    OcsError { code: u16, message: String },
}
```

Three response wrappers each implementing `IntoResponse`:

- `ApiError(Error)` — plain HTTP status + JSON `{"error":"..."}`. Used by non-OCS handlers.
- `OcsError(Error)` — wraps in the OCS XML/JSON envelope with the correct `<statuscode>`.
- `DavError(Error)` — WebDAV-shaped response (stub here; populated in the WebDAV sub-project).

`Internal` errors are logged with full chain + backtrace at `error` level; the response body is generic and never leaks internals.

### 7.7 `/status.php`

Returns the exact JSON shape Nextcloud emits, so existing client probes pass:

```json
{
  "installed": true,
  "maintenance": false,
  "needsDbUpgrade": false,
  "version": "31.0.0.0",
  "versionstring": "31.0.0",
  "edition": "",
  "productname": "Nextcloud",
  "extendedSupport": false
}
```

`version` / `versionstring` / `productname` are configurable so clients see a Nextcloud-compatible identity. Operators see the Crabcloud version via `crabcloud-server --version` (see §9.5).

---

## 8. Dioxus Fullstack integration

### 8.1 Mounting

`crabcloud-ui::ui_router() -> axum::Router<AppState>` returns a Dioxus Fullstack-configured sub-router. Dioxus's axum integration mounts:

- The SSR handler for each route declared in the `crabcloud-ui::app::App` component's `Router`.
- The server-function endpoint (`/api/_dx/*`).
- The static asset handler for the hydrated WASM bundle + CSS + fonts (`/assets/*`).

In this spec, the Dioxus router declares `/`, `/login`, and an error page. Other UI routes (`/files/*`, `/settings/*`, `/s/<token>`, …) are stubbed for later sub-projects.

`ui_router()` is `.merge`d **last** in §7.1's composition so API-surface routes win on conflicts. The UI router is the fall-through.

### 8.2 Per-request context

A tower middleware on the UI sub-router builds a `RequestContext` for every request:

```rust
pub struct RequestContext {
    pub user: Option<AuthenticatedUser>,
    pub locale: Locale,
    pub state: AppState,
    pub request_token: RequestToken,
}
```

Resolution order:

1. `OptionalUser` from the session.
2. Locale from user preference → `Accept-Language` → `config.default_language` → `en`.
3. Request token from the current session (rotated on login/logout).

The middleware inserts `RequestContext` as an axum `Extension`. SSR components and server functions retrieve it via `use_server_context::<RequestContext>()`.

### 8.3 Hydration handoff

The SSR root renders a `<script id="__dx_ctx" type="application/json">{...}</script>` tag with `{ user_id, display_name, locale, request_token, server_capabilities_etag }`. The WASM client reads this on mount and seeds its global state. No extra round-trip.

The CSRF request token is exposed both via the hydration payload and via a `<meta name="requesttoken">` tag (matching Nextcloud's convention) so non-Dioxus inline scripts can find it.

### 8.4 Server functions vs. OCS API

Both RPC mechanisms coexist:

- **`#[server]` functions** for browser-only RPC. Ergonomic, type-safe end-to-end, auto-generated client stubs. Use for personal-settings toggles, dashboard widgets, anything not consumed by external clients.
- **OCS API** for anything that must also be callable by desktop / mobile / CLI clients. Hand-written handlers; documented as part of the Nextcloud-compat surface. The frontend calls these via `fetch` like any other client, not as server functions.

**Rule:** if it's reachable by a non-browser client, it's OCS; if it's browser-only sugar, it's a server function.

The frontend ships an OCS client module and consumes the auto-generated server-function client.

### 8.5 Login flow

`/login` is a Dioxus SSR'd page (works without JS for accessibility + before WASM hydrates). Form POSTs to `/index.php/login` — the legacy URL Nextcloud clients also use. The handler validates credentials (stub against `bootstrap_admin` in this spec), opens a session, sets the cookie, redirects to `/`. After hydration, the page upgrades to a Dioxus-driven form with inline validation.

### 8.6 Public share landing

`/s/<token>` is a Dioxus SSR'd route. SSR pays off here for unauthenticated previews + OpenGraph tags. Stub-only in this spec.

### 8.7 Asset pipeline

`dx build --release` produces the WASM bundle + assets under `target/dx/crabcloud-ui/public/`. The `crabcloud-server` binary serves these either embedded (release builds via `rust-embed`) or from disk (debug / `--no-embed`). `cargo xtask build` orchestrates: UI assets first, then the server binary.

### 8.8 Dev experience

`cargo xtask dev` runs `dx serve` (hot-reload UI) on one port and `cargo run -p crabcloud-server -- --no-embed --ui-assets ../target/dx/crabcloud-ui/public` on another, with an axum stitching reverse-proxy on `:8080` exposing both surfaces under a single port.

---

## 9. Cross-cutting concerns

### 9.1 Cache — `crabcloud-cache`

Trait:

```rust
#[async_trait]
pub trait Cache: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn set(&self, key: &str, value: &[u8], ttl: Option<Duration>) -> Result<()>;
    async fn del(&self, key: &str) -> Result<()>;
    async fn incr(&self, key: &str, by: i64) -> Result<i64>;
    async fn cas(&self, key: &str, old: &[u8], new: &[u8]) -> Result<bool>;
}
```

Bytes in / bytes out — callers control serialization. A `TypedCache<T>` wrapper does serde + key prefixing for typed call sites.

**Implementations:**

- `MemoryCache` — `Arc<RwLock<HashMap<String, Entry>>>` + background TTL sweeper. Shipped in this spec.
- `RedisCache` — designed for, not implemented here. Lands in its own micro-sub-project before multi-node deploy.

**Namespacing.** All keys prefixed with `config.instanceid` to prevent cross-instance contamination on shared Redis.

**Usage in this spec:** sessions, `oc_appconfig` lookups, capabilities ETag, i18n catalogs (read-through). File metadata caching lands with the storage sub-project.

### 9.2 i18n — `crabcloud-i18n`

- **Format:** Gettext `.po`, one per `(app, locale)`: `l10n/<appid>/<locale>.po`. Core ships `l10n/core/<locale>.po`.
- **Loader:** scan `l10n/` on startup, parse via `polib` (pure Rust). Store as `HashMap<(Cow<str>, Locale), Catalog>` in `AppState`.
- **Resolution per request** (UI surface only): user preference (stub — None in this spec) → `Accept-Language` → `config.default_language` → `en`. Resolved `Locale` lands on `RequestContext`.
- **API:** `i18n.t("core", "Welcome, %s", &[username])`. Plural: `i18n.tn("files", "%d file", "%d files", count, &[count])`. Missing translations fall back to the source string.
- **Hot reload:** not supported in this spec. Restart to pick up new translations.

### 9.3 OCS envelope + capabilities — `crabcloud-ocs`

**Envelope.** All OCS responses follow upstream's wire format. XML by default; JSON when `Accept: application/json` or `?format=json`:

```xml
<?xml version="1.0"?>
<ocs>
  <meta><status>ok</status><statuscode>200</statuscode><message>OK</message></meta>
  <data>...payload...</data>
</ocs>
```

```json
{"ocs":{"meta":{"status":"ok","statuscode":200,"message":"OK"},"data":{...}}}
```

A generic `OcsResponse<T: Serialize>` implements `IntoResponse` with content negotiation and emits the envelope. Errors go through `OcsError` (see §7.6) with the same envelope.

**Statuscode mapping** matches upstream precisely (100 = ok in v1, 200 = ok in v2, 996/997/998/999 = server / unauthenticated / not found / unknown). Clients depend on these numbers.

**Capabilities aggregator.** `/ocs/v2.php/cloud/capabilities` returns:

```json
{"ocs":{"data":{"version":{...},"capabilities":{"core":{...}, ...}}}}
```

`CapabilityProvider` trait:

```rust
pub trait CapabilityProvider: Send + Sync {
    fn namespace(&self) -> &'static str;
    fn contribute(&self, ctx: &RequestContext) -> serde_json::Value;
}
```

`AppState` carries `Vec<Arc<dyn CapabilityProvider>>`. Core registers a `CoreCapabilities` provider for the `core` namespace (poll interval, webdav-root, mod-rewrite-working, etc., matching Nextcloud's keys). The handler iterates providers, merges their JSON under their namespaces, wraps in the envelope.

ETag derived from a stable hash of `(provider list, version, instanceid)`. Response cached for 60s in `crabcloud-cache` keyed on `(user_id, locale, etag_input_hash)`. Apps will register their own `CapabilityProvider`s via `BootstrapHook` when the app framework lands — no churn to the aggregator.

---

## 10. Bootstrap & lifecycle

### 10.1 Process startup

`crabcloud-server` `main()`:

1. Parse CLI args (`clap`): `--config <path>`, `--ui-assets <path>`, `--no-embed`; subcommands `serve` (default), `migrate`, `version`.
2. Load `FileConfig` (figment: TOML + env vars + CLI overrides). Validate. Fail fast on error.
3. Initialize `tracing` subscriber: fmt layer + `EnvFilter` (`RUST_LOG` / `config.loglevel`). JSON output when stdout is not a TTY.
4. Connect `DbPool` — enum-dispatched to one of three sqlx pools.
5. Run `MigrationRunner` against the `core` namespace. Refuse to serve if `config.installed = false`.
6. Build `Cache` — `MemoryCache` by default; `cache.backend = "redis"` → future `RedisCache`.
7. Load i18n catalogs from `l10n/`.
8. Build `CapabilityProvider` list — `CoreCapabilities` only in this spec.
9. Construct `AppState { config, pool, cache, i18n, capability_providers, instance_id }`.
10. Run registered `BootstrapHook`s (empty in this spec; populated by apps later).
11. Build axum router via `crabcloud-http::build_router(state)`.
12. Bind listener from `config.bind_address` (default `127.0.0.1:8080`).
13. Spawn signal handler (SIGTERM/SIGINT → graceful shutdown; Ctrl-C handler on Windows).
14. `axum::serve(...).with_graceful_shutdown(...).await`.

### 10.2 Subcommands

- `crabcloud-server migrate` — run migrations and exit. Container init / ops use this.
- `crabcloud-server version` — print Crabcloud version + git SHA + active dialect support.

### 10.3 Graceful shutdown

SIGTERM → stop accepting new connections, drain in-flight requests with a configurable timeout (default 30 s), close the DB pool, flush tracing, exit. The Dioxus Fullstack server hooks the same shutdown signal.

### 10.4 Embedded vs. on-disk assets

Release builds embed the WASM bundle + CSS + fonts into the binary via `rust-embed` so deployment is "drop the binary + config + DB". Debug builds default to on-disk for fast iteration. `--no-embed` overrides at runtime.

### 10.5 Two-axis versioning

`crabcloud-server --version` reports:

- **Crabcloud version** — semver, from a workspace constant. Operators see this.
- **Reported-as-Nextcloud version** — from `config.version` / `config.versionstring`, defaulting to a current upstream Nextcloud value. Clients see this via `/status.php` and `/ocs/v2.php/cloud/capabilities`.

### 10.6 Docker image

Multi-stage Dockerfile: build stage runs `cargo xtask build`; runtime stage is `gcr.io/distroless/cc` with the static binary + embedded assets. Target image size under 80 MB compressed.

---

## 11. `cargo xtask` commands

A small `xtask` crate provides project commands without bespoke tooling:

- `cargo xtask prepare` — starts `dev/docker-compose.yml`, runs `cargo sqlx prepare` against SQLite + MySQL + Postgres in turn, writes a shared `.sqlx/` cache.
- `cargo xtask build` — `dx build --release` then `cargo build --release -p crabcloud-server`.
- `cargo xtask dev` — `dx serve` + `cargo watch -x 'run -p crabcloud-server'` + a stitching reverse-proxy on `:8080`.
- `cargo xtask check-all` — `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace`. CI's primary command.

---

## 12. Testing strategy

### 12.1 Unit tests

Co-located with code. Pure functions, formatting, error mapping, config parsing, envelope serialization, capability merging.

### 12.2 Integration tests

`tests/` directory. Two flavors:

1. **Single-dialect** — runs against `SQLITE_TEST_URL` by default. Exercises the HTTP layer end-to-end with `tower::ServiceExt::oneshot`. Verifies session, CSRF, middleware ordering, OCS envelope shape, `/status.php` response, Dioxus SSR mount points.
2. **Multi-dialect query tests** — same test bodies parameterized over the three pool variants. Catches dialect drift. SQLite runs in-process; MySQL + Postgres via `testcontainers-rs`. CI runs all three on every PR; locally behind a `--features integration` flag so contributors without Docker aren't blocked.

### 12.3 Property tests

`proptest` for: OCS envelope round-trip (any serializable payload survives serialize → parse), config parsing (any valid TOML produces a valid `FileConfig`), request-token CSRF check (random tokens reject; matching tokens accept).

### 12.4 Snapshot tests

`insta` for SSR'd HTML of `/`, `/login`, error pages. Catches accidental markup regressions during refactors. Snapshot updates are explicit (`cargo insta review`).

### 12.5 No mocks for DB or cache

Repository tests hit a real database. Cache tests hit the real `MemoryCache`. The class of bugs mock-vs-prod divergence introduces is designed out, not tested around. Tests needing TTL behavior use `tokio::time::pause()`.

### 12.6 Coverage target

80 % line on `crabcloud-core`, `crabcloud-http`, `crabcloud-ocs`. Lower bar on `crabcloud-ui` (Dioxus components covered by snapshot tests + the future E2E layer).

### 12.7 E2E

Out of this spec. Playwright/Cypress lands when there's a real UI to test.

---

## 13. Acceptance criteria

The platform-core sub-project is complete when **all** of the following hold:

1. `cargo xtask check-all` passes against all three database backends.
2. `cargo xtask build` produces a single static binary with embedded UI assets.
3. The binary boots against a fresh SQLite, MySQL, and Postgres database, runs core migrations, and serves traffic.
4. `curl /status.php` returns the Nextcloud-shaped JSON.
5. `curl /ocs/v2.php/cloud/capabilities -H 'OCS-APIRequest: true'` returns a valid OCS envelope with the `core` namespace populated.
6. A browser visiting `/` gets a Dioxus SSR'd page that hydrates and shows "logged in as bootstrap-admin" when authenticated against the bootstrap admin.
7. `/login` form POST against the bootstrap admin sets a session cookie and redirects to `/`.
8. Trusted-domain rejection, proxy-header rewriting, CSRF token enforcement, and security-headers presence are all verified by integration tests.
9. Both single-dialect and multi-dialect test suites are green in CI.

---

## 14. Open questions (deferred; not blockers)

- **Dioxus Fullstack version pinning.** Dioxus is pre-1.0; APIs shift. Pin to a specific minor and treat upgrades like infrastructure migrations.
- **WASM bundle size budget.** Target under 2 MB compressed for the initial shell. Real budget setting happens once we have non-trivial components.
- **Telemetry / metrics.** No Prometheus endpoint here; tracing structured logs only. Metrics layer is a future micro-sub-project.
- **Deep health endpoint.** `/status.php` is the Nextcloud-compat probe. A separate `/_health` (DB ping, cache ping, migration status) for Kubernetes probes lands later.
- **Config schema export.** No JSON Schema for `config.toml`. The Rust types are the schema; the future docs site can render them.

---

## 15. Glossary

- **OCS** — Open Collaboration Services. Nextcloud's RPC envelope format for client APIs.
- **WebDAV / CalDAV / CardDAV** — file, calendar, and contact sync protocols. WebDAV is Nextcloud's file API; CalDAV / CardDAV are deferred sub-projects.
- **`oc_*` tables** — Nextcloud's DB table naming convention; the `oc_` prefix is configurable via `dbtableprefix`.
- **App framework** — Nextcloud's plugin / extension system. Each "app" is a self-contained feature module (Files, Calendar, etc.). Deferred to its own sub-project; platform core only exposes a `BootstrapHook` registration vector now.
- **Capabilities** — feature flags clients query at startup to decide what UI to enable.
- **Dioxus Fullstack** — Dioxus's SSR + hydration + server-function mode (analogous to Next.js).
- **`BootstrapHook`** — a one-shot function called during server startup. The extension point apps will plug into when the app framework lands.
