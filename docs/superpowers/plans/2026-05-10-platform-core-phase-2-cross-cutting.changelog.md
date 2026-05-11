# Phase 2 (Cross-cutting) — Changelog

Completed: 2026-05-10

## What works

- **`rustcloud-cache`**: `Cache` trait, `MemoryCache` impl with lazy TTL expiry, `TypedCache<T>` serde wrapper.
- **`rustcloud-i18n`**: `Locale` type with Accept-Language resolution, `Catalog` loader for gettext `.po` files (polib), `I18n` service with `t()`/`tn()` and source-string fallback. Seed `l10n/core/de.po` provided.
- **`rustcloud-ocs`**: `OcsResponse<T>` envelope rendering to JSON or XML; `Format` content negotiation; `CapabilityProvider` trait + cache-backed aggregator with stable ETag; `CoreCapabilities` provider matching Nextcloud's `core` namespace shape.
- **`rustcloud-core`**: `Error` enum with HTTP status mapping + client-safe message extraction; `AppConfigService` (cache-write-through against `oc_appconfig`); `BootstrapRegistry` + `BootstrapHook` — the extension point future apps will use; `AppState` + `AppStateBuilder` that assembles everything end-to-end.
- **`rustcloud-server`**: `migrate` subcommand now uses `AppStateBuilder::build()`, proving the assembly path.

## What's deferred

- HTTP surface (axum router + middleware + session + CSRF + `status.php` + login + OCS routes): Phase 3.
- Dioxus Fullstack UI: Phase 4.
- App/plugin framework (lifecycle hooks beyond `BootstrapHook`, dependency resolution, settings page registration): later sub-project.
- Redis cache backend: micro-sub-project before multi-node deploy.
- Background job runner / cron.

## Known limitations

- `MemoryCache` TTL expiry is lazy on read (no background sweeper). Acceptable for single-node; multi-node deploys will use Redis.
- The OCS XML serializer is hand-rolled tree-walk rather than `quick-xml::Serializer` — easier to control the exact element shape clients expect.
- `I18n` uses the simple English plural rule (`n != 1`). Full plural-form expression support is deferred.
- `Locale` normalizes `en-US` → `en_us`; some external systems use Nextcloud's pre-normalized form (`en_US`) — flag if clients complain.

## Known follow-ups (carried from Phase 1 + new from Phase 2)

- Centralize lint policy (`[workspace.lints]`). Carried.
- Sparse rustdoc on public type-level APIs. Carried; partly addressed for new types.
- `version` subcommand should print git SHA + dialect support (spec §10.2 / §10.5). Carried.
- Test config-builder duplication (`cfg_sqlite`/`base_config`) — now in 5 places. Consolidate before Phase 3.
- `quick-xml` is declared as a workspace dep but the current XML rendering is hand-rolled; either keep the dep for future XML parsing needs or drop it.
- `compute_etag` in `rustcloud-ocs::capabilities` uses `DefaultHasher`, which is documented as not stable across Rust versions. Acceptable for an ETag (clients re-fetch on mismatch) but worth swapping for `blake3` or `xxhash-rust` if a stable cross-version hash matters.
- `AppConfigService::fetch_db` repeats the same `query_as` body three times for the three pool variants. Phase 3 introduces the `db_dispatch!` macro mentioned in the spec; this is its first natural use site.
- **`rustcloud-core` has unused workspace deps**: `async-trait`, `tracing`, `serde`, `serde_json` are declared but not referenced by any source file. Drop them, or — better — instrument `AppConfigService::set`/`get` cache failures with `tracing::warn!` to actually use `tracing`.
- **Two semantic paths for the same validation error.** `LoadError::Validate(FileConfigError)` and `Error::ConfigValidation(FileConfigError)` are both reachable for the same underlying error; logs/Display will differ depending on which path the `?` operator takes. Either collapse `Error::ConfigValidation` (it's already reachable via `Error::Config`) or document which path to prefer.
- **OCS aggregator: TTL is a magic 60s; cache-set failures swallowed silently.** Lift to a `const CACHE_TTL` (done for rustcloud-ocs in Batch C; AppConfigService TTL still hardcoded). Add `tracing::warn!` on cache-set failure paths.
- **`xml_escape` allocates 5x per call.** Hot-path-only concern; Phase 3 may surface it.
- **`Format::negotiate` is a naive substring search**, doesn't honor Accept header q-weights. Nextcloud clients send simple Accept values so it's fine in practice — flag if a third-party client breaks.
- **`Locale` normalization is one-way** (`en-US` → `en_us`). If we ever emit Content-Language headers, we'll need `to_bcp47()`.
- **OCS XML array encoding** uses `<element>...</element>` per item. Verify against real Nextcloud client wire dumps before Phase 3 lands handlers.
- **Test config-builder duplication** is now in 5 places (`pool.rs`, `migrate.rs`, `core_migrations.rs`, `migrate_end_to_end.rs`, `state.rs` tests, `appconfig.rs` tests, `app_state_build.rs`). Consolidate into a shared `test_support` module before Phase 3.
- **`AppConfigService::fetch_db`** repeats query_as logic three times across pool variants. Phase 3's `db_dispatch!` macro will deduplicate.

## Acceptance criteria

| # | Criterion | Status |
|---|---|---|
| Phase 1 #1 | `cargo xtask check-all` against all three backends | ✅ (carry-over) |
| Phase 1 #3 | Binary boots + migrates against all three DBs | ✅ (carry-over; now via `AppStateBuilder`) |
| Phase 1 #9 | Single + multi-dialect tests green | ✅ (carry-over) |
| Phase 2 (a) | Cache trait + memory impl unit-tested | ✅ |
| Phase 2 (b) | I18n catalog loader + service unit-tested | ✅ |
| Phase 2 (c) | OCS envelope renders JSON and XML correctly | ✅ |
| Phase 2 (d) | Capabilities aggregator works in isolation (cached, ETag-keyed) | ✅ |
| Phase 2 (e) | `AppStateBuilder::build()` integration test proves end-to-end assembly | ✅ |
| Spec ac §13 #4 | `/status.php` | Deferred to Phase 3 |
| Spec ac §13 #5 | `/ocs/v2.php/cloud/capabilities` | Endpoint wiring deferred to Phase 3; aggregator works in isolation today |
| Spec ac §13 #6, 7, 8 | Browser/login/middleware | Deferred to Phases 3-4 |
