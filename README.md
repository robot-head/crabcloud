# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phase 1 (Foundations) complete. The binary can boot, load layered config, connect to SQLite/MySQL/Postgres, run core migrations, and exit. No HTTP surface yet; later phases add it.

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
- `crates/rustcloud-db` — `DbPool` enum over Sqlite/MySql/Postgres, `MigrationRunner`.
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.

Future phases add `rustcloud-cache`, `-i18n`, `-ocs`, `-core`, `-http`, `-ui`.

## License

AGPL-3.0-or-later.
