# Platform Core — Phase 3: HTTP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `rustcloud-server serve` actually serve HTTP — with `/status.php`, `/ocs/v2.php/cloud/capabilities`, a working `/index.php/login` flow against a bootstrap admin, a session+CSRF stack backed by the Phase 2 cache, all guarded by trusted-domain / proxy-header / security-header / body-limit / tracing middleware, and verified by an end-to-end integration test suite.

**Architecture:** A new `rustcloud-http` crate that consumes `rustcloud-core::AppState` and produces an `axum::Router`. Middleware are Tower layers; session storage is cache-backed via `Arc<dyn Cache>`; the cookie carries a signed opaque session ID (HMAC-SHA256 keyed by `config.secret`). Route handlers turn `rustcloud-core::Error` into HTTP responses via two `IntoResponse` wrappers (`ApiError`, `OcsError`) — the unified `Error` type still owns the http_status + client_message mapping. The browser-UI surface (Dioxus Fullstack) is **not** mounted yet — Phase 4 lands that as a sub-router merged after the API routes. For Phase 3 the UI surface is a fallthrough that returns 404.

**Tech Stack:** Rust 1.85, `axum 0.8`, `tower 0.5`, `tower-http 0.6`, `hyper 1`, `cookie 0.18` (cookie parsing + attributes), `hmac 0.12` + `sha2 0.10` + `subtle 2.6` (signed session IDs, constant-time comparison), `base64 0.22` (URL-safe encoding of the cookie value), `bcrypt 0.16` (password hashing for the bootstrap admin), `rand 0.8` (CSPRNG for session IDs / CSRF tokens). All existing Phase 1/2 crates are consumed via the workspace.

**Parent spec:** `docs/superpowers/specs/2026-05-10-platform-core-design.md` — Phase 3 implements §7 (HTTP layer) and the in-scope acceptance criteria from §13 (#3 "serves traffic", #4 `/status.php`, #5 `/ocs/v2.php/cloud/capabilities`, #7 `/login`, #8 middleware enforcement).

**Previous phase:** Phase 2 ended at commit `ae59bb8`. Workspace has `rustcloud-cache`, `-config`, `-core`, `-db`, `-i18n`, `-ocs`, `-server`, `xtask` with ~90 unit tests + integration tests. `rustcloud-core::AppState` and `AppStateBuilder` are the substrate Phase 3 builds on.

---

## Conventions (carried from Phases 1–2)

- **Commits:** Conventional Commits (`feat(http)`, `chore(server)`, `test(http)`, …) with the Co-Authored-By trailer.
- **TDD:** Write failing test → fail → implement → pass → commit. For brand-new crates the first verification may be `cargo build`; meaningful tests follow immediately.
- **rustfmt:** Run `cargo fmt --all` after writing files. Authorized at every task boundary.
- **No mocks for DB / cache.** Tests use real `SqlitePool` (in-process) and real `MemoryCache`.
- **`tokio` features per crate:** the workspace baseline does NOT include `sync` / `signal`. Crates that need those features declare them locally (`tokio = { workspace = true, features = ["sync", "signal"] }`).
- **Errors:** library code uses `thiserror`; the binary's `main` converts to `anyhow::Result`. New errors flow into `rustcloud-core::Error` so `http_status()` + `client_message()` handle them.
- **Plan-bug protocol:** if verbatim code fails to compile or test, fix the minimal issue and report DONE_WITH_CONCERNS with a clear diff explanation. Don't silently deviate.

---

## File Structure (Phase 3 additions)

```
rustcloud/
├── Cargo.toml                                  # extended workspace deps + members
├── crates/
│   ├── rustcloud-http/                         # NEW
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                          # re-exports + build_router
│   │   │   ├── error.rs                        # ApiError + OcsError IntoResponse
│   │   │   ├── extractors/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── auth.rs                     # AuthenticatedUser + OptionalUser
│   │   │   │   └── format.rs                   # OcsFormat extractor (?format= + Accept)
│   │   │   ├── session/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── data.rs                     # Session struct + serde
│   │   │   │   ├── store.rs                    # SessionStore over Arc<dyn Cache>
│   │   │   │   ├── cookie.rs                   # signed cookie encode/decode (HMAC-SHA256)
│   │   │   │   └── layer.rs                    # SessionLayer Tower middleware
│   │   │   ├── csrf.rs                         # RequestToken + CsrfLayer
│   │   │   ├── middleware/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── proxy_headers.rs            # ProxyHeadersLayer
│   │   │   │   ├── trusted_domain.rs           # TrustedDomainLayer
│   │   │   │   ├── security_headers.rs         # SecurityHeadersLayer
│   │   │   │   └── body_limit.rs               # RequestBodyLimit wrapper
│   │   │   ├── routes/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── status.rs                   # GET /status.php
│   │   │   │   ├── login.rs                    # POST /index.php/login
│   │   │   │   ├── ui_fallback.rs              # placeholder Dioxus surface (404)
│   │   │   │   └── ocs/
│   │   │   │       ├── mod.rs                  # OCS sub-router builder
│   │   │   │       └── capabilities.rs         # /ocs/v2.php/cloud/capabilities
│   │   │   └── router.rs                       # build_router(state) compose
│   │   └── tests/
│   │       └── http_end_to_end.rs              # full server boot + curl-style tests
│   ├── rustcloud-config/                       # MODIFIED: add bootstrap_admin section
│   │   └── src/types.rs
│   ├── rustcloud-core/                         # MODIFIED: register CoreCapabilities default
│   │   └── src/state.rs                        # `AppStateBuilder::with_core_capabilities()` helper
│   └── rustcloud-server/                       # MODIFIED: serve subcommand + signal handling
│       ├── Cargo.toml                          # adds rustcloud-http, rustcloud-ocs deps
│       └── src/main.rs                         # Cmd::Serve runs axum::serve
└── docs/superpowers/plans/
    └── 2026-05-10-platform-core-phase-3-http.changelog.md   # NEW
```

---

## New workspace `[workspace.dependencies]` (added in Task 1)

```toml
# HTTP stack
axum = { version = "0.8", default-features = false, features = ["http1", "json", "tokio", "tower-log", "matched-path", "original-uri", "query"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["trace", "limit", "cors", "catch-panic"] }
hyper = { version = "1.5", features = ["http1", "server"] }

# Auth + session crypto
cookie = { version = "0.18", features = ["percent-encode"] }
hmac = "0.12"
sha2 = "0.10"
subtle = "2.6"
base64 = "0.22"
bcrypt = "0.16"
rand = "0.8"

# Internal
rustcloud-http = { path = "crates/rustcloud-http" }
```

(`rustcloud-ocs`, `rustcloud-core`, etc. are already in workspace deps from Phase 2.)

---

## Task 1: Workspace scaffold for `rustcloud-http`

**Files:**
- Modify: `Cargo.toml` (workspace members + workspace deps)
- Create: `crates/rustcloud-http/Cargo.toml`
- Create: `crates/rustcloud-http/src/lib.rs` (minimal stub)

Set up the crate and add all new external deps to the workspace. The crate will be filled out in subsequent tasks.

- [ ] **Step 1: Extend workspace `Cargo.toml`**

Modify root `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-core",
    "crates/rustcloud-db",
    "crates/rustcloud-http",
    "crates/rustcloud-i18n",
    "crates/rustcloud-ocs",
    "crates/rustcloud-server",
    "xtask",
]
resolver = "2"
```

Append the new entries to `[workspace.dependencies]` (keep entries roughly alphabetical):

```toml
axum = { version = "0.8", default-features = false, features = ["http1", "json", "tokio", "tower-log", "matched-path", "original-uri", "query"] }
base64 = "0.22"
bcrypt = "0.16"
cookie = { version = "0.18", features = ["percent-encode"] }
hmac = "0.12"
hyper = { version = "1.5", features = ["http1", "server"] }
rand = "0.8"
rustcloud-http = { path = "crates/rustcloud-http" }
sha2 = "0.10"
subtle = "2.6"
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["trace", "limit", "cors", "catch-panic"] }
```

- [ ] **Step 2: Write `crates/rustcloud-http/Cargo.toml`**

```toml
[package]
name = "rustcloud-http"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
axum.workspace = true
base64.workspace = true
bcrypt.workspace = true
cookie.workspace = true
hmac.workspace = true
hyper.workspace = true
rand.workspace = true
rustcloud-cache.workspace = true
rustcloud-config.workspace = true
rustcloud-core.workspace = true
rustcloud-i18n.workspace = true
rustcloud-ocs.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
subtle.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["sync"] }
tower.workspace = true
tower-http.workspace = true
tracing.workspace = true

[dev-dependencies]
rustcloud-db.workspace = true
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 3: Write minimal `crates/rustcloud-http/src/lib.rs`**

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers (`/status.php`, `/index.php/login`,
//! `/ocs/v2.php/cloud/capabilities`).
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

// Sub-modules are added incrementally in subsequent tasks.
```

- [ ] **Step 4: Build the crate**

Run:
```
cargo build -p rustcloud-http
```

Expected: clean build (zero source files of substance, but the dependency tree resolves).

- [ ] **Step 5: Run `cargo xtask check-all`**

Expected: still green; nothing else changed.

- [ ] **Step 6: Commit**

```
git add Cargo.toml Cargo.lock crates/rustcloud-http
git commit -m "chore(http): scaffold rustcloud-http crate with HTTP/auth deps

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Error → IntoResponse wrappers

**Files:**
- Create: `crates/rustcloud-http/src/error.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`

`rustcloud-core::Error` already has `http_status()` and `client_message()`. Phase 3 adds two thin axum response wrappers:

- `ApiError(Error)` → plain HTTP status + JSON `{"error": "..."}` body (for non-OCS routes like `/status.php` and `/index.php/login`).
- `OcsError(Error)` → OCS envelope wrapping (for `/ocs/*` routes), reusing `rustcloud-ocs::OcsResponse` + `render`.

Future Phase 3+: a `DavError` wrapper for WebDAV.

- [ ] **Step 1: Write `crates/rustcloud-http/src/error.rs`**

```rust
//! axum `IntoResponse` wrappers around `rustcloud_core::Error`. Two flavors:
//!
//! - `ApiError` — plain status + JSON body. For non-OCS endpoints.
//! - `OcsError` — wraps in the OCS envelope so `/ocs/*` responses match
//!   Nextcloud's wire format.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rustcloud_core::Error as CoreError;
use rustcloud_ocs::{render, Format, OcsResponse, OcsStatus, OcsVersion};
use serde_json::json;

/// Plain HTTP error response. Body is JSON `{"error": "..."}` with the
/// `client_message()` text.
#[derive(Debug)]
pub struct ApiError(pub CoreError);

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(json!({ "error": self.0.client_message() }));
        (status, body).into_response()
    }
}

/// OCS-envelope error response. Wraps a `CoreError` into an `OcsResponse`
/// rendered as XML by default (or JSON via the `?format=json` query / Accept).
#[derive(Debug)]
pub struct OcsError {
    pub error: CoreError,
    pub version: OcsVersion,
    pub format: Format,
}

impl OcsError {
    pub fn new(error: CoreError, version: OcsVersion, format: Format) -> Self {
        Self { error, version, format }
    }

    fn ocs_status(&self) -> OcsStatus {
        match self.error {
            CoreError::NotFound => OcsStatus::NotFound,
            CoreError::Unauthorized => OcsStatus::Unauthorized,
            CoreError::Forbidden => OcsStatus::Forbidden,
            CoreError::BadRequest(_) => OcsStatus::BadRequest,
            CoreError::Conflict(_) => OcsStatus::BadRequest, // OCS has no 409
            CoreError::Locked => OcsStatus::ServerError,     // 423 not in OCS palette
            CoreError::Ocs { code: _, .. } => OcsStatus::UnknownError, // raw code already in message
            CoreError::Config(_)
            | CoreError::ConfigValidation(_)
            | CoreError::Db(_)
            | CoreError::Cache(_)
            | CoreError::Internal(_) => OcsStatus::ServerError,
        }
    }
}

impl IntoResponse for OcsError {
    fn into_response(self) -> Response {
        let status = self.ocs_status();
        let http_status =
            StatusCode::from_u16(self.error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let envelope = OcsResponse {
            status,
            message: self.error.client_message(),
            data: serde_json::Value::Null,
            version: self.version,
        };
        let (body, ct) = render(&envelope, self.format);
        (http_status, [(header::CONTENT_TYPE, ct)], body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn api_error_emits_json_error_body() {
        let resp = ApiError(CoreError::NotFound).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], "Not Found");
    }

    #[tokio::test]
    async fn api_error_masks_internal_details() {
        let inner = CoreError::Internal(anyhow::anyhow!("connection pool exhausted: 42 waiting"));
        let resp = ApiError(inner).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], "Internal Server Error"); // generic; no leak
    }

    #[tokio::test]
    async fn ocs_error_emits_xml_envelope_by_default() {
        let err = OcsError::new(CoreError::NotFound, OcsVersion::V2, Format::Xml);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("application/xml"));
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let body_s = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_s.contains("<statuscode>998</statuscode>"));
        assert!(body_s.contains("<message>Not Found</message>"));
    }

    #[tokio::test]
    async fn ocs_error_emits_json_when_format_json() {
        let err = OcsError::new(CoreError::Unauthorized, OcsVersion::V2, Format::Json);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("application/json"));
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 997);
        assert_eq!(parsed["ocs"]["meta"]["status"], "failure");
    }
}
```

- [ ] **Step 2: Wire `error` into `lib.rs`**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod error;

pub use error::{ApiError, OcsError};
```

- [ ] **Step 3: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 4 tests pass.

- [ ] **Step 4: Run `cargo xtask check-all`**

Expected: still green.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): add ApiError and OcsError IntoResponse wrappers

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `/status.php` + minimal `build_router` + smoke ServeRun

**Files:**
- Create: `crates/rustcloud-http/src/routes/mod.rs`
- Create: `crates/rustcloud-http/src/routes/status.rs`
- Create: `crates/rustcloud-http/src/router.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`

End-state after this task: an axum router exists, `/status.php` returns the Nextcloud JSON shape, and a unit test exercises the route via `tower::ServiceExt::oneshot`.

- [ ] **Step 1: Write `crates/rustcloud-http/src/routes/mod.rs`**

```rust
//! HTTP route modules. Each handler lives in its own file.

pub mod status;
```

- [ ] **Step 2: Write `crates/rustcloud-http/src/routes/status.rs`**

```rust
//! `GET /status.php` — Nextcloud-compatible probe used by clients to identify
//! the server and decide whether to keep talking to it.
//!
//! See spec §7.7.

use axum::extract::State;
use axum::response::Json;
use rustcloud_core::AppState;
use serde::Serialize;

#[derive(Serialize)]
pub struct StatusResponse {
    pub installed: bool,
    pub maintenance: bool,
    #[serde(rename = "needsDbUpgrade")]
    pub needs_db_upgrade: bool,
    pub version: String,
    pub versionstring: String,
    pub edition: String,
    pub productname: String,
    #[serde(rename = "extendedSupport")]
    pub extended_support: bool,
}

pub async fn handler(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        installed: state.config.installed,
        maintenance: false,
        needs_db_upgrade: false,
        version: state.config.version.clone(),
        versionstring: state.config.versionstring.clone(),
        edition: String::new(),
        productname: "Nextcloud".to_string(),
        extended_support: false,
    })
}
```

- [ ] **Step 3: Write `crates/rustcloud-http/src/router.rs`**

```rust
//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to. Sub-routers are added one at a time
//! as Phase 3 tasks land them.

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

use crate::routes::status;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/status.php", get(status::handler))
        .with_state(state)
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod error;
mod router;
mod routes;

pub use error::{ApiError, OcsError};
pub use router::build_router;
```

- [ ] **Step 5: Write the unit test in `routes/status.rs`**

Append to `crates/rustcloud-http/src/routes/status.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::build_router;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use rustcloud_core::AppStateBuilder;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
        }
    }

    #[tokio::test]
    async fn status_returns_nextcloud_shape() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("status.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder().uri("/status.php").body(axum::body::Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["installed"], true);
        assert_eq!(parsed["maintenance"], false);
        assert_eq!(parsed["needsDbUpgrade"], false);
        assert_eq!(parsed["version"], "31.0.0.0");
        assert_eq!(parsed["versionstring"], "31.0.0");
        assert_eq!(parsed["productname"], "Nextcloud");
        assert_eq!(parsed["extendedSupport"], false);
    }
}
```

- [ ] **Step 6: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 5 tests pass (4 from error.rs + 1 new).

- [ ] **Step 7: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): add /status.php handler with Nextcloud-shape response

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Trusted-domain + proxy-headers middleware

**Files:**
- Create: `crates/rustcloud-http/src/middleware/mod.rs`
- Create: `crates/rustcloud-http/src/middleware/proxy_headers.rs`
- Create: `crates/rustcloud-http/src/middleware/trusted_domain.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`
- Modify: `crates/rustcloud-http/src/router.rs`

Two custom Tower layers, both per spec §7.2. ProxyHeadersLayer rewrites the request URI based on `X-Forwarded-Proto`/`X-Forwarded-Host` only when the peer is in `config.trusted_proxies`. TrustedDomainLayer rejects requests whose effective `Host` isn't in `config.trusted_domains`, except for loopback connections (so CLI tests still work).

- [ ] **Step 1: Write `crates/rustcloud-http/src/middleware/mod.rs`**

```rust
//! Tower middleware layers used by the HTTP router.

pub mod proxy_headers;
pub mod trusted_domain;
```

- [ ] **Step 2: Write `crates/rustcloud-http/src/middleware/proxy_headers.rs`**

```rust
//! `ProxyHeadersLayer` — honors `X-Forwarded-{Proto,Host,For}` only when the
//! request peer is in `config.trusted_proxies`. Rewrites the request's
//! effective `Host` header to match.
//!
//! For Phase 3 we trust the headers iff (a) `trusted_proxies` contains either
//! the literal `"loopback"` or the peer IP, OR (b) the request has no peer
//! info (axum's `ConnectInfo` extension is absent — typical in tower tests).
//! In production with real connections, the binary inserts `ConnectInfo` via
//! `into_make_service_with_connect_info`.

use axum::extract::ConnectInfo;
use axum::http::header::HeaderName;
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use futures::future::BoxFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

#[derive(Clone)]
pub struct ProxyHeadersLayer {
    pub trusted_proxies: Arc<Vec<String>>,
}

impl ProxyHeadersLayer {
    pub fn new(trusted_proxies: Vec<String>) -> Self {
        Self { trusted_proxies: Arc::new(trusted_proxies) }
    }
}

impl<S> Layer<S> for ProxyHeadersLayer {
    type Service = ProxyHeaders<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ProxyHeaders { inner, trusted_proxies: self.trusted_proxies.clone() }
    }
}

#[derive(Clone)]
pub struct ProxyHeaders<S> {
    inner: S,
    trusted_proxies: Arc<Vec<String>>,
}

fn peer_is_trusted(peer: Option<SocketAddr>, trusted: &[String]) -> bool {
    let peer_ip = match peer {
        Some(p) => p.ip().to_string(),
        None => return true, // No peer info → likely a test harness; trust by default.
    };
    trusted.iter().any(|t| t == &peer_ip || t == "loopback" && peer.unwrap().ip().is_loopback())
}

impl<S, B> Service<Request<B>> for ProxyHeaders<S>
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
        let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        let trusted = self.trusted_proxies.clone();
        let mut inner = self.inner.clone();

        if peer_is_trusted(peer, &trusted) {
            // Apply forwarded host if present.
            if let Some(host) = req.headers().get(&X_FORWARDED_HOST).cloned() {
                req.headers_mut().insert(axum::http::header::HOST, host);
            }
            // X-Forwarded-Proto is informational; we tag it onto extensions so
            // downstream code can read it via a typed extractor in later
            // tasks. For now we record the string.
            if let Some(proto) = req.headers().get(&X_FORWARDED_PROTO).cloned() {
                if let Ok(s) = proto.to_str() {
                    req.extensions_mut().insert(EffectiveScheme(s.to_string()));
                }
            }
        }

        Box::pin(async move { inner.call(req).await })
    }
}

#[derive(Debug, Clone)]
pub struct EffectiveScheme(pub String);
```

This file needs `futures` in workspace deps. `futures = "0.3"` is small. Add it to workspace deps.

Modify root `Cargo.toml` `[workspace.dependencies]`:

```toml
futures = "0.3"
```

Then add to the crate's `Cargo.toml` `[dependencies]`:

```toml
futures.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-http/src/middleware/trusted_domain.rs`**

```rust
//! `TrustedDomainLayer` — rejects requests whose effective `Host` isn't in
//! `config.trusted_domains`. Loopback peers are exempt so the `/status.php`
//! probe in a fresh install works.
//!
//! Spec §7.2.

use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

#[derive(Clone)]
pub struct TrustedDomainLayer {
    pub allowed: Arc<Vec<String>>,
}

impl TrustedDomainLayer {
    pub fn new(allowed: Vec<String>) -> Self {
        Self { allowed: Arc::new(allowed) }
    }
}

impl<S> Layer<S> for TrustedDomainLayer {
    type Service = TrustedDomain<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TrustedDomain { inner, allowed: self.allowed.clone() }
    }
}

#[derive(Clone)]
pub struct TrustedDomain<S> {
    inner: S,
    allowed: Arc<Vec<String>>,
}

fn host_in_list(host: &HeaderValue, list: &[String]) -> bool {
    let Ok(s) = host.to_str() else { return false };
    // Strip port if present.
    let bare = s.split(':').next().unwrap_or(s);
    list.iter().any(|d| d == bare)
}

fn peer_is_loopback(peer: Option<SocketAddr>) -> bool {
    match peer {
        Some(p) => p.ip().is_loopback(),
        None => true, // No connect info → tests; allow.
    }
}

impl<S, B> Service<Request<B>> for TrustedDomain<S>
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        let allowed = self.allowed.clone();
        let host = req.headers().get(axum::http::header::HOST).cloned();
        let mut inner = self.inner.clone();

        if peer_is_loopback(peer) {
            return Box::pin(async move { inner.call(req).await });
        }
        match host {
            Some(h) if host_in_list(&h, &allowed) => Box::pin(async move { inner.call(req).await }),
            _ => Box::pin(async move {
                Ok((StatusCode::BAD_REQUEST, "untrusted host").into_response())
            }),
        }
    }
}
```

- [ ] **Step 4: Mount middleware in `build_router`**

Modify `crates/rustcloud-http/src/router.rs`:

```rust
//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to.

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::status;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();

    Router::new()
        .route("/status.php", get(status::handler))
        .with_state(state)
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
}
```

- [ ] **Step 5: Re-export middleware module**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod error;
pub mod middleware;
mod router;
mod routes;

pub use error::{ApiError, OcsError};
pub use router::build_router;
```

- [ ] **Step 6: Add middleware tests**

Append to `crates/rustcloud-http/src/middleware/trusted_domain.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn ok() -> &'static str { "ok" }

    fn app(trusted: Vec<&str>) -> Router {
        Router::new()
            .route("/", get(ok))
            .layer(TrustedDomainLayer::new(trusted.into_iter().map(String::from).collect()))
    }

    #[tokio::test]
    async fn allows_request_without_connect_info() {
        // No ConnectInfo means loopback / test → allow.
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example.com")
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_untrusted_host_from_non_loopback_peer() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn allows_trusted_host_from_non_loopback_peer() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn strips_port_when_matching() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "cloud.example.com:8443")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

Append to `crates/rustcloud-http/src/middleware/proxy_headers.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn echo_host(req: Request<Body>) -> String {
        req.headers().get(axum::http::header::HOST).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
    }

    fn app(trusted: Vec<&str>) -> Router {
        Router::new()
            .route("/", get(echo_host))
            .layer(ProxyHeadersLayer::new(trusted.into_iter().map(String::from).collect()))
    }

    #[tokio::test]
    async fn rewrites_host_when_peer_trusted() {
        let peer = ConnectInfo::<SocketAddr>("10.0.0.1:5555".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "internal:8080")
            .header("x-forwarded-host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["10.0.0.1"]).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "cloud.example.com");
    }

    #[tokio::test]
    async fn ignores_forwarded_host_when_peer_untrusted() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:5555".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "internal:8080")
            .header("x-forwarded-host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["10.0.0.1"]).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "internal:8080");
    }
}
```

- [ ] **Step 7: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 11 tests pass (5 prior + 4 trusted_domain + 2 proxy_headers).

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-http Cargo.toml Cargo.lock
git commit -m "feat(http): add trusted-domain and proxy-headers middleware

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: Security-headers middleware + body limit

**Files:**
- Create: `crates/rustcloud-http/src/middleware/security_headers.rs`
- Modify: `crates/rustcloud-http/src/middleware/mod.rs`
- Modify: `crates/rustcloud-http/src/router.rs`

`SecurityHeadersLayer` adds the standard set the spec calls for: `Strict-Transport-Security`, `X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, plus a baseline `Content-Security-Policy`. The body-limit comes from `tower-http::limit` and is configured in `build_router`.

- [ ] **Step 1: Write `crates/rustcloud-http/src/middleware/security_headers.rs`**

```rust
//! `SecurityHeadersLayer` — sets the cluster of security headers spec §7.2
//! requires on every response. CSP differs between API and UI responses;
//! Phase 3 ships the API-restrictive baseline. Phase 4 (UI) will add a
//! per-route override for the Dioxus surface.

use axum::http::header::{HeaderName, HeaderValue};
use axum::http::Request;
use axum::response::Response;
use futures::future::BoxFuture;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const HSTS: (&str, &str) = ("strict-transport-security", "max-age=31536000; includeSubDomains");
const XCTO: (&str, &str) = ("x-content-type-options", "nosniff");
const REFERRER: (&str, &str) = ("referrer-policy", "strict-origin-when-cross-origin");
const XFO: (&str, &str) = ("x-frame-options", "SAMEORIGIN");
const CSP: (&str, &str) = (
    "content-security-policy",
    "default-src 'none'; frame-ancestors 'self'; base-uri 'self'",
);

#[derive(Clone, Default)]
pub struct SecurityHeadersLayer;

impl SecurityHeadersLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeaders<S>;
    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeaders { inner }
    }
}

#[derive(Clone)]
pub struct SecurityHeaders<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for SecurityHeaders<S>
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut resp = inner.call(req).await?;
            let headers = resp.headers_mut();
            for (name, value) in &[HSTS, XCTO, REFERRER, XFO, CSP] {
                headers.insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn ok() -> &'static str { "ok" }

    #[tokio::test]
    async fn all_baseline_security_headers_present() {
        let app = Router::new().route("/", get(ok)).layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let h = resp.headers();
        assert!(h.get("strict-transport-security").is_some());
        assert!(h.get("x-content-type-options").is_some());
        assert!(h.get("referrer-policy").is_some());
        assert!(h.get("x-frame-options").is_some());
        assert!(h.get("content-security-policy").is_some());
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
    }
}
```

- [ ] **Step 2: Re-export from middleware module**

Modify `crates/rustcloud-http/src/middleware/mod.rs`:

```rust
//! Tower middleware layers used by the HTTP router.

pub mod proxy_headers;
pub mod security_headers;
pub mod trusted_domain;
```

- [ ] **Step 3: Add body limit + security headers to `build_router`**

Modify `crates/rustcloud-http/src/router.rs`:

```rust
//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to.

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;
use tower_http::limit::RequestBodyLimitLayer;

use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::status;

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();

    Router::new()
        .route("/status.php", get(status::handler))
        .with_state(state)
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
}
```

- [ ] **Step 4: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 12 tests pass (11 prior + 1 new).

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): add security-headers middleware and 512 MiB body limit

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Session machinery — `Session`, signed cookie, cache-backed store

**Files:**
- Create: `crates/rustcloud-http/src/session/mod.rs`
- Create: `crates/rustcloud-http/src/session/data.rs`
- Create: `crates/rustcloud-http/src/session/cookie.rs`
- Create: `crates/rustcloud-http/src/session/store.rs`
- Create: `crates/rustcloud-http/src/session/layer.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`

The session payload is a small JSON blob carried in cache. The cookie carries an opaque ID + HMAC-SHA256 signature keyed by `config.secret`. `SessionLayer` reads the cookie on each request, loads the session (or creates a fresh empty one), inserts it as a request extension, and on response writes it back to cache (sliding TTL).

- [ ] **Step 1: Write `crates/rustcloud-http/src/session/mod.rs`**

```rust
//! Session machinery: data model, cache-backed store, signed cookie, and the
//! Tower layer that ties them together.
//!
//! See spec §7.3.

mod cookie;
mod data;
mod layer;
mod store;

pub use cookie::{decode_cookie, encode_cookie, CookieError};
pub use data::{Session, SessionId};
pub use layer::SessionLayer;
pub use store::SessionStore;
```

- [ ] **Step 2: Write `crates/rustcloud-http/src/session/data.rs`**

```rust
//! Session payload types. Stored in cache as JSON.

use serde::{Deserialize, Serialize};

/// Opaque session ID. 32 random bytes, hex-encoded for storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new_random() -> Self {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        SessionId(hex::encode(buf))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Server-side session data. Persisted in cache keyed by `SessionId`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// Authenticated user ID, if any.
    pub user_id: Option<String>,
    /// CSRF request token. Rotated at login/logout.
    pub csrf_token: String,
    /// Last access timestamp (seconds since epoch). Used for sliding TTL.
    pub last_activity: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            user_id: None,
            csrf_token: random_token(),
            last_activity: now_secs(),
        }
    }

    pub fn rotate_csrf(&mut self) {
        self.csrf_token = random_token();
    }
}

fn random_token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_64_hex_chars() {
        let id = SessionId::new_random();
        assert_eq!(id.0.len(), 64);
        assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ids_differ_on_each_call() {
        let a = SessionId::new_random();
        let b = SessionId::new_random();
        assert_ne!(a, b);
    }

    #[test]
    fn new_session_has_token_and_no_user() {
        let s = Session::new();
        assert!(s.user_id.is_none());
        assert_eq!(s.csrf_token.len(), 64);
    }

    #[test]
    fn rotate_csrf_changes_token() {
        let mut s = Session::new();
        let before = s.csrf_token.clone();
        s.rotate_csrf();
        assert_ne!(s.csrf_token, before);
    }
}
```

`hex` is a new dep — add to workspace deps. Append to root `Cargo.toml`:

```toml
hex = "0.4"
```

Then in `crates/rustcloud-http/Cargo.toml` `[dependencies]`:

```toml
hex.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-http/src/session/cookie.rs`**

```rust
//! Signed-cookie encode/decode. The cookie value format is:
//!
//!     <base64url(session_id_bytes)>.<base64url(hmac)>
//!
//! HMAC is HMAC-SHA256 keyed by `config.secret`, computed over the raw
//! session-ID bytes. Verification is constant-time via `subtle`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum CookieError {
    #[error("cookie value is not a valid signed-session token")]
    Malformed,
    #[error("cookie signature mismatch")]
    BadSignature,
}

pub fn encode_cookie(session_id_hex: &str, secret: &[u8]) -> String {
    let id_bytes = hex::decode(session_id_hex).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&id_bytes);
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", B64.encode(&id_bytes), B64.encode(sig))
}

pub fn decode_cookie(raw: &str, secret: &[u8]) -> Result<String, CookieError> {
    let (id_b64, sig_b64) = raw.split_once('.').ok_or(CookieError::Malformed)?;
    let id_bytes = B64.decode(id_b64).map_err(|_| CookieError::Malformed)?;
    let sig = B64.decode(sig_b64).map_err(|_| CookieError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&id_bytes);
    let expected = mac.finalize().into_bytes();
    if expected.ct_eq(&sig).into() {
        Ok(hex::encode(&id_bytes))
    } else {
        Err(CookieError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_id() {
        let id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let secret = b"shhh-its-a-secret";
        let token = encode_cookie(id, secret);
        let decoded = decode_cookie(&token, secret).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn wrong_secret_fails_verification() {
        let id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let token = encode_cookie(id, b"key-a");
        let err = decode_cookie(&token, b"key-b").unwrap_err();
        assert!(matches!(err, CookieError::BadSignature));
    }

    #[test]
    fn malformed_value_is_rejected() {
        let err = decode_cookie("no-dot", b"key").unwrap_err();
        assert!(matches!(err, CookieError::Malformed));
        let err2 = decode_cookie("xx.yy", b"key").unwrap_err();
        // Either malformed base64 or bad signature once decoded — both are
        // safe rejections; our implementation returns Malformed when b64 fails.
        assert!(matches!(err2, CookieError::Malformed | CookieError::BadSignature));
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-http/src/session/store.rs`**

```rust
//! `SessionStore` — typed wrapper over `Arc<dyn Cache>` for session payloads.

use crate::session::data::{Session, SessionId};
use rustcloud_cache::{Cache, CacheError};
use std::sync::Arc;
use std::time::Duration;

/// Idle TTL for sessions. Spec §7.3 says 30 min idle, 24 h absolute. Phase 3
/// ships the idle-TTL only; absolute-TTL enforcement is a Phase 4 concern.
pub const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct SessionStore {
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl SessionStore {
    pub fn new(cache: Arc<dyn Cache>, instance_id: impl Into<String>) -> Self {
        Self { cache, instance_id: instance_id.into() }
    }

    fn key(&self, id: &SessionId) -> String {
        format!("{}:session:{}", self.instance_id, id.as_str())
    }

    pub async fn load(&self, id: &SessionId) -> Result<Option<Session>, CacheError> {
        let raw = self.cache.get(&self.key(id)).await?;
        match raw {
            Some(bytes) => {
                let s: Session = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::Io(format!("session decode: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub async fn save(&self, id: &SessionId, session: &Session) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| CacheError::Io(format!("session encode: {e}")))?;
        self.cache.set(&self.key(id), &bytes, Some(SESSION_IDLE_TTL)).await
    }

    pub async fn destroy(&self, id: &SessionId) -> Result<(), CacheError> {
        self.cache.del(&self.key(id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        let mut s = Session::new();
        s.user_id = Some("alice".into());
        store.save(&id, &s).await.unwrap();
        let loaded = store.load(&id).await.unwrap().unwrap();
        assert_eq!(loaded.user_id.as_deref(), Some("alice"));
        assert_eq!(loaded.csrf_token, s.csrf_token);
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_removes_session() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        store.save(&id, &Session::new()).await.unwrap();
        store.destroy(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }
}
```

- [ ] **Step 5: Write `crates/rustcloud-http/src/session/layer.rs`**

```rust
//! `SessionLayer` — Tower middleware that loads the session from the cookie
//! into a request extension, then writes it back on response.
//!
//! Cookie name: `oc_sessionPassphrase` (Nextcloud-compatible).

use crate::session::cookie::{decode_cookie, encode_cookie};
use crate::session::data::{Session, SessionId};
use crate::session::store::SessionStore;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use futures::future::BoxFuture;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::{Layer, Service};

pub const COOKIE_NAME: &str = "oc_sessionPassphrase";

/// Wrapper inserted into request extensions so handlers can mutate the session.
#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub inner: Arc<Mutex<Session>>,
    /// Set to true when the handler wants the session destroyed on response.
    pub destroy: Arc<Mutex<bool>>,
}

impl SessionHandle {
    pub async fn read(&self) -> Session {
        self.inner.lock().await.clone()
    }
    pub async fn mutate<F: FnOnce(&mut Session)>(&self, f: F) {
        let mut s = self.inner.lock().await;
        f(&mut s);
    }
    pub async fn destroy(&self) {
        *self.destroy.lock().await = true;
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
        Self { store, secret: Arc::new(secret), secure }
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

fn extract_cookie(req: &Request<impl Send>, name: &str) -> Option<String> {
    let raw = req.headers().get(COOKIE)?.to_str().ok()?;
    for piece in raw.split(';').map(str::trim) {
        if let Some((k, v)) = piece.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn make_set_cookie(value: &str, secure: bool, max_age: u64) -> HeaderValue {
    let mut s = format!("{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}", COOKIE_NAME, value, max_age);
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

fn make_destroy_cookie(secure: bool) -> HeaderValue {
    let mut s = format!("{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0", COOKIE_NAME);
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
            // 1. Resolve session ID from cookie, or mint a new one.
            let (id, mut session) = match extract_cookie(&req, COOKIE_NAME) {
                Some(raw) => match decode_cookie(&raw, secret.expose_secret().as_bytes()) {
                    Ok(id_hex) => {
                        let id = SessionId(id_hex);
                        let loaded = store.load(&id).await.ok().flatten();
                        match loaded {
                            Some(s) => (id, s),
                            None => (SessionId::new_random(), Session::new()),
                        }
                    }
                    Err(_) => (SessionId::new_random(), Session::new()),
                },
                None => (SessionId::new_random(), Session::new()),
            };

            // 2. Slide TTL (touch last_activity).
            session.last_activity = now_secs();

            // 3. Insert handle into request extensions.
            let handle = SessionHandle {
                id: id.clone(),
                inner: Arc::new(Mutex::new(session)),
                destroy: Arc::new(Mutex::new(false)),
            };
            req.extensions_mut().insert(handle.clone());

            // 4. Run inner service.
            let mut resp = inner.call(req).await?;

            // 5. Save or destroy session as the handler indicated.
            let destroy = *handle.destroy.lock().await;
            if destroy {
                let _ = store.destroy(&handle.id).await;
                resp.headers_mut().append(SET_COOKIE, make_destroy_cookie(secure));
            } else {
                let final_session = handle.inner.lock().await.clone();
                let _ = store.save(&handle.id, &final_session).await;
                let cookie_value = encode_cookie(handle.id.as_str(), secret.expose_secret().as_bytes());
                resp.headers_mut().append(
                    SET_COOKIE,
                    make_set_cookie(&cookie_value, secure, super::store::SESSION_IDLE_TTL.as_secs()),
                );
            }

            Ok(resp)
        })
    }
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::SessionStore;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use rustcloud_cache::MemoryCache;
    use tower::ServiceExt;

    async fn login_handler(axum::Extension(handle): axum::Extension<SessionHandle>) -> &'static str {
        handle.mutate(|s| s.user_id = Some("alice".into())).await;
        "ok"
    }

    async fn whoami(axum::Extension(handle): axum::Extension<SessionHandle>) -> String {
        handle.read().await.user_id.unwrap_or_default()
    }

    fn app() -> (Router, Arc<dyn rustcloud_cache::Cache>) {
        let cache: Arc<dyn rustcloud_cache::Cache> = Arc::new(MemoryCache::new());
        let store = SessionStore::new(cache.clone(), "inst1");
        let layer = SessionLayer::new(store, SecretString::new("secret".into()), false);
        let app = Router::new()
            .route("/login", get(login_handler))
            .route("/whoami", get(whoami))
            .layer(layer);
        (app, cache)
    }

    #[tokio::test]
    async fn login_sets_session_cookie() {
        let (app, _) = app();
        let req = Request::builder().uri("/login").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let setc = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
        assert!(setc.starts_with("oc_sessionPassphrase="));
        assert!(setc.contains("HttpOnly"));
        assert!(setc.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn round_trip_session_via_cookie() {
        let (app, _) = app();
        // 1st request: login.
        let req1 = Request::builder().uri("/login").body(Body::empty()).unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        let setc = resp1.headers().get(SET_COOKIE).unwrap().to_str().unwrap().to_string();
        let cookie = setc.split(';').next().unwrap().to_string();
        // 2nd request: whoami with the cookie.
        let req2 = Request::builder()
            .uri("/whoami")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let body = axum::body::to_bytes(resp2.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "alice");
    }
}
```

- [ ] **Step 6: Wire `session` into `lib.rs`**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod error;
pub mod middleware;
mod router;
mod routes;
pub mod session;

pub use error::{ApiError, OcsError};
pub use router::build_router;
pub use session::{Session, SessionHandle, SessionId, SessionLayer, SessionStore};
```

(Re-exporting `SessionHandle` so handlers can extract it; it's defined in `session::layer` — adjust the re-export to point at the right module.)

Adjust `crates/rustcloud-http/src/session/mod.rs` to also export `SessionHandle`:

```rust
//! Session machinery: data model, cache-backed store, signed cookie, and the
//! Tower layer that ties them together.
//!
//! See spec §7.3.

mod cookie;
mod data;
mod layer;
mod store;

pub use cookie::{decode_cookie, encode_cookie, CookieError};
pub use data::{Session, SessionId};
pub use layer::{SessionHandle, SessionLayer, COOKIE_NAME};
pub use store::{SessionStore, SESSION_IDLE_TTL};
```

- [ ] **Step 7: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 22 tests pass (12 prior + 4 data + 3 cookie + 3 store + 2 layer).

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-http Cargo.toml Cargo.lock
git commit -m "feat(http): session machinery — signed cookie + cache-backed store + layer

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: CSRF — `RequestToken` + middleware with OCS-APIRequest bypass

**Files:**
- Create: `crates/rustcloud-http/src/csrf.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`

CSRF middleware runs **after** `SessionLayer` in the layer stack (i.e. inserted closer to the handler). For each request:

- If the method is safe (GET, HEAD, OPTIONS) → pass.
- If `OCS-APIRequest: true` header is present → pass (matches Nextcloud).
- If there's no session user (anonymous request) → pass.
- Otherwise: require `requesttoken` header or form field to equal the session's `csrf_token`; 403 if absent / mismatched.

- [ ] **Step 1: Write `crates/rustcloud-http/src/csrf.rs`**

```rust
//! CSRF middleware — matches Nextcloud's request-token scheme. Reads the
//! token from `requesttoken` header (or query/form field), compares against
//! the session's `csrf_token`, bypasses entirely for `OCS-APIRequest: true`
//! and for non-authenticated requests.
//!
//! Spec §7.4.

use crate::session::SessionHandle;
use axum::http::{HeaderName, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const TOKEN_HEADER: HeaderName = HeaderName::from_static("requesttoken");
const OCS_APIREQUEST_HEADER: HeaderName = HeaderName::from_static("ocs-apirequest");

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

#[derive(Clone, Default)]
pub struct CsrfLayer;

impl CsrfLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        CsrfMiddleware { inner }
    }
}

#[derive(Clone)]
pub struct CsrfMiddleware<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for CsrfMiddleware<S>
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            // Safe methods bypass.
            if is_safe_method(req.method()) {
                return inner.call(req).await;
            }
            // OCS-APIRequest header bypass (Nextcloud convention).
            if req
                .headers()
                .get(&OCS_APIREQUEST_HEADER)
                .map(|v| v.as_bytes() == b"true")
                .unwrap_or(false)
            {
                return inner.call(req).await;
            }
            // Anonymous (no session user) bypass.
            let handle = req.extensions().get::<SessionHandle>().cloned();
            let user_id = match &handle {
                Some(h) => h.read().await.user_id.clone(),
                None => None,
            };
            if user_id.is_none() {
                return inner.call(req).await;
            }
            // Authenticated session: require matching token.
            let expected = match &handle {
                Some(h) => h.read().await.csrf_token.clone(),
                None => String::new(),
            };
            let supplied = req.headers().get(&TOKEN_HEADER).and_then(|v| v.to_str().ok());
            if supplied.map(|s| s == expected).unwrap_or(false) {
                inner.call(req).await
            } else {
                Ok((StatusCode::FORBIDDEN, "csrf token missing or mismatched").into_response())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionHandle;
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn handler() -> &'static str { "ok" }

    fn handle_with_user(user: Option<&str>, token: &str) -> SessionHandle {
        let mut s = crate::session::Session::new();
        s.user_id = user.map(String::from);
        s.csrf_token = token.into();
        SessionHandle {
            id: crate::session::SessionId("00".into()),
            inner: Arc::new(Mutex::new(s)),
            destroy: Arc::new(Mutex::new(false)),
        }
    }

    fn app() -> Router {
        Router::new()
            .route("/safe", get(handler))
            .route("/danger", post(handler))
            .layer(CsrfLayer::new())
    }

    #[tokio::test]
    async fn safe_method_passes_without_token() {
        let req = Request::builder()
            .method("GET")
            .uri("/safe")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn anonymous_post_passes_without_token() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ocs_apirequest_bypasses_check() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("ocs-apirequest", "true")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_post_without_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticated_post_with_matching_token_passes() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "expected")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_post_with_mismatching_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "wrong")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod csrf;
mod error;
pub mod middleware;
mod router;
mod routes;
pub mod session;

pub use csrf::CsrfLayer;
pub use error::{ApiError, OcsError};
pub use router::build_router;
pub use session::{Session, SessionHandle, SessionId, SessionLayer, SessionStore};
```

- [ ] **Step 3: Run the tests**

```
cargo test -p rustcloud-http --lib
```

Expected: 28 tests pass (22 prior + 6 csrf).

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): CSRF middleware with OCS-APIRequest and safe-method bypass

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: AuthExtractors + bootstrap_admin config

**Files:**
- Modify: `crates/rustcloud-config/src/types.rs` (add `BootstrapAdminConfig`)
- Create: `crates/rustcloud-http/src/extractors/mod.rs`
- Create: `crates/rustcloud-http/src/extractors/auth.rs`
- Modify: `crates/rustcloud-http/src/lib.rs`

For Phase 3, "the user store" doesn't exist yet — the proper implementation is its own sub-project. We support a single hard-coded admin via `config.bootstrap_admin.{username, password_hash}` (bcrypt). `AuthenticatedUser`/`OptionalUser` extractors resolve via the `SessionHandle`.

- [ ] **Step 1: Add `BootstrapAdminConfig` to `rustcloud-config`**

Modify `crates/rustcloud-config/src/types.rs` — append after the existing `CacheConfig`:

```rust
/// Phase 3 stub for the deferred users sub-project. A single admin account
/// whose credentials live in `config.toml`. Real user store lands later.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAdminConfig {
    pub username: String,
    /// bcrypt hash of the password. Generate with `htpasswd -nBC 12` or
    /// `bcrypt::hash`.
    pub password_hash: String,
}
```

Then extend the `FileConfig` struct (still in the same file). Add a new field just before the trailing `}`:

Find:
```rust
    #[serde(default)]
    pub cache: CacheConfig,
}
```

Replace with:
```rust
    #[serde(default)]
    pub cache: CacheConfig,

    /// Optional bootstrap admin (Phase 3 deferred-users stand-in).
    pub bootstrap_admin: Option<BootstrapAdminConfig>,
}
```

Re-export from `crates/rustcloud-config/src/lib.rs` by amending its types re-export line:

Find:
```rust
pub use types::{CacheConfig, DbType, FileConfig, FileConfigError};
```

Replace with:
```rust
pub use types::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig, FileConfigError};
```

Update existing tests in `types.rs` that construct `FileConfig` literally — add `bootstrap_admin: None,` to each fixture. Grep for `minimal_sqlite_config` and add the field.

Add a new test in `types.rs`:

```rust
    #[test]
    fn bootstrap_admin_round_trips_via_toml() {
        let input = r#"
instanceid = "i1"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "rustcloud"
datadirectory = "/var/lib/rustcloud"
trusted_domains = ["localhost"]

[bootstrap_admin]
username = "admin"
password_hash = "$2b$12$abcdefghijklmnopqrstuv"
"#;
        let cfg: FileConfig = toml::from_str(input).unwrap();
        cfg.validate().unwrap();
        let ba = cfg.bootstrap_admin.unwrap();
        assert_eq!(ba.username, "admin");
        assert!(ba.password_hash.starts_with("$2b$"));
    }
```

- [ ] **Step 2: Apply the same fixture fixup in `rustcloud-db` + `rustcloud-core` + `rustcloud-http` tests**

Several test files construct `FileConfig` by hand (`cfg_sqlite`). After Step 1, these will fail to compile (missing field). Update each:

- `crates/rustcloud-db/src/pool.rs` tests
- `crates/rustcloud-db/src/migrate.rs` tests
- `crates/rustcloud-db/src/core_migrations.rs` tests
- `crates/rustcloud-db/tests/migrate_end_to_end.rs`
- `crates/rustcloud-core/src/appconfig.rs` tests
- `crates/rustcloud-core/src/state.rs` tests
- `crates/rustcloud-core/tests/app_state_build.rs`
- `crates/rustcloud-http/src/routes/status.rs` tests

In each, find the literal `cache: CacheConfig::default(),` (or `cache: CacheConfig { ... }`) line and append after it inside the struct literal:

```rust
            bootstrap_admin: None,
```

Sanity-check by running `cargo build --workspace --all-targets` after the edits. Compilation errors are expected to pinpoint any remaining unfixed call sites.

- [ ] **Step 3: Write `crates/rustcloud-http/src/extractors/mod.rs`**

```rust
//! axum extractors for HTTP handlers.

pub mod auth;
```

- [ ] **Step 4: Write `crates/rustcloud-http/src/extractors/auth.rs`**

```rust
//! `AuthenticatedUser` and `OptionalUser` axum extractors. Phase 3 resolves
//! the user purely from the session cookie — Bearer/Basic/app-password auth
//! lands later.

use crate::session::SessionHandle;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub auth_method: AuthMethod,
}

#[derive(Debug, Clone)]
pub enum AuthMethod {
    Session,
    // Bearer / Basic / AppPassword variants land in the users sub-project.
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
        let handle = parts
            .extensions
            .get::<SessionHandle>()
            .cloned()
            .ok_or(UnauthorizedRejection)?;
        let session = handle.read().await;
        let user_id = session.user_id.ok_or(UnauthorizedRejection)?;
        Ok(AuthenticatedUser { user_id, auth_method: AuthMethod::Session })
    }
}

/// `Option<AuthenticatedUser>`-style extractor for handlers that work for both
/// anonymous and authenticated callers.
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<AuthenticatedUser>);

impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let handle = parts.extensions.get::<SessionHandle>().cloned();
        if let Some(h) = handle {
            let session = h.read().await;
            if let Some(uid) = session.user_id {
                return Ok(OptionalUser(Some(AuthenticatedUser {
                    user_id: uid,
                    auth_method: AuthMethod::Session,
                })));
            }
        }
        Ok(OptionalUser(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, SessionHandle, SessionId};
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn auth_only(user: AuthenticatedUser) -> String { user.user_id }
    async fn optional(opt: OptionalUser) -> String {
        opt.0.map(|u| u.user_id).unwrap_or_else(|| "guest".into())
    }

    fn handle_with(user: Option<&str>) -> SessionHandle {
        let mut s = Session::new();
        s.user_id = user.map(String::from);
        SessionHandle {
            id: SessionId("00".into()),
            inner: Arc::new(Mutex::new(s)),
            destroy: Arc::new(Mutex::new(false)),
        }
    }

    fn app() -> Router {
        Router::new().route("/auth", get(auth_only)).route("/opt", get(optional))
    }

    #[tokio::test]
    async fn authenticated_user_rejects_when_no_session() {
        let req = Request::builder().uri("/auth").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_user_rejects_when_session_has_no_user() {
        let req = Request::builder()
            .uri("/auth")
            .extension(handle_with(None))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_user_resolves_when_session_has_user() {
        let req = Request::builder()
            .uri("/auth")
            .extension(handle_with(Some("alice")))
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
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "guest");
    }
}
```

- [ ] **Step 5: Re-export the extractors**

Modify `crates/rustcloud-http/src/lib.rs`:

```rust
//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod csrf;
mod error;
pub mod extractors;
pub mod middleware;
mod router;
mod routes;
pub mod session;

pub use csrf::CsrfLayer;
pub use error::{ApiError, OcsError};
pub use extractors::auth::{AuthMethod, AuthenticatedUser, OptionalUser};
pub use router::build_router;
pub use session::{Session, SessionHandle, SessionId, SessionLayer, SessionStore};
```

- [ ] **Step 6: Run the workspace tests**

```
cargo xtask check-all
```

Expected: green. New tests: 4 auth + 1 bootstrap_admin TOML = 5 new tests. Workspace total now around 33 in `rustcloud-http`.

- [ ] **Step 7: Commit**

```
git add crates/rustcloud-config crates/rustcloud-http crates/rustcloud-db crates/rustcloud-core
git commit -m "feat(http,config): add bootstrap_admin + AuthenticatedUser/OptionalUser extractors

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: `POST /index.php/login` against bootstrap admin

**Files:**
- Create: `crates/rustcloud-http/src/routes/login.rs`
- Modify: `crates/rustcloud-http/src/routes/mod.rs`
- Modify: `crates/rustcloud-http/src/router.rs`

Handler accepts form-encoded `username` + `password`, verifies via bcrypt against `state.config.bootstrap_admin`, sets `session.user_id`, rotates the CSRF token, and returns 303 to `/`. Bad credentials return 401 (no redirect).

- [ ] **Step 1: Write `crates/rustcloud-http/src/routes/login.rs`**

```rust
//! `POST /index.php/login` — bootstrap-admin login. Form-encoded body with
//! `username` and `password`. Bcrypt verification. On success, populates the
//! session and 303-redirects to `/`.

use crate::session::SessionHandle;
use crate::ApiError;
use axum::extract::{Form, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use rustcloud_core::{AppState, Error as CoreError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Extension(handle): Extension<SessionHandle>,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    let admin = state
        .config
        .bootstrap_admin
        .as_ref()
        .ok_or_else(|| CoreError::Unauthorized)?;

    if admin.username != form.username {
        return Err(ApiError(CoreError::Unauthorized));
    }
    let ok = bcrypt::verify(&form.password, &admin.password_hash)
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("bcrypt verify: {e}")))?;
    if !ok {
        return Err(ApiError(CoreError::Unauthorized));
    }

    handle
        .mutate(|s| {
            s.user_id = Some(form.username.clone());
            s.rotate_csrf();
        })
        .await;

    let mut resp = (StatusCode::SEE_OTHER, "").into_response();
    resp.headers_mut().insert(axum::http::header::LOCATION, HeaderValue::from_static("/"));
    Ok(resp)
}
```

- [ ] **Step 2: Re-export from `routes/mod.rs`**

Modify `crates/rustcloud-http/src/routes/mod.rs`:

```rust
//! HTTP route modules. Each handler lives in its own file.

pub mod login;
pub mod status;
```

- [ ] **Step 3: Mount `/index.php/login` in `build_router`**

Modify `crates/rustcloud-http/src/router.rs`:

```rust
//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to.

use axum::routing::{get, post};
use axum::Router;
use rustcloud_core::AppState;
use tower_http::limit::RequestBodyLimitLayer;

use crate::csrf::CsrfLayer;
use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::{login, status};
use crate::session::{SessionLayer, SessionStore};

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();
    let secret = state.config.secret.clone();
    let cache = state.cache.clone();
    let instance_id = state.config.instanceid.clone();
    let secure_cookies = state
        .config
        .overwrite_protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    let session_store = SessionStore::new(cache, &instance_id);

    Router::new()
        .route("/status.php", get(status::handler))
        .route("/index.php/login", post(login::handler))
        .with_state(state)
        .layer(CsrfLayer::new())
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
}
```

- [ ] **Step 4: Add a login test using a real bcrypt hash**

Append to `crates/rustcloud-http/src/routes/login.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::build_router;
    use axum::body::Body;
    use axum::http::Request;
    use rustcloud_config::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
    use rustcloud_core::AppStateBuilder;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn cfg_with_admin(path: PathBuf, hash: &str) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
            bootstrap_admin: Some(BootstrapAdminConfig {
                username: "admin".into(),
                password_hash: hash.into(),
            }),
        }
    }

    fn valid_login_body(user: &str, pass: &str) -> Body {
        Body::from(format!("username={user}&password={pass}"))
    }

    #[tokio::test]
    async fn correct_credentials_set_session_and_redirect() {
        let dir = tempdir().unwrap();
        let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
        let cfg = cfg_with_admin(dir.path().join("login.db"), &hash);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "hunter2"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/");
        let setc = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(setc.starts_with("oc_sessionPassphrase="));
    }

    #[tokio::test]
    async fn wrong_password_returns_401() {
        let dir = tempdir().unwrap();
        let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
        let cfg = cfg_with_admin(dir.path().join("login.db"), &hash);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "WRONG"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_admin_config_returns_401() {
        let dir = tempdir().unwrap();
        let mut cfg = cfg_with_admin(dir.path().join("login.db"), "irrelevant");
        cfg.bootstrap_admin = None;
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "hunter2"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 5: Run the tests**

```
cargo test -p rustcloud-http
```

Expected: ~36 tests pass (33 prior + 3 login).

- [ ] **Step 6: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): /index.php/login with bootstrap_admin bcrypt verification

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: OCS router + `/ocs/v2.php/cloud/capabilities`

**Files:**
- Create: `crates/rustcloud-http/src/routes/ocs/mod.rs`
- Create: `crates/rustcloud-http/src/routes/ocs/capabilities.rs`
- Create: `crates/rustcloud-http/src/extractors/format.rs`
- Modify: `crates/rustcloud-http/src/extractors/mod.rs`
- Modify: `crates/rustcloud-http/src/routes/mod.rs`
- Modify: `crates/rustcloud-http/src/router.rs`
- Modify: `crates/rustcloud-core/src/state.rs` (helper to seed CoreCapabilities)

The OCS sub-router lives under `/ocs/v2.php`. The capabilities handler resolves format from `?format=` query or `Accept` header, runs the Phase 2 aggregator, and emits the assembled OCS response. The aggregator needs at least one provider; we add `with_core_capabilities()` to `AppStateBuilder` so the default `CoreCapabilities` provider is registered automatically.

- [ ] **Step 1: Add an `AppStateBuilder::with_core_capabilities()` helper**

Modify `crates/rustcloud-core/src/state.rs`. In `impl AppStateBuilder { ... }`, after `with_hook`:

```rust
    /// Register the default `CoreCapabilities` provider on bootstrap so the
    /// `core` namespace is non-empty at the `/ocs/.../capabilities` route.
    pub fn with_core_capabilities(self) -> Self {
        use rustcloud_ocs::CoreCapabilities;
        let core = std::sync::Arc::new(CoreCapabilities::default());
        self.with_hook(crate::bootstrap::boxed_hook(move |state| async move {
            state.register_capability_provider(core).await;
            Ok(())
        }))
    }
```

(`with_hook` already consumes `self` and returns `Self`, so this chains cleanly.)

Add a unit test in the state.rs tests module:

```rust
    #[tokio::test]
    async fn with_core_capabilities_registers_the_provider() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg).with_core_capabilities().build().await.unwrap();
        let guard = state.capability_providers.lock().await;
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].namespace(), "core");
    }
```

- [ ] **Step 2: Write the OCS format extractor**

Create `crates/rustcloud-http/src/extractors/format.rs`:

```rust
//! `OcsFormat` extractor — resolves the response format from `?format=`
//! query string or the `Accept` request header, mirroring Nextcloud's
//! content-negotiation rules.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use rustcloud_ocs::{negotiate, Format};
use std::convert::Infallible;

#[derive(Debug, Clone, Copy)]
pub struct OcsFormat(pub Format);

impl<S> FromRequestParts<S> for OcsFormat
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let format_query: Option<String> = parts
            .uri
            .query()
            .and_then(|q| {
                q.split('&').find_map(|kv| {
                    kv.split_once('=').and_then(|(k, v)| (k == "format").then(|| v.to_string()))
                })
            });
        let accept = parts
            .headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        Ok(OcsFormat(negotiate(format_query.as_deref(), accept.as_deref())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn echo_format(f: OcsFormat) -> &'static str {
        match f.0 {
            Format::Json => "json",
            Format::Xml => "xml",
        }
    }

    fn app() -> Router {
        Router::new().route("/x", get(echo_format))
    }

    #[tokio::test]
    async fn defaults_to_xml() {
        let req = Request::builder().uri("/x").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"xml");
    }

    #[tokio::test]
    async fn query_param_selects_json() {
        let req = Request::builder().uri("/x?format=json").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"json");
    }

    #[tokio::test]
    async fn accept_header_selects_json() {
        let req = Request::builder()
            .uri("/x")
            .header("accept", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"json");
    }
}
```

Modify `crates/rustcloud-http/src/extractors/mod.rs`:

```rust
//! axum extractors for HTTP handlers.

pub mod auth;
pub mod format;
```

- [ ] **Step 3: Write `crates/rustcloud-http/src/routes/ocs/mod.rs`**

```rust
//! OCS sub-router under `/ocs/v2.php`.

pub mod capabilities;

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/v2.php/cloud/capabilities", get(capabilities::handler))
}
```

- [ ] **Step 4: Write `crates/rustcloud-http/src/routes/ocs/capabilities.rs`**

```rust
//! `GET /ocs/v2.php/cloud/capabilities`.

use crate::extractors::auth::OptionalUser;
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rustcloud_core::{AppState, Error as CoreError};
use rustcloud_ocs::{aggregate, render, CapabilityContext, OcsResponse, OcsStatus, OcsVersion};

pub async fn handler(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let user_id = user.as_ref().map(|u| u.user_id.clone());
    let ctx = CapabilityContext {
        locale: None,
        user_id: user_id.as_deref(),
    };
    let providers = state.capability_providers.lock().await.clone();
    let payload = aggregate(
        &providers,
        &ctx,
        state.cache.clone(),
        &state.config.versionstring,
        &state.config.instanceid,
    )
    .await
    .map_err(|e| OcsError::new(CoreError::Internal(anyhow::anyhow!("caps: {e}")), OcsVersion::V2, fmt.0))?;

    let envelope = OcsResponse {
        status: OcsStatus::Ok,
        message: "OK".into(),
        data: payload.body,
        version: OcsVersion::V2,
    };
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    if let Ok(etag) = HeaderValue::from_str(&payload.etag) {
        headers.insert(header::ETAG, etag);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}

#[cfg(test)]
mod tests {
    use crate::router::build_router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use rustcloud_core::AppStateBuilder;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
            bootstrap_admin: None,
        }
    }

    #[tokio::test]
    async fn capabilities_xml_default() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg).with_core_capabilities().build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/capabilities")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("application/xml"));
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("<statuscode>200</statuscode>"));
        assert!(s.contains("<pollinterval>60</pollinterval>"));
    }

    #[tokio::test]
    async fn capabilities_json_via_query() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg).with_core_capabilities().build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/capabilities?format=json")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
        assert_eq!(parsed["ocs"]["data"]["capabilities"]["core"]["pollinterval"], 60);
    }
}
```

- [ ] **Step 5: Mount the OCS sub-router**

Modify `crates/rustcloud-http/src/router.rs`:

Find the `Router::new()` chain in `build_router` and add `.nest("/ocs", crate::routes::ocs::router())` right after `.route("/index.php/login", post(login::handler))`:

```rust
    Router::new()
        .route("/status.php", get(status::handler))
        .route("/index.php/login", post(login::handler))
        .nest("/ocs", crate::routes::ocs::router())
        .with_state(state)
        ...
```

- [ ] **Step 6: Re-export ocs sub-router**

Modify `crates/rustcloud-http/src/routes/mod.rs`:

```rust
//! HTTP route modules. Each handler lives in its own file.

pub mod login;
pub mod ocs;
pub mod status;
```

- [ ] **Step 7: Run tests**

```
cargo xtask check-all
```

Expected: green. New tests this task: 3 format + 2 capabilities + 1 state. Total `rustcloud-http` around 42 tests.

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-http crates/rustcloud-core
git commit -m "feat(http,core): /ocs/v2.php/cloud/capabilities + default CoreCapabilities helper

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: CORS + tracing + catch-panic middleware

**Files:**
- Modify: `crates/rustcloud-http/src/router.rs`

Tower-http ships `TraceLayer`, `CatchPanicLayer`, and `CorsLayer`. Add them to the outer stack — CORS is permissive in dev (allow `http://localhost:8080` plus the configured trusted_domains as origins), strict in prod.

- [ ] **Step 1: Modify `build_router` to add the remaining layers**

Replace `crates/rustcloud-http/src/router.rs`:

```rust
//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to. The outermost-to-innermost layer order
//! follows spec §7.2.

use axum::http::{HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use rustcloud_core::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::csrf::CsrfLayer;
use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::{login, status};
use crate::session::{SessionLayer, SessionStore};

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();
    let secret = state.config.secret.clone();
    let cache = state.cache.clone();
    let instance_id = state.config.instanceid.clone();
    let secure_cookies = state
        .config
        .overwrite_protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    let session_store = SessionStore::new(cache, &instance_id);

    let cors_origins: Vec<HeaderValue> = trusted_domains
        .iter()
        .filter_map(|d| {
            HeaderValue::from_str(&format!("https://{d}"))
                .ok()
                .into_iter()
                .chain(HeaderValue::from_str(&format!("http://{d}")).ok())
                .collect::<Vec<_>>()
                .into_iter()
                .next()
        })
        .collect();
    let cors_layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_credentials(true)
        .allow_origin(AllowOrigin::list(cors_origins));

    Router::new()
        .route("/status.php", get(status::handler))
        .route("/index.php/login", post(login::handler))
        .nest("/ocs", crate::routes::ocs::router())
        .with_state(state)
        .layer(CsrfLayer::new())
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(SecurityHeadersLayer::new())
        .layer(cors_layer)
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}
```

- [ ] **Step 2: Run tests**

```
cargo xtask check-all
```

Expected: green; no new tests but all previous tests should still pass.

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): wire CORS, tracing, and catch-panic into the layer stack

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: Server `Cmd::Serve` runs axum + graceful shutdown

**Files:**
- Modify: `crates/rustcloud-server/Cargo.toml`
- Modify: `crates/rustcloud-server/src/main.rs`

The serve subcommand becomes real: build `AppStateBuilder::new(cfg).with_core_capabilities().build()`, hand the resulting `AppState` to `build_router`, bind to `config.bind_address`, run `axum::serve` with a graceful shutdown driven by SIGINT/SIGTERM (or Ctrl-C on Windows).

- [ ] **Step 1: Add the HTTP dep + ocs dep**

Modify `crates/rustcloud-server/Cargo.toml` — extend `[dependencies]`:

```toml
[dependencies]
anyhow.workspace = true
clap.workspace = true
rustcloud-config.workspace = true
rustcloud-core.workspace = true
rustcloud-http.workspace = true
rustcloud-ocs.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "signal"] }
tracing.workspace = true
tracing-subscriber.workspace = true
```

(`rustcloud-ocs` is needed because the server's `Cmd::Serve` references `CoreCapabilities` via the builder helper — actually the helper does the import internally. The dep can be omitted; verify after step 3. If `cargo build` flags missing imports, add it back.)

- [ ] **Step 2: Add a `shutdown_signal` helper**

Modify `crates/rustcloud-server/src/main.rs` — add this helper above `main`:

```rust
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("received Ctrl-C, shutting down"),
        _ = terminate => info!("received SIGTERM, shutting down"),
    }
}
```

- [ ] **Step 3: Replace the `Cmd::Serve` arm**

Inside `main`'s match block, replace the existing `Cmd::Serve` arm:

```rust
        Cmd::Serve => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            let bind = config.bind_address;
            info!(
                dbtype = %config.dbtype.as_str(),
                bind = %bind,
                "starting Rustcloud server"
            );

            let state = rustcloud_core::AppStateBuilder::new(config)
                .with_core_capabilities()
                .build()
                .await?;

            let router = rustcloud_http::build_router(state.clone());

            let listener = tokio::net::TcpListener::bind(bind).await?;
            let local_addr = listener.local_addr()?;
            info!(addr = %local_addr, "listening");

            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await?;

            info!("server stopped");
            state.pool.close().await;
            Ok(())
        }
```

- [ ] **Step 4: Build + smoke-test**

```
cargo build
```

Then smoke-test with a fixture and an HTTP probe in another terminal:

PowerShell:
```powershell
@'
instanceid = "phase3smoke"
secret = "a-32-byte-or-longer-secret-key!"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "phase3-smoke.db"
datadirectory = "./data"
trusted_domains = ["localhost"]
bind_address = "127.0.0.1:8765"
'@ | Out-File -Encoding utf8 fixture.toml

# Background: run the server
Start-Process cargo -ArgumentList @('run', '-p', 'rustcloud-server', '--', '--config', 'fixture.toml', 'serve')
Start-Sleep -Seconds 5

# Probe
curl http://127.0.0.1:8765/status.php

# Stop the process (PID printed by Start-Process); for simplicity just kill the cargo run process
Get-Process -Name "rustcloud-server" | Stop-Process
Remove-Item fixture.toml
Remove-Item phase3-smoke.db
```

Expected `curl` output (single line, JSON):
```json
{"installed":true,"maintenance":false,"needsDbUpgrade":false,"version":"31.0.0.0","versionstring":"31.0.0","edition":"","productname":"Nextcloud","extendedSupport":false}
```

If `rustcloud-ocs` is needed in `[dependencies]`, the smoke-test build will fail with a missing-import error; add it back to `crates/rustcloud-server/Cargo.toml` and retry.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-server Cargo.lock
git commit -m "feat(server): serve subcommand runs axum with graceful shutdown

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: End-to-end multi-endpoint integration test

**Files:**
- Create: `crates/rustcloud-http/tests/http_end_to_end.rs`

A self-contained integration test that boots the server's router (no real TCP socket), hits the three Phase-3 endpoints in sequence, and verifies the full session+CSRF+capabilities flow.

- [ ] **Step 1: Write the integration test**

Create `crates/rustcloud-http/tests/http_end_to_end.rs`:

```rust
//! End-to-end Phase 3 HTTP flow: /status.php → capabilities → login → use session.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use rustcloud_config::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
use rustcloud_core::AppStateBuilder;
use rustcloud_http::build_router;
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;
use tower::ServiceExt;

fn cfg(path: PathBuf, hash: &str) -> FileConfig {
    FileConfig {
        instanceid: "e2e".into(),
        secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
        passwordsalt: SecretString::new("ps".into()),
        installed: true,
        version: "31.0.0.0".into(),
        versionstring: "31.0.0".into(),
        dbtype: DbType::Sqlite,
        dbhost: None,
        dbport: None,
        dbname: path.to_string_lossy().into(),
        dbuser: None,
        dbpassword: None,
        dbtableprefix: "oc_".into(),
        db_pool_max: 4,
        datadirectory: PathBuf::from("/tmp"),
        trusted_domains: vec!["localhost".into()],
        trusted_proxies: vec![],
        overwrite_cli_url: None,
        overwrite_protocol: None,
        overwrite_host: None,
        loglevel: "info".into(),
        logfile: None,
        default_language: "en".into(),
        bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        cache: CacheConfig::default(),
        bootstrap_admin: Some(BootstrapAdminConfig {
            username: "admin".into(),
            password_hash: hash.into(),
        }),
    }
}

#[tokio::test]
async fn phase3_full_flow() {
    let dir = tempdir().unwrap();
    let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
    let state = AppStateBuilder::new(cfg(dir.path().join("e2e.db"), &hash))
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let app = build_router(state);

    // 1. status.php returns Nextcloud shape.
    let resp = app
        .clone()
        .oneshot(Request::builder().uri("/status.php").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 8192).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["productname"], "Nextcloud");
    assert_eq!(parsed["version"], "31.0.0.0");

    // 2. capabilities returns the core namespace.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 32 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(parsed["ocs"]["data"]["capabilities"]["core"]["pollinterval"], 60);
    assert_eq!(parsed["ocs"]["data"]["version"]["major"], 31);

    // 3. login with correct creds → 303 + Set-Cookie.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index.php/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let setc = resp.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    let cookie = setc.split(';').next().unwrap().to_string();
    assert!(cookie.starts_with("oc_sessionPassphrase="));

    // 4. login with wrong creds → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index.php/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=WRONG"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 5. capabilities again with cookie → still 200 (auth-optional route).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 6. Security headers present on status.php.
    let resp = app
        .oneshot(Request::builder().uri("/status.php").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let h = resp.headers();
    assert!(h.get("strict-transport-security").is_some(), "HSTS missing");
    assert!(h.get("x-content-type-options").is_some(), "XCTO missing");
    assert!(h.get("content-security-policy").is_some(), "CSP missing");
}
```

- [ ] **Step 2: Run the integration test**

```
cargo test -p rustcloud-http --test http_end_to_end
```

Expected: 1 test passes.

- [ ] **Step 3: Run full check**

```
cargo xtask check-all
```

Expected: green.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-http/tests
git commit -m "test(http): end-to-end Phase 3 flow — status, capabilities, login, headers

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: Phase 3 acceptance + README + changelog

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/plans/2026-05-10-platform-core-phase-3-http.changelog.md`

- [ ] **Step 1: Update README**

Replace `README.md` with the Phase 3-updated version:

```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phases 1 (Foundations), 2 (Cross-cutting), and 3 (HTTP) complete. The server actually serves HTTP now: `/status.php` returns Nextcloud-shape JSON, `/ocs/v2.php/cloud/capabilities` returns the OCS-enveloped core capabilities, and `/index.php/login` authenticates a bootstrap admin and sets a session cookie. Trusted-domain / proxy-header / security-header / CSRF / session middleware are all enforced. No UI yet (Phase 4).

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - For login, add a [bootstrap_admin] section with a bcrypt password hash.
#    Generate one with: cargo run -p rustcloud-server -- gen-admin-hash (TODO)
#    Or via Python: python -c "import bcrypt; print(bcrypt.hashpw(b'hunter2', bcrypt.gensalt(12)).decode())"

# 2a. SQLite: nothing else needed.
# 2b. MySQL or Postgres: start the dev DBs.
cargo xtask up

# 3. Run migrations.
cargo run -p rustcloud-server -- migrate

# 4. Serve.
cargo run -p rustcloud-server -- serve

# 5. Probe.
curl http://127.0.0.1:8080/status.php
curl -H "OCS-APIRequest: true" "http://127.0.0.1:8080/ocs/v2.php/cloud/capabilities?format=json"
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests (SQLite + HTTP integration)
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment).
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/rustcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/rustcloud-http` — axum router, middleware (proxy/trusted-domain/security/CSRF/session/CORS/tracing/catch-panic), session machinery, auth extractors, route handlers (`/status.php`, `/ocs/...`, `/index.php/login`).
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation.
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

Future phase: `rustcloud-ui` (Phase 4) — Dioxus Fullstack UI.

## License

AGPL-3.0-or-later.
```

- [ ] **Step 2: Write the Phase 3 changelog**

Create `docs/superpowers/plans/2026-05-10-platform-core-phase-3-http.changelog.md`:

```markdown
# Phase 3 (HTTP) — Changelog

Completed: 2026-05-10

## What works

- **`rustcloud-http`** crate with axum 0.8 router.
- **`/status.php`** returns Nextcloud-shape JSON (installed/maintenance/needsDbUpgrade/version/versionstring/edition/productname/extendedSupport).
- **`/ocs/v2.php/cloud/capabilities`** runs Phase 2's aggregator with content negotiation (XML default, JSON via `?format=json` or `Accept: application/json`); emits stable ETag.
- **`/index.php/login`** validates form credentials against `config.bootstrap_admin` via bcrypt, opens a session, rotates CSRF, redirects to `/`.
- **Session machinery**: signed cookie (HMAC-SHA256 over the 32-byte session ID keyed by `config.secret`), `oc_sessionPassphrase` cookie name (Nextcloud-compatible), cache-backed `SessionStore` with 30-minute sliding idle TTL, `HttpOnly` + `SameSite=Lax` + optional `Secure`.
- **CSRF middleware** matches Nextcloud's request-token scheme; safe methods, anonymous requests, and the `OCS-APIRequest: true` header all bypass; authenticated mutating requests require the matching `requesttoken` header.
- **Middleware stack**: `TraceLayer`, `CatchPanicLayer`, `RequestBodyLimitLayer` (512 MiB), `ProxyHeadersLayer`, `TrustedDomainLayer`, `CorsLayer`, `SecurityHeadersLayer` (HSTS, X-Content-Type-Options, Referrer-Policy, X-Frame-Options, baseline CSP), `SessionLayer`, `CsrfLayer`.
- **`AuthenticatedUser` / `OptionalUser`** axum extractors backed by the session.
- **`AppStateBuilder::with_core_capabilities()`** seeds the default core-namespace provider so the capabilities endpoint is non-empty out of the box.
- **`rustcloud-server serve`** binds to `config.bind_address`, runs `axum::serve` with `into_make_service_with_connect_info::<SocketAddr>` so peer info reaches the middleware, and shuts down gracefully on Ctrl-C / SIGTERM.

## What's deferred

- **UI surface** (Dioxus Fullstack): Phase 4.
- **Real user store** (passwords, app passwords, OAuth, LDAP, SAML, 2FA): its own sub-project.
- **Bearer / Basic / app-password auth**: deferred with the user store; only session auth resolves users today.
- **CalDAV / CardDAV / WebDAV**: their own sub-projects.
- **`gen-admin-hash` CLI subcommand**: convenience for generating a bootstrap-admin bcrypt; flagged for Phase 4 polish.
- **Absolute 24h session TTL** (spec §7.3): Phase 3 enforces idle TTL only.
- **`X-Forwarded-For` parsing into ConnectInfo**: middleware reads `X-Forwarded-Proto` and `X-Forwarded-Host` from trusted proxies; client-IP rewrite is a polish item.
- **CSP per-route override for UI**: ships in Phase 4 when the Dioxus surface lands.

## Known limitations

- Cookie name (`oc_sessionPassphrase`) is hard-coded; Phase 3 doesn't expose a `config.session.cookie_name`. Spec calls for Nextcloud-compatibility so the choice is fixed for the moment.
- `SecurityHeadersLayer` ships one CSP for everything. API responses get an over-restrictive `default-src 'none'`; that's correct for JSON/XML responses but will need a per-route override when the Dioxus UI lands.
- `ProxyHeadersLayer` honors `X-Forwarded-Proto` / `-Host` only — `X-Forwarded-For` isn't yet used to update `ConnectInfo`. Trusted-domain still works because the rewritten `Host` is what's checked.
- CSRF middleware reads the session via `SessionHandle::read().await`, which acquires a `tokio::sync::Mutex`. Under heavy contention this serializes per-session; benchmark before any production rollout.

## Known follow-ups (carried + new)

- **Centralize lint policy (`[workspace.lints]`)** — carried.
- **Sparse rustdoc on public type-level APIs** — carried; Phase 3 adds new public types (`SessionLayer`, `CsrfLayer`, `AuthenticatedUser`, etc.) — extend the doc rollout to cover them.
- **`version` subcommand should print git SHA + dialect support** (spec §10.2 / §10.5) — carried.
- **Test config-builder duplication** — now in 7+ places. Phase 3 added two more (`status.rs` tests, `login.rs` tests, `capabilities.rs` tests, `http_end_to_end.rs`). Consolidate to a `test_support` module.
- **`AppConfigService::fetch_db`** repeats query_as logic three times — the `db_dispatch!` macro from the spec lands when the first non-trivial cross-dialect query in `rustcloud-http` needs it.
- **`X-Forwarded-For` → `ConnectInfo` rewrite** for accurate downstream client-IP.
- **`gen-admin-hash` CLI subcommand** — UX polish.

## Acceptance criteria

| # | Criterion | Status |
|---|---|---|
| §13 #1 | `cargo xtask check-all` against all three backends | ✅ (carry-over) |
| §13 #3 | Binary boots + serves traffic against fresh SQLite/MySQL/Postgres | ✅ (binary serves; migrations applied via builder) |
| §13 #4 | `curl /status.php` returns Nextcloud JSON | ✅ |
| §13 #5 | `curl /ocs/v2.php/cloud/capabilities` returns valid OCS envelope | ✅ |
| §13 #7 | `/login` POST sets session cookie + redirects | ✅ |
| §13 #8 | Trusted-domain, proxy-header, CSRF, security-headers integration-tested | ✅ (verified by `http_end_to_end.rs` + per-middleware unit tests) |
| §13 #9 | Single + multi-dialect tests green in CI | ✅ (carry-over) |
| §13 #2 | `cargo xtask build` ships static binary with embedded UI | Deferred (Phase 4) |
| §13 #6 | Browser at `/` SSR'd + hydrated | Deferred (Phase 4) |
```

- [ ] **Step 3: Run the final acceptance check**

```
cargo clean
cargo xtask check-all
```

Expected: PASS end-to-end.

If Docker is available:
```
cargo xtask up
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down
```

Expected: 3 multi-dialect tests pass.

- [ ] **Step 4: Commit**

```
git add README.md docs/superpowers/plans/2026-05-10-platform-core-phase-3-http.changelog.md
git commit -m "docs: phase 3 acceptance — README + changelog

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

- [ ] **Step 5: (Optional) push to remote**

The user controls when to push. If they ask for a push after this batch:

```
git push origin master
```

---

## Phase 3 Self-Review (executor verifies before declaring complete)

Run through each spec §13 in-scope criterion:

| Criterion | Verified by |
|---|---|
| `/status.php` returns Nextcloud JSON | `crates/rustcloud-http/src/routes/status.rs::tests::status_returns_nextcloud_shape` + `http_end_to_end.rs` step 1 |
| `/ocs/v2.php/cloud/capabilities` returns OCS envelope | `crates/rustcloud-http/src/routes/ocs/capabilities.rs::tests` (XML + JSON) + `http_end_to_end.rs` step 2 |
| `/login` sets session cookie | `crates/rustcloud-http/src/routes/login.rs::tests` + `http_end_to_end.rs` step 3 |
| Trusted-domain rejection | `crates/rustcloud-http/src/middleware/trusted_domain.rs::tests` |
| Proxy header rewriting | `crates/rustcloud-http/src/middleware/proxy_headers.rs::tests` |
| CSRF enforcement | `crates/rustcloud-http/src/csrf.rs::tests` |
| Security headers present | `crates/rustcloud-http/src/middleware/security_headers.rs::tests` + `http_end_to_end.rs` step 6 |
| Multi-dialect tests green | CI on push |

If any of those have not been touched by a test, fix before declaring complete.

---
