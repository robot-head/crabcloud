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
| §13 #1 | `cargo xtask check-all` against all three backends | OK (carry-over) |
| §13 #3 | Binary boots + serves traffic against fresh SQLite/MySQL/Postgres | OK (binary serves; migrations applied via builder) |
| §13 #4 | `curl /status.php` returns Nextcloud JSON | OK |
| §13 #5 | `curl /ocs/v2.php/cloud/capabilities` returns valid OCS envelope | OK |
| §13 #7 | `/login` POST sets session cookie + redirects | OK |
| §13 #8 | Trusted-domain, proxy-header, CSRF, security-headers integration-tested | OK (verified by `http_end_to_end.rs` + per-middleware unit tests) |
| §13 #9 | Single + multi-dialect tests green in CI | OK (carry-over) |
| §13 #2 | `cargo xtask build` ships static binary with embedded UI | Deferred (Phase 4) |
| §13 #6 | Browser at `/` SSR'd + hydrated | Deferred (Phase 4) |
