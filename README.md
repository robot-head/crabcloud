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
