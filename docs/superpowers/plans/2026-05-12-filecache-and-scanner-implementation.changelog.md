# Sub-project 4b — Changelog

Completed: 2026-05-12

## What works

- New `crabcloud-filecache` crate (DB-backed cache + scanner).
- Migration `0004_filecache` creates `oc_storages` + `oc_mimetypes` + `oc_filecache` on sqlite/mysql/postgres (3 tables, 5 indexes, 4 FKs).
- `ChannelEventSink` in `crabcloud-storage` wraps `tokio::sync::broadcast` (default capacity 1024).
- `FileCache::apply` handles `Written`/`DirCreated`/`Deleted`/`Moved`/`Copied`. Each handler runs leaf mutation + ancestor walk in one DB transaction. Directory moves rewrite all descendant paths.
- Cache-miss `stat`/`list` populate through real-backend stat under per-path mutex: 100 concurrent stats for one path → 1 backend hit; distinct paths parallelize.
- `Scanner` continuous consumer applies events; `full_scan` walks a storage top-down for drift recovery; `RecvError::Lagged` triggers full-scan of every registered storage.
- `files:scan <storage_id>` CLI subcommand in `crabcloud-server`.
- `[filecache] enabled = true, event_channel_capacity = 1024` block in `crabcloud-config`.
- `AppState` gains `storage_sink`/`filecache`/`scanner` fields; `AppStateBuilder` spawns the scanner when `enabled = true`.

## What's deferred

- **S3 backend** — sub-project **4b-S3** (separate brainstorming). Prep notes at `docs/superpowers/specs/2026-05-12-filecache-and-scanner-design.followup-4b-s3.md`.
- **Mount composition / View layer** — sub-project **4c**.
- **Chunked-upload protocol translation** — sub-project **4c**.
- **WebDAV / HTTP routes** — sub-project **5**.
- **Trash, versions, WebDAV LOCK/UNLOCK** — separate later sub-projects.
- **Server-side encryption hooks** — separate later sub-project.
- **Sharing-aware permissions composition** — 4c + sharing sub-project.
- **Negative caching** — 4b doesn't remember NotFound results.
- **Parallel apply** — single-consumer; events apply in order.
- **`oc_filecache.parent` integrity audit** — there's no scrubber to detect orphan rows (parent points to a non-existent fileid).

## Known limitations

- **`oc_filecache.path` is capped at 4000 chars** (MySQL/Postgres VARCHAR limit before index-width concerns). `StoragePath::new` caps at 4096; gap is 96 chars. Operators with deeper paths must wait for a future VARCHAR widening or switch to TEXT-typed columns.
- **External edits between scans** are not visible until next `files:scan`. Documented for operators.
- **Migration version is 4** (not 3 as the spec said — `0003_auth_tokens` already exists on master).
- **Cross-storage moves** require `Storage::rename` to be on the same storage; 4c's View layer will add cross-storage copy+delete.
- **Per-path lock map** grows monotonically with opportunistic cleanup; bounded eviction is a future hardening.
- **Test suites use SQLite-only fixtures**; multi-dialect coverage runs in CI via `cargo xtask check-all`.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean (sqlite/mysql/postgres) | OK (CI) |
| 2 | Migration `0004_filecache` creates 3 tables + 5 indexes + FKs | OK (`tests/apply_events.rs` + `core_migration_applies_against_sqlite`) |
| 3 | `ChannelEventSink` is `EventSink`; capacity 1024 default | OK (`crates/crabcloud-storage/src/lib.rs::channel_sink_tests`) |
| 4 | Written event inserts leaf with correct mimetype/size/etag/permissions | OK (`tests/apply_events.rs::apply_written_event_inserts_leaf_with_metadata`) |
| 5 | Ancestor size + etag propagation atomic | OK (`tests/apply_events.rs::apply_propagates_size_and_etag_up_chain`) |
| 6 | Cache-miss populate serializes per-path (100 → 1 backend hit) | OK (`tests/populate.rs::stat_cache_miss_concurrent_populates_once`) |
| 7 | Cache-miss populate parallelizes across paths | OK (`tests/populate.rs::stat_cache_miss_distinct_paths_run_in_parallel`) |
| 8 | Scanner consumes broadcast events | OK (`tests/scanner.rs::scanner_consumes_written_events_into_cache`) |
| 9 | Full-scan reconciles external drift | OK (`tests/scanner.rs::scanner_full_scan_reconciles_external_writes`) |
| 10 | `RecvError::Lagged` triggers full-scan recovery | OK (`tests/scanner.rs::scanner_lagged_triggers_full_scan_recovery`) |
| 11 | `files:scan` CLI runs full-scan | OK (smoke + Batch E wiring) |
| 12 | Deleted directory cascades descendants via FK | OK (`tests/apply_events.rs::apply_deleted_cascades_descendants_and_decrements_size`) |
| 13 | Moved row updates fields + descendant paths + propagates ETag both chains | OK (`tests/apply_events.rs::apply_moved_directory_rewrites_descendant_paths` + `apply_moved_across_parents_shifts_size_and_bumps_both_etags`) |
| 14 | Workspace `-D warnings` clean | OK (CI) |
| 15 | `git grep -i rustcloud` empty | OK |
