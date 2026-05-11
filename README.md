# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phases 1 (Foundations), 2 (Cross-cutting), 3 (HTTP), and 4 (UI) complete. The server serves an SSR'd Dioxus UI at `/` and `/login`, alongside Phase 3's Nextcloud-compatible API surface (`/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`). Browser visits to `/` get a fully-rendered HTML page with a hydration payload; the WASM client (compiled by `dx`) takes over for interactivity.

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.6"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - Optional: add [bootstrap_admin] with a bcrypt password_hash for /login.

# 2. Build the UI assets + server.
cargo xtask build

# 3a. SQLite: run migrations and serve.
cargo run --release -p rustcloud-server -- migrate
cargo run --release -p rustcloud-server -- serve

# 3b. MySQL or Postgres: start the dev DBs.
cargo xtask up
cargo run --release -p rustcloud-server -- migrate
cargo run --release -p rustcloud-server -- serve

# 4. Visit http://127.0.0.1:8080/ in a browser.
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests
cargo xtask build         # dx build + cargo build --release
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down

# Iterating on UI components:
#   - Edit crates/rustcloud-ui/src/...
#   - Run `dx serve` in crates/rustcloud-ui/ for hot-reload (browser-only)
#   - OR re-run `cargo xtask build && cargo run --release -p rustcloud-server -- serve`
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment).
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/rustcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/rustcloud-http` — axum router, middleware, session, CSRF, auth extractors, API handlers.
- `crates/rustcloud-ui` — Dioxus 0.6 SSR + WASM hydration UI (`/`, `/login`, 404).
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `build`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

Platform core is now complete; Phase 5 (test scale-out + ship) is the next milestone. After that, app sub-projects (users, storage, WebDAV, sharing, ...) build on this substrate.

## License

AGPL-3.0-or-later.
