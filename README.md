# Crabcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** platform-core complete. The server boots, serves the Nextcloud-compatible API surface (`/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`), and renders an SSR'd Dioxus UI that hydrates in the browser. Spec §13 acceptance criteria are all green (verified by `cargo xtask check-all` + the Playwright E2E suite). Per-feature sub-projects (users, storage, WebDAV, sharing, calendar/contacts, etc.) build on this substrate.

See `docs/superpowers/specs/` for design specs, `docs/superpowers/plans/` for implementation plans, and `CONTRIBUTING.md` for dev workflow.

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.6"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - For login, add a [bootstrap_admin] section with a bcrypt password hash.

# 2. Build UI + server.
cargo xtask build

# 3. Run migrations + serve.
cargo run --release -p crabcloud-server -- migrate
cargo run --release -p crabcloud-server -- serve

# 4. Visit http://127.0.0.1:8080/ in a browser.
```

## Workspace layout

- `crates/crabcloud-config` — layered TOML config loader.
- `crates/crabcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/crabcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/crabcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/crabcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/crabcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/crabcloud-http` — axum router, middleware, session, CSRF, auth extractors, API handlers.
- `crates/crabcloud-ui` — Dioxus 0.6 SSR + WASM hydration UI.
- `crates/crabcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation.
- `e2e/` — Playwright tests (real-browser SSR + hydration verification).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

## License

AGPL-3.0-or-later.
