# Phase 5 (Ship) — Changelog

Completed: 2026-05-11

## What works

- **Test fixture consolidation**: `crabcloud-config::test_support::minimal_sqlite_config` (behind a `test-support` feature) replaces ~10 hand-rolled `cfg_sqlite` copies across the workspace. Adding a new `FileConfig` field now means changing one helper, not ten.
- **Centralized lint policy**: `[workspace.lints]` at the root + `[lints] workspace = true` in every member. `unused_crate_dependencies = "warn"` surfaces dep drift via plain `cargo check`.
- **`crabcloud-core` cleanup**: drops unused `async-trait` / `serde` / `serde_json`. `tracing` now carries its weight via `AppConfigService` instrumentation.
- **`AppConfigService` instrumentation**: cache `set` / `del` failures now emit `tracing::warn!` with structured fields (error, appid, key). `CACHE_TTL` lifted to a module constant.
- **`version` subcommand expansion**: prints crate version, git SHA (via `vergen-gix`), supported dialects, sub-project marker. Closes spec §10.2 / §10.5 acceptance.
- **Content-type-aware CSP**: `SecurityHeadersLayer` inspects the response `Content-Type` and ships the UI-permissive CSP (`script-src 'self' 'wasm-unsafe-eval'`) for HTML and the API-restrictive `default-src 'none'` for everything else. **Unblocks WASM hydration**.
- **Dioxus router 404 status**: SSR handler parses the request path through `Route::from_str` and returns HTTP 404 when the resolved variant is `NotFoundRoute`. The body is still the Dioxus-rendered 404 page; the status finally matches.
- **Hydration marker**: `App` component wraps its content in `<div id="app-root" data-hydrated="...">`. SSR emits `"false"`; `use_effect` on the WASM client flips it to `"true"`. The Playwright E2E waits on this transition.
- **Playwright E2E** (`e2e/`): three tests against a real Chromium — SSR snapshot + hydration, login-flow + authenticated greeting, 404 status. CI job builds the release server, writes a fixture config with a bcrypt admin hash, runs Playwright, tears down. **Verifies spec §13 #6 end-to-end.**
- **Rustdoc rollout**: every public type/field/variant/method across the workspace has a one-line summary; load-bearing types keep their existing fuller docs.
- **`CONTRIBUTING.md`**: MSRV (1.85), tooling versions (Dioxus 0.6, Node 20), workflow commands, CI layout, commit conventions.

## What's deferred (post-platform-core)

These are explicitly *not* in scope for the platform-core program; they belong to specific feature sub-projects:

- **Real user store**: Bearer auth, Basic auth, app passwords, OAuth2 server, LDAP, SAML, 2FA, constant-time username comparison. The `bootstrap_admin` stand-in handles Phase 4-5 demo needs.
- **Server functions** (`#[server]`): Phase 4 deliberately routed all auth-bearing operations through the OCS API surface for cross-client compatibility. Server functions land per-feature when a UI-only RPC actually needs them.
- **WebDAV / CalDAV / CardDAV**: own sub-projects.
- **File sharing**: own sub-project.
- **Background job runner**: own micro-sub-project.
- **Redis cache backend**: `Cache` trait is ready; implementation lands before multi-node deploy.
- **App / plugin framework lifecycle hooks** beyond `BootstrapHook`: settings UI registration, dependency resolution, navigation entries, etc.
- **Internationalization wiring into UI components**: Home/Login still render English inline.
- **Public share landing** (`/s/<token>`): stub in Route enum, not implemented.
- **Theming engine**.

## Known limitations

- `data-hydrated` marker depends on `use_effect` not running during SSR. Dioxus 0.6 semantics agree, but if a future Dioxus upgrade changes that the E2E test will break loud (good — that's the signal).
- CSP `'wasm-unsafe-eval'` allows the WASM bundle to instantiate but doesn't permit `eval()` or `Function(string)` from JS. Acceptable for Dioxus; revisit if a future asset needs a wider exception.
- Playwright E2E uses `kill $PID` for teardown — if the server hangs, the CI step may need a `timeout` wrapper. Not seen in practice.

## Spec §13 acceptance criteria — final status

| # | Criterion | Status | Verified by |
|---|---|---|---|
| 1 | `cargo xtask check-all` against all three backends | GREEN | CI workflow + multi-dialect job |
| 2 | `cargo xtask build` produces a static binary with embedded UI assets | GREEN | `xtask build` task + `rust-embed` |
| 3 | Binary boots + migrates + serves against fresh SQLite/MySQL/Postgres | GREEN | `migrate_end_to_end` integration tests + serve subcommand |
| 4 | `curl /status.php` returns Nextcloud JSON | GREEN | `http_end_to_end.rs` + `routes::status` tests |
| 5 | `curl /ocs/v2.php/cloud/capabilities` returns OCS envelope | GREEN | `routes::ocs::capabilities` tests + `http_end_to_end.rs` |
| 6 | Browser at `/` SSR'd + hydrated | **GREEN** | Playwright E2E `hydration.spec.ts` — verifies in real Chromium |
| 7 | `/login` POST sets session cookie + redirects | GREEN | `routes::login` tests + E2E login flow test |
| 8 | Middleware enforcement integration-tested | GREEN | per-middleware unit tests + `http_end_to_end.rs` |
| 9 | Single + multi-dialect tests green in CI | GREEN | CI workflow |

Platform-core is complete.
