# Storage Trait + Local FS + In-Memory Backend (Sub-project 4a)

**Date:** 2026-05-12
**Status:** Brainstormed; awaiting user approval before plan-writing.

## 1. Goal

Lay down the foundational `Storage` trait that Crabcloud's filesystem abstraction will be built on, plus two backends:

- `LocalStorage` — production-grade local filesystem backend (atomic writes, ETag xattr, mimetype detection, range reads, multipart).
- `MemoryStorage` — in-process backend for tests + dev fixtures.

The trait is the contract that all future backends (S3 in 4b; SMB/external storage in later sub-projects) implement. Sub-projects 4b (filecache + scanner + S3) and 4c (mount composition + chunked-upload assembly) plug into it.

## 2. Why now / who asked

Sub-project 4 (storage abstraction) is the dependency gate for sub-project 5 (WebDAV + Files API), which is what makes Crabcloud actually useful (clients can sync). Sub-project 4 was decomposed into 4a/4b/4c to keep each spec reviewable and each plan executable in 4–6 batches.

## 3. Scope

**In scope:**

- `crabcloud-storage` crate (new in workspace).
- `Storage` trait, async + object-safe.
- `StoragePath` newtype with normalization.
- `FileMetadata`, `ETag`, `Mimetype`, `Permissions`, `DirEntry`, `FileKind`.
- `StorageError` enum + `StorageResult<T>`.
- `EventSink` trait + `NoopEventSink` + `StorageEvent` enum (seam for 4b's async scanner).
- `MultipartHandle` + `PartTag` types.
- `LocalStorage` backend with atomic-write, xattr ETag + mtime/inode fallback, mimetype detection (~400-entry extension table + magic-byte sniffing), range reads, multipart via tempdir + concatenate + atomic-rename.
- `MemoryStorage` backend with `BTreeMap<StoragePath, MemEntry>` under a single `RwLock`.
- Parametrized trait test suite exercising both backends symmetrically.
- Local-FS-specific tests (atomic durability, xattr persistence).
- Acceptance docs.

**Out of scope (deferred to 4b/4c or later):**

- `oc_filecache` table or any DB integration — 4b.
- S3 backend — 4b.
- Async scanner / real `EventSink` consumer — 4b.
- Mount composition / `View` layer — 4c.
- Chunked-upload protocol translation (Nextcloud's `/dav/uploads/...` MOVE flow) — 4c. (The storage-layer multipart primitives ship here.)
- WebDAV / HTTP routes — sub-project 5.
- Trash, versions, WebDAV locking — separate later sub-projects.
- Encryption hooks — separate later sub-project (the trait's `EventSink` seam is precedent for how an encryption decorator could later wrap a storage).
- Sharing-aware permissions composition — 4c + sharing sub-project.
- `chmod`/`chown` — files owned by the Crabcloud process user; per-user permissions live in 4b/4c.

## 4. Load-bearing decisions

- **ETag scheme matches upstream Nextcloud** — 40-char lowercase hex, regenerated on every mutation. Required for desktop/iOS/Android client sync to detect changes via byte-compatible ETags.
- **Permissions bitmap matches upstream** — `CREATE=4`, `READ=1`, `UPDATE=2`, `DELETE=8`, `SHARE=16`. Sub-project 4a stores raw backend permissions only; 4b/4c combine with mount-level permissions.
- **No filecache in 4a.** Backends `stat` the underlying storage on every call. Will be cheap on local FS (page cache); expensive on S3 (4b adds caching).
- **Async event sink seam from day one.** Every mutating trait method takes `&dyn EventSink`. 4a ships `NoopEventSink`. 4b adds `ChannelEventSink` over `tokio::sync::broadcast`. Backends call `sink.emit(...).await` on every mutation. Failed emits are logged, not propagated — events are observations of state already committed.
- **Streams over Bytes.** Reads + writes use `Pin<Box<dyn AsyncRead + Send>>` for stream-friendliness with large files. Backends + tests use `tokio::io::*`.
- **Object-safe trait.** No generic methods. `Arc<dyn Storage>` works in 4c's mount table.
- **Within-storage moves only.** `Storage::rename(from, to)` and `Storage::copy(from, to)` require both paths on the same storage. Cross-storage moves are 4c's `View` concern (copy + delete with cross-store streaming).

## 5. Crate + module layout

```
crates/crabcloud-storage/
├── Cargo.toml
└── src/
    ├── lib.rs            # Storage trait + EventSink trait + StorageEvent enum + NoopEventSink
    ├── error.rs          # StorageError + StorageResult
    ├── path.rs           # StoragePath newtype (UTF-8, normalized)
    ├── meta.rs           # FileMetadata, ETag, Mimetype, Permissions, DirEntry, FileKind, MultipartHandle, PartTag
    ├── local/
    │   ├── mod.rs        # LocalStorage struct + Storage impl
    │   ├── atomic.rs     # tempfile + fsync + rename + parent-dir fsync
    │   └── mimetype.rs   # extension table (phf) + magic-byte sniff (infer crate)
    └── memory/
        └── mod.rs        # MemoryStorage struct + Storage impl
```

**Cargo dependencies** (new crate):

- `tokio` (workspace, `fs`/`io-util`/`sync`/`macros` features).
- `bytes` (workspace).
- `async-trait` (workspace).
- `thiserror` (workspace).
- `tracing` (workspace).
- `phf` (new — for the extension→mimetype map).
- `infer` (new — magic-byte sniffing).
- `xattr` (new — POSIX xattr for ETag persistence; behind cfg).
- `tempfile` (dev-dep only — for tests).
- `rand` (workspace — already pinned; for ETag generation).

`crabcloud-storage` does NOT depend on `sqlx`, `axum`, `crabcloud-core`, `crabcloud-http`, `crabcloud-users`. Pure primitives.

## 6. Public surface

### 6.1 `StoragePath`

```rust
pub struct StoragePath(String);

impl StoragePath {
    pub fn new(s: impl Into<String>) -> StorageResult<Self>;
    pub fn root() -> Self;                     // empty path; represents storage root
    pub fn as_str(&self) -> &str;
    pub fn parent(&self) -> Option<StoragePath>;
    pub fn basename(&self) -> &str;
    pub fn join(&self, child: &str) -> StorageResult<StoragePath>;
}
```

Normalization rules (enforced in `new`):

- UTF-8 (the `String` type enforces this; rejects invalid byte sequences upstream).
- No leading `/` — paths are relative-to-storage-root.
- No embedded NUL (`\0`).
- No `..` segments.
- No empty segments (`a//b` rejected).
- Forward-slash separator only (Windows backslashes are not accepted as separators — call sites must convert).
- Max length 4096 (POSIX `PATH_MAX`).
- Trailing slash stripped on construction.

### 6.2 `Storage` trait

```rust
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    fn id(&self) -> &str;

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata>;
    async fn exists(&self, path: &StoragePath) -> StorageResult<bool>;
    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>>;

    async fn read(&self, path: &StoragePath)
        -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;
    async fn read_range(&self, path: &StoragePath, range: Range<u64>)
        -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn mkdir(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn delete(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn begin_multipart(
        &self,
        target: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle>;

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag>;

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()>;
}
```

### 6.3 Supporting types (`meta.rs`)

```rust
pub enum FileKind { File, Directory }

pub struct FileMetadata {
    pub path: StoragePath,
    pub kind: FileKind,
    pub size: u64,              // bytes; 0 for directories (4b's cache computes aggregates)
    pub mtime: SystemTime,
    pub etag: ETag,
    pub mimetype: Mimetype,
    pub permissions: Permissions,
}

pub struct DirEntry {
    pub name: String,           // basename only
    pub metadata: FileMetadata,
}

pub struct ETag(String);
impl ETag {
    pub fn new() -> Self;       // 40-char hex from CSPRNG
    pub fn from_hex(s: &str) -> StorageResult<Self>;
    pub fn as_str(&self) -> &str;
}

pub struct Mimetype(String);
impl Mimetype {
    pub fn parse(s: &str) -> StorageResult<Self>;  // validates type/subtype shape
    pub fn octet_stream() -> Self;
    pub fn as_str(&self) -> &str;
}

pub struct Permissions(u8);
impl Permissions {
    pub const CREATE: u8 = 4;
    pub const READ:   u8 = 1;
    pub const UPDATE: u8 = 2;
    pub const DELETE: u8 = 8;
    pub const SHARE:  u8 = 16;
    pub const ALL:    u8 = Self::CREATE | Self::READ | Self::UPDATE | Self::DELETE | Self::SHARE;
    pub fn full() -> Self;
    pub fn readonly() -> Self;
    pub fn bits(self) -> u8;
    pub fn contains(self, other: Permissions) -> bool;
}

pub struct MultipartHandle {
    pub upload_id: String,      // opaque to caller; backend-defined
    pub target: StoragePath,
}

pub struct PartTag {
    pub part_number: u32,
    pub etag: String,           // backend-defined (S3's ETag; local-FS's sha256)
}
```

### 6.4 `StorageError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("directory not empty")]
    NotEmpty,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("multipart: {0}")]
    Multipart(String),
    #[error("storage error: {0}")]
    Other(String),
}

pub type StorageResult<T> = Result<T, StorageError>;
```

### 6.5 `EventSink` + `StorageEvent`

```rust
#[derive(Debug, Clone)]
pub enum StorageEvent {
    Written {
        storage_id: String,
        path: StoragePath,
        metadata: FileMetadata,
    },
    DirCreated {
        storage_id: String,
        path: StoragePath,
        metadata: FileMetadata,
    },
    Deleted {
        storage_id: String,
        path: StoragePath,
    },
    Moved {
        storage_id: String,
        from: StoragePath,
        to: StoragePath,
    },
    Copied {
        storage_id: String,
        from: StoragePath,
        to: StoragePath,
    },
}

#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: StorageEvent);
}

pub struct NoopEventSink;

#[async_trait::async_trait]
impl EventSink for NoopEventSink {
    async fn emit(&self, _event: StorageEvent) {}
}
```

## 7. LocalStorage details

### 7.1 Construction + path resolution

`LocalStorage::new(root: PathBuf) -> StorageResult<Self>`:
- `root.canonicalize()` (resolves symlinks, must exist).
- `id = format!("local::{}", root.display())`.

`fn resolve(&self, path: &StoragePath) -> StorageResult<PathBuf>`:
- `let p = self.root.join(path.as_str());`
- If the file exists, `canonicalize` it and assert the result starts with `self.root` (defense in depth against any `..`-bypass bug in `StoragePath`).
- If the file doesn't exist yet (write target), assert `p.canonicalize().is_some_and(...)` on the parent.
- Reject with `InvalidPath` if resolution escapes `root`.

### 7.2 Atomic writes (`local/atomic.rs`)

`put_file` sequence:

1. Resolve target → `target_real`.
2. Validate target's parent exists + is a directory.
3. Create temp file `{target_parent}/.tmp-crabcloud-{random_64}`. Same dir = same filesystem = atomic rename.
4. Stream `body` to temp file. `flush()`. `sync_all()` (fsync).
5. Compute fresh `ETag::new()`.
6. Set xattr `user.crabcloud.etag` on the temp file. If xattr unsupported, skip (fallback: ETag derived from mtime+inode at stat time — non-random but stable).
7. Set xattr `user.crabcloud.mimetype` (computed on first 4096 bytes of body, buffered into a peek).
8. Rename temp → target. POSIX `rename(2)` atomic on same fs; Windows `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`.
9. Fsync parent directory (POSIX; no-op on Windows).
10. Stat target to assemble `FileMetadata`.
11. `sink.emit(StorageEvent::Written { ... }).await`.
12. Return metadata.

Failure recovery: a `TempFileGuard` Drop-implementing struct removes the temp file on any error path before the rename. After successful rename, the guard is `.forget()`ed.

Crash recovery: a crash between rename and event-emit means the file is on disk but no event fired. 4b's scanner handles this via filesystem walk + cache reconciliation. Best-effort `tracing::warn!` on emit failure.

### 7.3 ETag persistence

- **Linux + macOS:** xattr `user.crabcloud.etag` via the `xattr` crate.
- **Windows:** NTFS alternate data stream `:crabcloud:etag` — but the `xattr` crate doesn't support Windows. For 4a, on Windows we use the mtime+inode fallback exclusively. Documented limitation.
- **Filesystem without xattr support** (vfat, exfat, some network mounts): fallback to `etag = hex(blake3(mtime_ns_le_bytes || inode_le_bytes))[..40]`. Stable across reads, changes on mutation, but loses the random-per-write property. Documented.

### 7.4 Mimetype detection (`local/mimetype.rs`)

1. Extension lookup against a static `phf::Map<&str, &str>` — initial seed of common types (~400 entries) derived from Nextcloud's `resources/config/mimetypemapping.dist.json`. Compiled at build time via `phf_codegen` in `build.rs`.
2. If lookup returns `None` OR the lookup result is `application/octet-stream`, peek first 4096 bytes via `infer::get(&peek)`. If found, use that mimetype.
3. Final fallback: `application/octet-stream`.

Cached on the file as xattr `user.crabcloud.mimetype` on write. Stat reads xattr first; if absent, recomputes.

### 7.5 Range reads

```rust
async fn read_range(&self, path: &StoragePath, range: Range<u64>)
    -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
    let real = self.resolve(path)?;
    let mut f = tokio::fs::File::open(real).await?;
    f.seek(SeekFrom::Start(range.start)).await?;
    let limited = f.take(range.end - range.start);
    Ok(Box::pin(limited))
}
```

No `Content-Range` math here — caller (sub-project 5's WebDAV handler) does HTTP headers.

### 7.6 Multipart

`MultipartHandle::upload_id` = `format!("local-mp-{}", random_32_hex)`. Tempdir at `{target_parent}/.upload-{upload_id}/`.

`put_part(handle, n, body)`:
1. Stream `body` to `{tempdir}/part-{n:08}`.
2. Compute sha256 of the written part.
3. Return `PartTag { part_number: n, etag: hex(sha256) }`.

`commit_multipart(handle, parts, sink)`:
1. Validate `parts` is non-empty, sorted by `part_number`, and contiguous starting at 1. Reject with `StorageError::Multipart` on gap or duplicate.
2. Verify each part's stored sha256 (from disk re-hash) matches the `PartTag.etag` supplied by the caller. Mismatch → `StorageError::Multipart("part {n} integrity check failed")`. Always — cheap defense against tampering or a buggy caller mis-tagging.
3. Concatenate parts in order into a fresh temp file at `{target_parent}/.tmp-crabcloud-{random_64}`. Fsync.
4. Same final-rename + parent-fsync sequence as `put_file`. Generate fresh ETag.
5. Recursively delete the upload tempdir.
6. `sink.emit(StorageEvent::Written { ... }).await`.
7. Return metadata.

`abort_multipart(handle)`: recursively delete the upload tempdir. Idempotent.

### 7.7 Directory ops

- `mkdir`: `tokio::fs::create_dir` (errors if parent missing — caller creates parents explicitly).
- `list`: `tokio::fs::read_dir`, then `stat` each entry. O(N) syscalls per listing; 4b's cache will replace.
- `rename`: `tokio::fs::rename`. Same-storage only (caller validates).
- `copy`: file → stream-copy; directory → recursive walk, fresh ETag per leaf, `mkdir` mirrors at destination.
- `delete`: file → `remove_file`; directory → check empty via single `read_dir` pass, then `remove_dir` (no recursive — caller orchestrates).

## 8. MemoryStorage details

`Arc<RwLock<MemTree>>` around the whole tree. Coarse but adequate for test workloads.

```rust
struct MemTree {
    entries: BTreeMap<StoragePath, MemEntry>,
}

enum MemEntry {
    File {
        bytes: Bytes,
        etag: ETag,
        mtime: SystemTime,
        mimetype: Mimetype,
    },
    Directory {
        etag: ETag,
        mtime: SystemTime,
    },
}
```

Reads take read-lock; mutations take write-lock + bump mtime + emit event.

Multipart: per-handle `Arc<Mutex<BTreeMap<u32, Bytes>>>`. `commit_multipart` concatenates in order. `abort_multipart` drops the handle map.

Implicit directory creation: when `put_file("a/b/c.txt", ...)` is called, parent directories `a/` and `a/b/` are implicitly created if absent (matches the way most consumers expect to work). Reason: tests want to write files without N preceding `mkdir` calls. **Note:** `LocalStorage` does NOT implicitly create parents — it errors with `NotFound` on missing parent. The asymmetry is documented; the trait suite tests each backend on its own contract.

(Open question: should `LocalStorage` also implicitly create parents? Common convenience but masks bugs in callers. Decision: NO — strict mode. The View layer in 4c will handle implicit-mkdir-on-WebDAV-PUT if desired.)

## 9. Error mapping

- `std::io::ErrorKind::NotFound` → `StorageError::NotFound`.
- `AlreadyExists` → `StorageError::AlreadyExists`.
- `PermissionDenied` → `StorageError::PermissionDenied`.
- `InvalidInput` containing "not a directory" string → `StorageError::NotADirectory` (Linux-only marker; macOS/Windows differ).
- `IsADirectory` (Linux) → `StorageError::IsADirectory`.
- `DirectoryNotEmpty` → `StorageError::NotEmpty`.
- Anything else → `StorageError::Io(e)`.

A small helper `fn map_io(e: std::io::Error) -> StorageError` centralizes this. Mimetype + path validation use their own variants.

## 10. Test strategy

### 10.1 Parametrized trait suite

`crates/crabcloud-storage/tests/trait_suite.rs` (integration test, not unit) defines:

```rust
pub fn run_storage_suite<F, S>(name: &str, factory: F)
where
    F: Fn() -> S + Send + Sync,
    S: Storage + 'static,
{
    // …
}
```

Each test in the suite runs against both `LocalStorage` and `MemoryStorage` via two top-level test functions that invoke `run_storage_suite`. Suite covers (each as its own assertion within the runner):

1. `StoragePath::new("")` returns `InvalidPath`.
2. `StoragePath::new("../etc")` returns `InvalidPath`.
3. `StoragePath::new("a/./b")` returns `InvalidPath` (no `.` segments either).
4. `StoragePath::new("/abs")` returns `InvalidPath`.
5. `StoragePath::new("a\0b")` returns `InvalidPath`.
6. `put_file("hello.txt", "hi")` then `read("hello.txt")` returns `"hi"`.
7. `put_file` twice on same path: second wins; ETag differs from first.
8. `stat` on a written file: `size == 2`, `mtime` ≥ pre-write time, `etag` valid 40-char hex, `mimetype == "text/plain"`.
9. `read_range("hello.txt", 1..2)` returns `"i"`.
10. `mkdir("dir")` then `list("")` includes `dir` as a `Directory` entry.
11. `mkdir` then `put_file("dir/inner.txt", "x")` then `list("dir")` returns `inner.txt`.
12. `delete("dir/inner.txt")` removes; subsequent `stat` is `NotFound`.
13. `delete("dir")` after emptying: succeeds. `delete("dir")` while non-empty: `NotEmpty`.
14. `rename("a.txt", "b.txt")`: `stat("a.txt") == NotFound`; `read("b.txt")` returns original bytes.
15. `copy("a.txt", "b.txt")`: both readable, contents identical, ETags differ.
16. Multipart happy: `begin_multipart("big.bin")`, `put_part(1, "AAA")`, `put_part(2, "BBB")`, `commit_multipart` → `read("big.bin") == "AAABBB"`.
17. Multipart abort: `begin_multipart`, `put_part(1, "AAA")`, `abort_multipart` → `stat("big.bin") == NotFound`.
18. Multipart gap: `begin_multipart`, `put_part(1)`, `put_part(3)`, commit → `StorageError::Multipart`.
19. Multipart duplicate: `put_part(1)` twice, commit with both tags → `StorageError::Multipart`.
20. ETag changes on every mutating op (`put_file`, `rename` of dest dir doesn't change source; `copy` to → fresh ETag on dest).
21. EventSink emissions: a `RecordingSink` records every event; assert one event per mutation with correct variant + storage_id + path.

### 10.2 Local-FS-specific tests

`crates/crabcloud-storage/tests/local_specific.rs`:

- **Atomic durability:** spawn a `put_file` future, abort it at a controlled point (drop the temp guard before the rename), assert the target is either absent (first write) or contains the previous version (overwrite). Implementation uses an inject point in `atomic.rs` gated behind `#[cfg(test)]` — a `TestCrashHook` closure that panics at a chosen step.
- **Xattr persistence:** write file → read xattr `user.crabcloud.etag` directly via `xattr` crate → matches `stat().etag`. Restart `LocalStorage` (drop + new) → `stat` still returns the same etag.
- **Xattr-missing fallback:** mock by stripping xattrs after write → next stat falls back to mtime+inode-derived etag. `#[ignore]` on Windows (no xattr API there).
- **Path escape defense:** call `resolve()` with a hand-crafted `StoragePath` that bypasses the constructor (impossible from safe Rust outside the crate — but if a backend somehow gets one, canonicalize-and-verify rejects).

### 10.3 Memory-specific tests

`crates/crabcloud-storage/tests/memory_specific.rs`:

- **Concurrent distinct paths:** 100 tokio tasks `put_file` to distinct paths, all succeed, all readable. Asserts no false NotFound under load.
- **Concurrent same path:** 100 tasks `put_file` to one path with distinct contents; one wins, all others succeed without error, final content is one of the 100 inputs, ETag matches the winner.

### 10.4 Test helpers

`crates/crabcloud-storage/tests/support/mod.rs`:

```rust
pub struct RecordingSink {
    pub events: Arc<Mutex<Vec<StorageEvent>>>,
}

#[async_trait::async_trait]
impl EventSink for RecordingSink {
    async fn emit(&self, e: StorageEvent) {
        self.events.lock().unwrap().push(e);
    }
}
```

## 11. Security model

- **Path escape is the dominant risk.** Defense in depth: (1) `StoragePath` constructor rejects `..`, leading `/`, `\0`, etc. (2) `LocalStorage::resolve` does `canonicalize` and verifies `starts_with(root)`. Either layer alone catches the attack; both together survive a single-component bug.
- **Symlink handling:** `LocalStorage` uses `canonicalize` which follows symlinks. A symlink inside `root` pointing outside `root` will fail the `starts_with` check and reject with `InvalidPath`. **Out of scope:** TOCTOU between `canonicalize` and the actual open — a future hardening could use `openat`-style ops; not 4a.
- **Mimetype sniffing reads only 4096 bytes.** No content interpretation. The `infer` crate is well-maintained and doesn't execute file content.
- **Random ETags use the workspace's `rand::rng()`** which is a CSPRNG (ThreadRng, ChaCha-based, OS-reseeded) per the rand 0.10 contract. Documented.
- **EventSink failures are logged.** Cannot be used to roll back state. Future audit: 4b's scanner can detect divergence via filesystem walk vs cache.

## 12. Performance notes (not requirements)

- Local FS `stat` is one syscall + 1–2 xattr reads — comparable to native FS performance.
- Local FS `list` is O(entries) syscalls — 4b's cache will replace once filecache lands.
- Memory backend `RwLock` is coarse; if test workloads later need finer locking, switch to per-path locks. Not a 4a concern.
- No benchmarks in 4a's acceptance — defer perf measurement to 4b when comparing cached vs uncached reads.

## 13. Estimated batch breakdown (~5–6 PRs)

| Batch | Theme |
|-------|---|
| **A** | `crabcloud-storage` crate skeleton; `StoragePath`, `StorageError`, `FileMetadata`/`ETag`/`Mimetype`/`Permissions`/`DirEntry`/`FileKind`/`MultipartHandle`/`PartTag`; `EventSink` trait + `NoopEventSink` + `StorageEvent` enum; comprehensive type-level unit tests. No `Storage` trait yet. |
| **B** | `Storage` trait definition + the parametrized trait test runner. Backends-empty (no implementations). The runner compiles + is invokable; Batch C plugs the first backend in. |
| **C** | `MemoryStorage` complete implementation; runs the full trait suite green. First end-to-end storage. |
| **D** | `LocalStorage` core: `new` + `resolve` + `stat`/`exists`/`list`/`read`/`read_range`/`mkdir`/`delete`/`rename`/`copy`; atomic-write `put_file`; mimetype table + sniffing; xattr ETag (with fallback). Trait suite passes. |
| **E** | `LocalStorage` multipart (`begin`/`put_part`/`commit`/`abort`) + local-specific tests (atomic durability, xattr persistence, path escape). |
| **F** | Acceptance docs (changelog + README mention) + spec follow-up notes for 4b (filecache schema sketch + event consumer interface). |

## 14. Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | `cargo xtask check-all` green | CI |
| 2 | `Storage` trait is object-safe (`Arc<dyn Storage>` compiles in a doctest) | Compile test |
| 3 | Both backends pass the full parametrized trait suite | Trait suite |
| 4 | Atomic write: kill-mid-write does not corrupt target | Local-specific test |
| 5 | ETag is 40-char lowercase hex matching Nextcloud format | Trait suite |
| 6 | Mimetype detection: .png/.txt/.pdf/.zip; magic sniff fallback; octet-stream fallback | Trait suite |
| 7 | Range reads return exactly the requested slice | Trait suite |
| 8 | Multipart happy + abort + gap-rejection + duplicate-rejection | Trait suite |
| 9 | EventSink emissions match every mutation 1:1 with the correct variant | Trait suite |
| 10 | Path escape attempts rejected (StoragePath constructor + resolve defense) | Trait suite + local-specific |
| 11 | Workspace `-D warnings` clean | CI |
| 12 | `git grep -i rustcloud` empty | CI |
| 13 | New crate documented in README's workspace-layout bullet | Manual |

## 15. Risks + mitigations

- **Cross-platform xattr divergence (Linux ✓, macOS ✓, Windows ✗).** Mitigation: documented fallback (mtime+inode); xattr-specific tests `#[ignore]`d on Windows in CI.
- **`canonicalize` requires the file to exist.** For write targets that don't exist yet, we canonicalize the parent. Edge case: parent also doesn't exist → `put_file` errors with `NotFound` (parent missing); caller must `mkdir` first. Documented in the trait contract.
- **Phf static map is build-time.** A bad seed file breaks `cargo build`. Mitigation: pin the seed JSON file in the repo; add a unit test that the map's size is reasonable (>200 entries).
- **The `infer` crate adds another dep.** Mitigation: it's lightweight (~25 KB compiled, no transitive deps beyond `core`). Already widely used.
- **Memory-backend implicit-mkdir vs Local-backend strict-mkdir asymmetry.** Documented in the trait docstring; suite-level tests cover each backend on its own contract.

## 16. Open questions (deferred)

- Should `Storage` expose `truncate(path, new_len)` for partial overwrites? Nextcloud doesn't really use it. Defer until WebDAV needs it.
- Should multipart `PartTag` include a content hash for integrity validation at commit time? Current design includes it (sha256 in local backend). S3's ETag is already content-based for non-multipart parts.
- Should `EventSink` be a single `&dyn EventSink` or a `Vec<Arc<dyn EventSink>>` for multi-subscriber? Single is simpler; 4b can introduce a `FanOutSink` that wraps `Vec<Arc<dyn EventSink>>` internally.

## 17. Dependencies on other sub-projects

- **None upstream.** Sub-project 4a is the foundation; no other Crabcloud crate depends on it yet.
- **Downstream:** 4b adds filecache + S3 + real EventSink consumer; 4c adds mount composition + chunked-upload protocol translation; sub-project 5 (WebDAV) consumes 4c's View.
