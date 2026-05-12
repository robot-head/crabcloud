# Sub-project 4a — Changelog

Completed: 2026-05-12

## What works

- New `crabcloud-storage` crate; pure primitives (no DB, HTTP, or Dioxus deps).
- `Storage` async trait covering `stat`/`exists`/`list`/`read`/`read_range`/`put_file`/`mkdir`/`delete`/`rename`/`copy` + `begin_multipart`/`put_part`/`commit_multipart`/`abort_multipart`. Object-safe.
- `MemoryStorage` backend: `Arc<RwLock<MemTree>>` around a `BTreeMap<StoragePath, MemEntry>`; multipart via per-handle `Mutex<BTreeMap<u32, Bytes>>`; implicit-mkdir for parent directories.
- `LocalStorage` backend: atomic-rename writes (tempfile + fsync + rename + parent-fsync); xattr-persisted ETag with mtime+inode fallback; ~100-entry mimetype table (extension lookup) + `infer`-based magic-byte sniff fallback; range reads; recursive copy; multipart via per-upload tempdir + sha256 part integrity check.
- `StoragePath` newtype with normalization (no `.`, no `..`, no leading `/`, no NUL, no backslash, <=4096 chars).
- `EventSink` trait + `NoopEventSink`; mutating ops emit `StorageEvent::{Written, DirCreated, Deleted, Moved, Copied}` with `storage_id`, `path`, and (where applicable) `metadata`.
- Parametrized trait test suite — 15 assertions covering happy paths + edge cases + multipart + event emission. Both backends pass.
- Local-specific tests for xattr persistence, mtime+inode fallback, atomic temp cleanup, symlink-based path escape rejection, multipart abort + corrupted-part rejection.
- Memory-specific tests for 100-way concurrent writes (distinct + same path).

## What's deferred

- `oc_filecache` schema + scanner — **sub-project 4b**.
- S3 backend — **sub-project 4b**.
- Real (channel-backed) `EventSink` consumer — **sub-project 4b**.
- Mount composition (`View` layer) — **sub-project 4c**.
- Chunked-upload protocol translation (Nextcloud's `/dav/uploads/...` flow) — **sub-project 4c**.
- WebDAV — sub-project 5.
- Trash, versions, WebDAV LOCK/UNLOCK — separate later sub-projects.
- Server-side encryption seam — later sub-project.
- Sharing-aware permissions composition — later sub-project.

## Known limitations

- ETag xattr is **Unix-only**. On Windows, `LocalStorage::stat` falls back to the mtime+inode-derived ETag — deterministic and changes on mutation, but no random per-write entropy. (Windows inode is `0` because there's no native concept; on Windows the fallback aliases all files with identical mtime.)
- Mimetype table is ~100 entries (most-used). Nextcloud's upstream `mimetypemapping.dist.json` has ~400; we'll expand additively as test coverage drives.
- `MemoryStorage` implicitly creates parent directories on `put_file`. `LocalStorage` requires explicit `mkdir` for parents. The asymmetry is documented in the trait suite — each backend is tested against its own contract.
- No `LOCK`/`UNLOCK`. WebDAV-compliant locking is a separate sub-project.
- `LocalStorage::resolve` defends against symlink escape via post-canonicalize `starts_with(root)`. There's still a TOCTOU window between `canonicalize` and the actual open syscall — `openat`-style hardening is deferred.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean | OK (CI) |
| 2 | `Storage` trait object-safe (`Arc<dyn Storage>` compiles) | OK (`lib.rs::tests::storage_trait_is_object_safe`) |
| 3 | Both backends pass full parametrized trait suite | OK (`tests/trait_suite.rs::{memory,local}_backend_passes_trait_suite`) |
| 4 | Atomic write: no leftover temp files; rename is atomic | OK (`tests/local_specific.rs::atomic_write_temp_cleaned_on_drop`) |
| 5 | ETag is 40-char lowercase hex matching Nextcloud format | OK (`meta.rs::tests::etag_new_is_40_hex_chars`) |
| 6 | Mimetype detection for known extensions, magic-byte sniff, octet-stream fallback | OK (`local/mimetype.rs::tests::*`) |
| 7 | Range reads return exactly the requested slice | OK (trait suite) |
| 8 | Multipart happy + abort + gap + duplicate | OK (trait suite) |
| 9 | EventSink emissions match every mutation 1:1 | OK (trait suite) |
| 10 | Path escape rejected (constructor + resolve defense) | OK (`path.rs::tests` + `tests/local_specific.rs::path_escape_via_canonicalize_rejected`) |
| 11 | `-D warnings` clean | OK (CI) |
| 12 | `git grep -i rustcloud` empty | OK |
| 13 | New crate documented in README's workspace-layout bullet | OK (batch F) |
