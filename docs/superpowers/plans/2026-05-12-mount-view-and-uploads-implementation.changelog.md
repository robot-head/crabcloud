# Sub-project 4c — Changelog

Completed: 2026-05-12

## What works

- New `crabcloud-fs` crate (per-user filesystem façade; no HTTP/DB deps).
- `UserPath` newtype: leading-`/` required, ≤4096 chars, rejects `..`/`.`/NUL/backslash/empty segments. Trailing slash stripped (except root).
- `Mount { path_prefix: StoragePath, storage: Arc<dyn Storage> }` + `MountResolver` trait + `StorageFactory` trait. Forward-designed for share + external mounts.
- `HomeMountResolver` returns one mount per user, anchored at root.
- `LocalStorageFactory` constructs `<data_dir>/<uid>/files` (creates the directory if missing).
- `View` façade: `stat`/`list`/`read`/`read_range`/`put_file`/`mkdir`/`delete`/`rename`/`copy`. Reads route through `FileCache`; writes emit through `ChannelEventSink`. Within-mount rename/copy succeed; cross-mount errors `FsError::CrossMount`.
- `Uploads` façade: `begin`/`put_part`/`abort`/`commit`. Opaque self-describing `upload_id` encodes `(path_prefix, dest_path, backend_upload_id)` as URL-safe base64. No DB table; resumable across server restarts as long as backing-storage multipart state survives.
- `AppState` gains `mount_resolver: Arc<dyn MountResolver>` field + `view_for(uid)` / `uploads_for(uid)` factory methods. `AppStateBuilder::build` wires `HomeMountResolver` over `LocalStorageFactory` using `config.datadirectory`.

## What's deferred

- **WebDAV / HTTP routes** — sub-project **5**.
- **Share mounts** — sharing sub-project (layers an additional resolver).
- **External storage mounts** — separate later sub-project.
- **Cross-mount rename/copy** — currently errors `FsError::CrossMount`. Relaxed when share mounts arrive.
- **Trash, versions, WebDAV LOCK/UNLOCK** — separate later sub-projects.
- **Encryption hooks** — separate later sub-project.
- **Quota enforcement** — separate sub-project.
- **`uploads:gc` CLI** to reap stale multiparts — a future sub-project.
- **Mount caching on AppState** — currently each `view_for` re-resolves; revisit when share mounts are added.

## Known limitations

- **Spec said `[storage] data_dir`**, but `FileConfig.datadirectory: PathBuf` already existed on master with identical semantics. The implementation uses `datadirectory` — no new config block.
- **`upload_id` length** can reach ~5500 chars worst-case for deep paths (UserPath caps at 4096; base64 4/3 inflation; plus backend id). Most clients support 8 KB URIs; document for operators.
- **Scanner lag** between `View::put_file` returning and the filecache being updated. Mitigation: `View::put_file` returns the storage's fresh `FileMetadata` directly so callers don't need to wait. Tests that need cache state explicitly use bounded polling.
- **No upload garbage collection.** Orphaned multiparts (client crashes without `abort`) leak storage. Mitigation deferred to a later CLI subcommand; LocalStorage tempdirs can be reaped by file-mtime sweep, S3 has bucket lifecycle policies.
- **Cross-mount tests in 4c use a synthetic 2-mount fixture.** `HomeMountResolver` only ever returns one mount, so the cross-mount branch can't fire in production for 4c.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean (sqlite/mysql/postgres) | OK (CI) |
| 2 | `crabcloud-fs` crate exists with View + Uploads + Mount + MountResolver + StorageFactory + UserPath | OK |
| 3 | `UserPath` enforces leading `/`, no `..`/`.`/NUL/backslash, ≤4096 chars | OK (`path.rs::tests::*`) |
| 4 | Mount resolution: longest-prefix-match; trims prefix to derive storage-relative path | OK (`view.rs::tests::resolve_picks_longest_matching_prefix`) |
| 5 | `HomeMountResolver` returns exactly one mount per user, anchored at root | OK (`resolver/mod.rs::tests::home_resolver_returns_single_mount_at_root`) |
| 6 | `LocalStorageFactory` constructs storage at `data_dir/uid/files` (creates dir if absent) | OK (`resolver/local.rs::tests::home_storage_creates_path`) |
| 7 | View read ops route through FileCache | OK (`tests/view_reads.rs::view_stat_returns_metadata_for_existing_file`) |
| 8 | View write ops emit through ChannelEventSink | OK (`tests/view_reads.rs::view_put_then_read_roundtrip`) |
| 9 | View rename/copy within mount succeed; cross-mount errors `FsError::CrossMount` | OK (`tests/view_moves.rs::*`) |
| 10 | `Uploads::begin` → `put_part` → `commit` round-trips | OK (`tests/uploads.rs::uploads_begin_put_commit_roundtrip`) |
| 11 | `Uploads::commit` errors on destination mismatch | OK (`tests/uploads.rs::uploads_destination_mismatch_errors_on_commit`) |
| 12 | `Uploads::abort` is idempotent on unknown id | OK (`tests/uploads.rs::uploads_abort_idempotent_on_unknown_id`) |
| 13 | `AppState::view_for(uid)` + `uploads_for(uid)` work | OK (`tests/appstate_wiring.rs::*`) |
| 14 | `[storage] data_dir = "..."` config block | DEVIATION: uses existing `datadirectory` instead. |
| 15 | Workspace `-D warnings` clean | OK (CI) |
| 16 | `git grep -i rustcloud` empty | OK |
