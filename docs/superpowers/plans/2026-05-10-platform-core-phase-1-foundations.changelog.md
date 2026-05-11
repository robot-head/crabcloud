# Phase 1 (Foundations) — Changelog

Completed: 2026-05-10

## What works

- Cargo workspace with `rustcloud-config`, `rustcloud-db`, `rustcloud-server`, `xtask`.
- Layered config: TOML base + `config.local.toml` overlay + `RUSTCLOUD_*` env vars + CLI overrides. Sensitive fields use `secrecy::SecretString`.
- `DbPool` enum over `sqlx::SqlitePool` / `MySqlPool` / `PgPool`. `connect()` dispatches on `config.dbtype`.
- `MigrationRunner` with namespace tracking (`oc_migrations`). Idempotent re-runs. Per-dialect SQL.
- Core migration 0001 creates `oc_appconfig` matching Nextcloud's shape across all three dialects.
- `rustcloud-server` subcommands: `version`, `migrate`, `serve` (stubbed).
- CI: fmt + clippy + SQLite tests + multi-dialect tests against GitHub Actions service containers.
- `cargo xtask` commands: `check-all`, `up`, `down`.

## What's deferred

- HTTP surface: Phase 3 (`rustcloud-http`).
- UI: Phase 4 (`rustcloud-ui` + Dioxus Fullstack).
- Cache trait + memory impl: Phase 2.
- i18n loader: Phase 2.
- OCS envelope + capabilities: Phase 2.
- AppState facade: Phase 2.
- `cargo xtask prepare` / `dev` / `build`: filled in as later phases need them.

## Known limitations

- `MigrationRunner` doesn't wrap migration SQL in a transaction (DDL portability issues across MySQL). Rely on idempotent SQL (`CREATE TABLE IF NOT EXISTS`, etc.) for safety.
- The migration runner splits SQL on `;` naively; migration files must not contain semicolons inside string literals or comments.
- No offline sqlx cache yet — no `sqlx::query!` macros used in Phase 1. Phase 2 introduces the first compile-time-checked queries and the `cargo xtask prepare` flow.

## Known follow-ups

Minor polish items deferred from earlier batches — tracked here so they aren't lost:

- **Cargo.lock dependency pinning** (Batch C): `serde_with`, `home`, `url`, and `idna` were downgraded to satisfy the Rust 1.85.0 MSRV. This is undocumented in-tree and will surface for the next contributor who runs `cargo update`. Consider adding a comment in `Cargo.toml` or a short note in `docs/` explaining the pins.
- **Test config-builder duplication** (Batches C and E): `cfg_sqlite` / `base_config` helpers appear in roughly 4 places across the test suites. Consolidation opportunity — extract a shared `rustcloud-config::test_support` or `dev-dependencies` helper crate.
- **CI workflow polish** (Batch E):
  - SHA-pin third-party GitHub Actions instead of relying on moving tags (`@v4`, etc.) for supply-chain hardening.
  - Expand the `test-sqlite` job to also run the SQLite integration test (`migrate_end_to_end migrate_sqlite`), not just unit tests.
  - Add a Windows runner to the test matrix to catch path/line-ending issues earlier.
- **xtask error messages** (Batch D): `xtask` shells out to `docker` and `cargo` without checking PATH; failures are opaque if the binary isn't installed. Wrap with a friendlier "did you install docker?" hint.
- **`MigrationRunner::applied()` side effect** (Batch C): the method creates the `oc_migrations` tracking table as a side effect of querying it. Currently undocumented — add a rustdoc note so callers aren't surprised.
- **`execute_multi` comment handling** (Batch C): the naive `;`-split doesn't handle SQL `--` line comments. Fine for the migrations we control today, but worth tightening before accepting third-party migration files.
- **`DbError::Migration` chaining** (Batch C): the `Migration` variant takes a `String` and doesn't chain the underlying `sqlx::Error` as a `#[source]`. Loses the cause chain for downstream consumers.
