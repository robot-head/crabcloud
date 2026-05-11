# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phases 1 (Foundations) and 2 (Cross-cutting) complete. The binary can boot, load layered config, connect to SQLite/MySQL/Postgres, assemble a full `AppState` (DbPool + Cache + I18n + AppConfig + capability providers), run core migrations, and exit. No HTTP surface yet; Phase 3 adds it.

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 1. Copy the example config and edit it.
cp config/config.toml.example config/config.toml
# (Set `installed = true` and pick a `dbtype`.)

# 2a. SQLite: nothing else needed. Skip to step 3.

# 2b. MySQL or Postgres: start the dev DBs.
cargo xtask up

# 3. Run migrations.
cargo run -p rustcloud-server -- migrate
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests (SQLite only)
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down          # stop dev DBs
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment-based).
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum over Sqlite/MySql/Postgres, `MigrationRunner`.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n` service.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), `CapabilityProvider` aggregator with cache-backed ETag.
- `crates/rustcloud-core` — `AppState`, `AppConfigService`, `Error`, `BootstrapHook`.
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

Future phases add `rustcloud-http` (Phase 3) and `rustcloud-ui` (Phase 4).

## License

AGPL-3.0-or-later.
