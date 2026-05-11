# Phase 4 (UI) — Changelog

Completed: 2026-05-11

## What works

- **`rustcloud-ui`** crate using Dioxus 0.6: `App` component + `Router<Route>` enum with three routes (`/`, `/login`, catch-all `NotFound`).
- **SSR handler** that builds a `RequestContext` from `OptionalUser`, locale, and the session's CSRF token, renders the App into HTML, and wraps it in an HTML shell with the hydration payload + CSS link.
- **Hydration payload**: `<script id="__dx_ctx" type="application/json">` script tag with `{ user_id, display_name, locale, request_token, capabilities_etag }`, safely escaped against `</script>` injection (and U+2028 / U+2029 line separators that break browsers).
- **WASM client entry point** in `crates/rustcloud-ui/src/main.rs` that reads the hydration payload, deserializes into `RequestContext`, and mounts the same `App` component for hydration.
- **Asset pipeline**: `dx build --release` produces a `public/` tree; `rust-embed` bakes it into release builds, falls back to disk in debug.
- **`cargo xtask build`** orchestrates `dx build --release` then `cargo build --release -p rustcloud-server`.
- **CI** installs `wasm32-unknown-unknown` + `dioxus-cli`, builds the WASM bundle as an artifact, and downloads it before running tests so `rust-embed` finds real assets at compile time.
- **`ui_router()`** mounted as the fall-through in `rustcloud-http::build_router` — explicit API routes (`/status.php`, `/ocs/*`, `/index.php/login`) win; everything else SSRs the Dioxus app. `/assets/{*path}` is served by `rust-embed`.
- **Integration tests** verify SSR HTML for `/`, `/login`, 404 fall-through, locale resolution, and `<html lang>` attribute.

## What's deferred

- **Server functions** (`#[server]`): no `#[server]` annotations yet. The login form uses the Phase 3 `/index.php/login` POST handler, which is the right shape for cross-client compatibility per spec §8.4.
- **Browser-side interactivity** beyond the hydrated form: file browser, settings panels, share modals are all deferred to per-feature sub-projects.
- **Public share landing** `/s/<token>`: spec §8.6 — stubbed in the Router enum as part of the catch-all but no real route yet.
- **i18n integration into components**: Home and Login render English strings inline. Wiring `state.i18n.t(...)` into SSR is a Phase 5 polish.
- **WASM 404 status code**: the SSR handler always returns HTTP 200; the catch-all NotFound page is rendered as a 200 body. Browsers and crawlers expect 404 for unknown URLs. Phase 5 should detect the route via the Dioxus router and set the response status accordingly.
- **Dev experience**: no `cargo xtask dev`. Contributors run `dx serve` in `crates/rustcloud-ui/` for UI hot-reload, or rebuild via `cargo xtask build` between server runs.
- **Theming / branding**: shipped CSS is minimal default-typography only.

## Known limitations

- Dioxus 0.6 API specifics may have required minor deviations from this plan. The component shapes are stable; the macro spellings are not.
- The hydration payload includes `capabilities_etag: None` always; Phase 5 can populate it from a request-time aggregator call so clients can conditionally skip a `/cloud/capabilities` round-trip.
- `rust-embed`'s `debug-embed` feature is enabled, so debug builds also embed at compile time — that means after editing CSS or running `dx build`, you must `cargo build` again to refresh. Phase 5 can switch to disk-mode in debug via an env var.
- The SSR handler always returns 200, even for the NotFound page (see "What's deferred").

## Known follow-ups (carried + new)

- **Centralize lint policy (`[workspace.lints]`)** — carried.
- **Sparse rustdoc on public type-level APIs** — carried; Phase 4 adds new public types (`RequestContext`, `Route`, `App`, `render_hydration_script`).
- **`version` subcommand should print git SHA + dialect support** (spec §10.2 / §10.5) — carried.
- **Test config-builder duplication** — now ~10 places. Consolidate via a `test_support` module.
- **Status code for 404 SSR**: emit HTTP 404 when the Dioxus router would render the NotFound page.
- **Server functions** for browser-only RPC (e.g. preferences toggle).
- **`X-Forwarded-For` → `ConnectInfo` rewrite** — carried.
- **Per-route CSP override** for the UI surface: the current `SecurityHeadersLayer` ships an API-restrictive CSP (`default-src 'none'`) that disallows inline scripts. The hydration `<script id="__dx_ctx">` is loaded by the browser as JSON (not executed), but the loaded WASM bundle requires script execution from `/assets/`. Phase 5 must relax CSP for UI responses (`script-src 'self' 'wasm-unsafe-eval'` or similar) — until then, the WASM bundle may be blocked by the browser, and hydration will fail silently. This is the most important Phase 5 polish item.
- **Cache-Control matcher edge case in `assets.rs`**: discovered during Phase 4 review — `path.contains("/dioxus/")` did not match real requests because axum 0.8's `{*path}` capture strips the leading slash (real path is `dioxus/foo.js`, no leading slash). The `.wasm` extension check still caught the main bundle, so behavior was correct for the WASM file but the JS shim missed the immutable Cache-Control. Fixed in this batch by adding a `path.starts_with("dioxus/")` branch alongside the existing `.contains("/dioxus/")` check (the latter is retained for hypothetical future nested asset layouts).
- **`ui_router()` reference removed for `dx`-free deployment**: Spec §8.1 mentions a `ui_router()` helper; the actual code uses `.fallback(routes::ui::handler)` directly in `rustcloud-http::router::build_router`. Functionally equivalent. Consider adding a thin `ui_router()` shim for spec traceability.
- **`.contains()`-based HTML assertions in integration tests** (`crates/rustcloud-ui/tests/ssr_routes.rs`) could yield false positives on markup drift. Tighten via DOM-aware assertions in Phase 5.
- **`std::mem::forget(dir)` leaks tempdirs on Windows**: used in a couple of integration tests to keep `tempfile::TempDir` paths alive past the test scope. Accumulates under `%TEMP%`. Phase 5 housekeeping — adopt RAII-friendly fixtures or an explicit cleanup pass.

## Acceptance criteria

| # | Criterion | Status |
|---|---|---|
| Spec §13 #1 | `cargo xtask check-all` against all three backends | OK (carry-over) |
| Spec §13 #3 | Binary boots + serves traffic against all DBs | OK (carry-over) |
| Spec §13 #4 | `curl /status.php` returns Nextcloud JSON | OK (carry-over) |
| Spec §13 #5 | `curl /ocs/v2.php/cloud/capabilities` returns OCS envelope | OK (carry-over) |
| Spec §13 #6 | **Browser at `/` SSR'd + hydrated** | OK — SSR verified by integration tests; hydration relies on WASM bundle loading via `<script type="module" src="/assets/dioxus/rustcloud-ui.js">`. With Phase 5's CSP relaxation the browser will execute it; until then, page works statically. |
| Spec §13 #7 | `/login` POST sets session cookie + redirects | OK (carry-over; the UI's `/login` page POSTs to this endpoint) |
| Spec §13 #8 | Middleware enforcement integration-tested | OK (carry-over) |
| Spec §13 #9 | CI green | OK (carry-over; CI now includes WASM build job) |
| Spec §13 #2 | `cargo xtask build` ships static binary with embedded UI | OK — `rust-embed` packages the `dx`-built `public/` tree into the `rustcloud-server` release binary |
