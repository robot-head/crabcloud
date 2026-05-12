# Mount/View + Chunked-Upload Façades (Sub-project 4c)

**Date:** 2026-05-12
**Status:** Brainstormed; awaiting user approval before plan-writing.

## 1. Goal

Make the storage primitives shipped in 4a + the cache shipped in 4b usable by HTTP handlers. Two façades:

- **`View`** — per-user filesystem facade. Resolves user paths (`/photos/cat.jpg`) to `(Arc<dyn Storage>, StoragePath)` via the user's mounts. Routes reads through `FileCache`; writes emit through `ChannelEventSink`.
- **`Uploads`** — translates Nextcloud's chunked-upload HTTP protocol to the Storage trait's multipart primitives. Stateless across requests: the opaque `upload_id` returned to the client encodes everything needed for `put_part`/`abort`/`commit`.

After 4c, sub-project 5 (WebDAV) can be a thin HTTP-protocol layer over these façades.

## 2. Why now / who asked

Sub-project 4a shipped the `Storage` trait. 4b shipped the cache. Without 4c, every HTTP handler would need its own copy of:
- Per-user storage lookup (data_dir + uid → home storage).
- Path translation between user-facing absolute paths (`/photos/cat.jpg`) and storage-relative paths.
- Mount composition (currently just home, eventually shares + external storage).
- Chunked-upload protocol assembly.

4c centralizes these so sub-project 5 (WebDAV) and any future file API (mobile-app push, share API) call into one place.

## 3. Scope

**In scope:**

- New `crabcloud-fs` workspace crate.
- `UserPath` newtype (user-facing absolute path: leading `/`, no `..`, no `.`, no NUL, ≤ 4096 chars).
- `Mount` struct + `MountResolver` trait (forward-designed for share + external storage mounts).
- `HomeMountResolver` (one home mount per user).
- `StorageFactory` trait + `LocalStorageFactory` (data_dir/uid/files).
- `View` façade: `stat`/`list`/`read`/`read_range`/`put_file`/`mkdir`/`delete`/`rename`/`copy`.
- `Uploads` façade: `begin`/`put_part`/`abort`/`commit` with opaque self-describing `upload_id`.
- `FsError` + `FsResult`.
- `[storage] data_dir = "..."` config block in `crabcloud-config`.
- `AppState` gains `mount_resolver` field + `view_for(uid)` / `uploads_for(uid)` factory methods.
- 16 integration tests + per-module unit tests.

**Out of scope (deferred):**

- WebDAV / HTTP routes — sub-project **5**.
- Share mounts — sharing sub-project.
- External storage mounts — separate later sub-project.
- Mount-level permission overrides — combined sharing + 4c follow-up.
- Cross-mount rename/copy — relax `FsError::CrossMount` in a future sub-project when share mounts exist.
- Trash, versions, WebDAV LOCK/UNLOCK — separate later sub-projects.
- Encryption hooks — separate later sub-project.
- Quota enforcement — separate sub-project.
- `oc_uploads` DB table — not needed; uploads live in storage-layer multipart primitives.
- `uploads:gc` CLI to reap stale multiparts — a future sub-project.

## 4. Load-bearing decisions

- **`UserPath` is distinct from `StoragePath`.** Two newtypes keep the boundary explicit; the `View::resolve` method is the only conversion site. Mirrors the platform-core decision to separate wire-facing from internal representations.
- **One home mount per user in 4c.** `MountResolver` trait forward-designed so future mount kinds plug in. Cross-mount operations error with `FsError::CrossMount` — this can't fire in practice for 4c (only one mount) but is the right wire-shape for callers.
- **No new DB schema.** Uploads live in the storage backend's multipart state (LocalStorage tempdir, future S3 UploadId). Mounts computed in-process per request.
- **Opaque self-describing `upload_id`.** Format: `"{path_prefix_b64}:{dest_path_b64}:{backend_upload_id}"`. Survives server restarts as long as backing storage's multipart state survives. No in-process map or DB table.
- **View writes emit through `ChannelEventSink`.** Storage backends emit events through the sink; the scanner (from 4b) consumes asynchronously. Read-after-write through `View::stat` may lag the scanner by milliseconds. **`View::put_file` returns the storage's `FileMetadata` directly** (the fresh ETag), so callers don't need to wait for the scanner.
- **Destination-mismatch defense on commit.** `Uploads::commit(upload_id, destination, parts)` verifies `destination` matches what was passed to `begin`. Protects against client error / replay.
- **Per-request View construction.** `AppState::view_for(uid)` resolves the user's mounts and returns a fresh `View`. Mounts aren't cached on `AppState` (forward-design for share mounts that change with the user's share grants).

## 5. Crate + module layout

```
crates/crabcloud-fs/                                NEW
├── Cargo.toml
└── src/
    ├── lib.rs                                     UserPath + re-exports + FsError + FsResult
    ├── error.rs                                   FsError enum + FsResult type
    ├── mount.rs                                   Mount struct + MountResolver trait + StorageFactory trait
    ├── resolver/
    │   ├── mod.rs                                 HomeMountResolver default impl
    │   └── local.rs                               LocalStorageFactory (cfg-driven home dirs)
    ├── view.rs                                    View struct + stat/list/read/write/mkdir/delete/rename/copy
    └── uploads.rs                                 Uploads struct + UploadHandle + encode/decode upload_id

crates/crabcloud-fs/tests/                          Integration tests
├── support/
│   └── mod.rs                                     CountingResolver, multi-mount fixtures
└── (one file per integration scenario)

crates/crabcloud-core/src/state.rs                  MODIFIED + mount_resolver + view_for/uploads_for
crates/crabcloud-config/src/types.rs                MODIFIED + StorageConfig { data_dir }
Cargo.toml                                          MODIFIED + crabcloud-fs in workspace deps + members
```

**Cargo dependencies** for `crabcloud-fs`:

- `crabcloud-config` (workspace dep — for `StorageConfig`).
- `crabcloud-filecache` (workspace dep — for `FileCache`).
- `crabcloud-storage` (workspace dep — for `Storage`, `ChannelEventSink`, `MultipartHandle`, `PartTag`).
- `crabcloud-users` (workspace dep — for `UserId`).
- `async-trait`, `tokio` (fs/io-util/sync/macros), `thiserror`, `tracing`, `base64`.
- Dev: `tempfile`.

No HTTP / Axum / sqlx deps. Pure facade crate.

## 6. Public surface

### 6.1 `UserPath` (`crates/crabcloud-fs/src/lib.rs`)

```rust
pub struct UserPath(String);

impl UserPath {
    pub fn new(s: impl Into<String>) -> FsResult<Self>;
    pub fn root() -> Self;                  // "/"
    pub fn as_str(&self) -> &str;
    pub fn is_root(&self) -> bool;
    pub fn parent(&self) -> Option<UserPath>;
    pub fn basename(&self) -> &str;
    pub fn join(&self, child: &str) -> FsResult<UserPath>;
}
```

Normalization (enforced in `new`):

- **MUST start with `/`.**
- No `..` segments.
- No `.` segments.
- No empty segments (`/a//b`).
- No embedded NUL.
- No backslash.
- Forward-slash separator only.
- Max length 4096.
- Trailing slash stripped (`/foo/` → `/foo`).
- `UserPath::root()` == `"/"`; `is_root()` true.

### 6.2 `Mount` + `MountResolver` + `StorageFactory` (`crates/crabcloud-fs/src/mount.rs`)

```rust
#[derive(Clone)]
pub struct Mount {
    pub path_prefix: StoragePath,      // user-facing prefix; "" for home mount
    pub storage: Arc<dyn Storage>,
}

#[async_trait::async_trait]
pub trait MountResolver: Send + Sync {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>>;
}

#[async_trait::async_trait]
pub trait StorageFactory: Send + Sync {
    /// Per-user home storage. For LocalStorage: `<data_dir>/<uid>/files`.
    async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>>;
}
```

### 6.3 `HomeMountResolver` + `LocalStorageFactory`

```rust
pub struct HomeMountResolver {
    factory: Arc<dyn StorageFactory>,
}

impl HomeMountResolver {
    pub fn new(factory: Arc<dyn StorageFactory>) -> Self;
}

#[async_trait]
impl MountResolver for HomeMountResolver {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>> {
        let storage = self.factory.home_storage(uid).await?;
        Ok(vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
        }])
    }
}

pub struct LocalStorageFactory {
    data_dir: PathBuf,
}

impl LocalStorageFactory {
    pub fn new(data_dir: PathBuf) -> Self;
}

#[async_trait]
impl StorageFactory for LocalStorageFactory {
    async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
        let home = self.data_dir.join(uid.as_str()).join("files");
        tokio::fs::create_dir_all(&home).await.map_err(|e| {
            FsError::Storage(StorageError::Io(e))
        })?;
        Ok(Arc::new(LocalStorage::new(home).map_err(FsError::Storage)?))
    }
}
```

### 6.4 `View` (`crates/crabcloud-fs/src/view.rs`)

```rust
pub struct View {
    uid: UserId,
    mounts: Vec<Mount>,
    filecache: Arc<FileCache>,
    storage_sink: Arc<ChannelEventSink>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
    ) -> Self;

    pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata>;
    pub async fn list(&self, user_path: &UserPath) -> FsResult<Vec<DirEntry>>;

    pub async fn read(&self, user_path: &UserPath)
        -> FsResult<Pin<Box<dyn AsyncRead + Send>>>;
    pub async fn read_range(&self, user_path: &UserPath, range: Range<u64>)
        -> FsResult<Pin<Box<dyn AsyncRead + Send>>>;

    pub async fn put_file(
        &self,
        user_path: &UserPath,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<FileMetadata>;
    pub async fn mkdir(&self, user_path: &UserPath) -> FsResult<FileMetadata>;
    pub async fn delete(&self, user_path: &UserPath) -> FsResult<()>;

    pub async fn rename(&self, from: &UserPath, to: &UserPath) -> FsResult<()>;
    pub async fn copy(&self, from: &UserPath, to: &UserPath) -> FsResult<()>;
}
```

Internal helper:

```rust
fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)>;
```

Longest-prefix match against `self.mounts`. Strips the mount's `path_prefix` to produce the storage-relative `StoragePath`. Errors `FsError::MountNotFound` if no mount matches (shouldn't happen with a home mount anchored at `/`).

`rename` + `copy` reject cross-mount with `FsError::CrossMount` — for 4c this can't fire (one mount) but the wire shape is set.

### 6.5 `Uploads` (`crates/crabcloud-fs/src/uploads.rs`)

```rust
pub struct Uploads {
    uid: UserId,
    mounts: Vec<Mount>,
    storage_sink: Arc<ChannelEventSink>,
    filecache: Arc<FileCache>,
}

#[derive(Debug, Clone)]
pub struct UploadHandle {
    pub upload_id: String,           // opaque encoding
    pub destination: UserPath,
}

impl Uploads {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        storage_sink: Arc<ChannelEventSink>,
        filecache: Arc<FileCache>,
    ) -> Self;

    pub async fn begin(&self, destination: &UserPath) -> FsResult<UploadHandle>;

    pub async fn put_part(
        &self,
        upload_id: &str,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<PartTag>;

    pub async fn abort(&self, upload_id: &str) -> FsResult<()>;

    pub async fn commit(
        &self,
        upload_id: &str,
        destination: &UserPath,
        parts: Vec<PartTag>,
    ) -> FsResult<FileMetadata>;
}
```

#### `upload_id` encoding

`{path_prefix_b64}:{dest_path_b64}:{backend_upload_id}` where each `*_b64` uses URL-safe base64 of the raw UTF-8 string. The `backend_upload_id` is whatever the storage backend returned (e.g., `local-mp-<random_32>` for LocalStorage).

```rust
fn encode_upload_id(prefix: &StoragePath, dest: &StoragePath, backend: &str) -> String;
fn decode_upload_id(
    encoded: &str,
    mounts: &[Mount],
) -> FsResult<(&Mount, StoragePath, String)>;
```

`decode_upload_id` errors `FsError::Upload("malformed upload id")` on parse failure, `FsError::Upload("unknown mount")` if the prefix doesn't match any current mount.

### 6.6 `FsError` (`crates/crabcloud-fs/src/error.rs`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found")]
    NotFound,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("no mount matches user path")]
    MountNotFound,
    #[error("cross-mount operation not supported in this sub-project")]
    CrossMount,
    #[error("storage: {0}")]
    Storage(#[from] crabcloud_storage::StorageError),
    #[error("filecache: {0}")]
    FileCache(#[from] crabcloud_filecache::FileCacheError),
    #[error("upload: {0}")]
    Upload(String),
}

pub type FsResult<T> = Result<T, FsError>;
```

### 6.7 `[storage]` config block (`crates/crabcloud-config/src/types.rs`)

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
}
```

Added as a required field on `FileConfig`:

```rust
pub storage: StorageConfig,
```

(NOT `#[serde(default)]` — operators must specify the data directory.)

`test_support::minimal_sqlite_config` constructs a tempdir for `data_dir`.

### 6.8 `AppState` extensions (`crates/crabcloud-core/src/state.rs`)

```rust
pub struct AppState {
    // ... existing fields ...
    pub mount_resolver: Arc<dyn MountResolver>,
}

impl AppState {
    pub async fn view_for(&self, uid: &UserId) -> FsResult<View> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(View::new(
            uid.clone(),
            mounts,
            self.filecache.clone(),
            self.storage_sink.clone(),
        ))
    }

    pub async fn uploads_for(&self, uid: &UserId) -> FsResult<Uploads> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(Uploads::new(
            uid.clone(),
            mounts,
            self.storage_sink.clone(),
            self.filecache.clone(),
        ))
    }
}
```

`AppStateBuilder::build` constructs the resolver:

```rust
let factory = Arc::new(LocalStorageFactory::new(config.storage.data_dir.clone()));
let mount_resolver: Arc<dyn MountResolver> = Arc::new(HomeMountResolver::new(factory));
```

## 7. Path resolution algorithm

```rust
fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)> {
    let trimmed = user_path.as_str().trim_start_matches('/');
    let best = self.mounts.iter()
        .filter(|m| {
            let prefix = m.path_prefix.as_str();
            prefix.is_empty()
                || trimmed == prefix
                || trimmed.starts_with(&format!("{prefix}/"))
        })
        .max_by_key(|m| m.path_prefix.as_str().len())
        .ok_or(FsError::MountNotFound)?;
    let suffix = if best.path_prefix.is_root() {
        trimmed.to_string()
    } else {
        let with_slash = format!("{}/", best.path_prefix.as_str());
        trimmed.strip_prefix(&with_slash).map(String::from).unwrap_or_default()
    };
    let storage_path = StoragePath::new(suffix)?;
    Ok((best, storage_path))
}
```

Cross-mount = `resolve(from).0.path_prefix != resolve(to).0.path_prefix`.

## 8. Read/write flow

### `View::stat`

```rust
let (mount, storage_path) = self.resolve(user_path)?;
let meta = self.filecache.stat(&mount.storage, &storage_path).await?;
Ok(meta)
```

### `View::put_file`

```rust
let (mount, storage_path) = self.resolve(user_path)?;
let meta = mount.storage
    .put_file(&storage_path, body, &*self.storage_sink)
    .await?;
Ok(meta)
```

The storage emits a `Written` event to `storage_sink`; the scanner (4b's `Scanner`) consumes and updates the filecache asynchronously. The View doesn't wait — `put_file` returns the storage's fresh `FileMetadata` directly.

### `View::list`

```rust
let (mount, storage_path) = self.resolve(user_path)?;
self.filecache.list(&mount.storage, &storage_path).await.map_err(Into::into)
```

(`FileCache::list` populates immediate children on miss, per 4b.)

### `View::rename`

```rust
let (from_mount, from_path) = self.resolve(from)?;
let (to_mount, to_path) = self.resolve(to)?;
if from_mount.path_prefix != to_mount.path_prefix {
    return Err(FsError::CrossMount);
}
from_mount.storage.rename(&from_path, &to_path, &*self.storage_sink).await?;
Ok(())
```

(Identical shape for `copy`.)

### `View::mkdir` / `delete`

Straight pass-through to storage with `storage_sink`.

## 9. Upload flow

### `Uploads::begin`

```rust
let (mount, storage_path) = self.resolve(destination)?;
let handle = mount.storage
    .begin_multipart(&storage_path, &*self.storage_sink)
    .await?;
let upload_id = encode_upload_id(&mount.path_prefix, &storage_path, &handle.upload_id);
Ok(UploadHandle { upload_id, destination: destination.clone() })
```

### `Uploads::put_part`

```rust
let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
let handle = MultipartHandle { upload_id: backend_id, target: storage_path };
mount.storage.put_part(&handle, part_number, body).await.map_err(Into::into)
```

### `Uploads::commit`

```rust
let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
let (dest_mount, dest_path) = self.resolve(destination)?;
if dest_mount.path_prefix != mount.path_prefix || dest_path != storage_path {
    return Err(FsError::Upload("destination mismatch".into()));
}
let handle = MultipartHandle { upload_id: backend_id, target: storage_path };
let meta = mount.storage
    .commit_multipart(handle, parts, &*self.storage_sink)
    .await?;
Ok(meta)
```

### `Uploads::abort`

```rust
let (mount, storage_path, backend_id) = match decode_upload_id(upload_id, &self.mounts) {
    Ok(x) => x,
    Err(_) => return Ok(()),  // idempotent
};
let handle = MultipartHandle { upload_id: backend_id, target: storage_path };
let _ = mount.storage.abort_multipart(handle).await;
Ok(())
```

## 10. Protocol mapping (informational, consumed by sub-project 5)

| HTTP request | `Uploads` call |
|---|---|
| `MKCOL /dav/uploads/<user>/<upload_id>` (after client handshake) | `Uploads::begin(destination)` |
| `PUT /dav/uploads/<user>/<upload_id>/<n>` | `Uploads::put_part(upload_id, n, body)` |
| `MOVE /dav/uploads/<user>/<upload_id>/.file` to `Destination: /dav/files/<user>/<path>` | `Uploads::commit(upload_id, destination, parts)` |
| `DELETE /dav/uploads/<user>/<upload_id>` | `Uploads::abort(upload_id)` |

Wire encoding decisions (header names, `.file` suffix, part-tag transmission) belong to sub-project 5.

## 11. Test strategy

### 11.1 Unit tests (per module)

- `UserPath::new` invariants (leading `/`, `..`, `.`, NUL, backslash, max length, trailing slash).
- `Mount::path_prefix` longest-prefix match (with 2-mount fixture for the algorithm; HomeMountResolver itself uses only 1).
- `upload_id` encode → decode round-trip; decode rejects malformed input.
- `HomeMountResolver` returns exactly one mount per user.
- `LocalStorageFactory` constructs `<data_dir>/<uid>/files`; creates the dir if absent.

### 11.2 Integration tests (`crates/crabcloud-fs/tests/`)

1. `view_stat_returns_metadata`
2. `view_put_then_read_roundtrip`
3. `view_list_returns_children`
4. `view_mkdir_creates_directory`
5. `view_delete_removes_file_and_dir`
6. `view_rename_within_mount`
7. `view_copy_within_mount`
8. `view_rename_cross_mount_errors` (uses a synthetic 2-mount resolver fixture)
9. `view_invalid_path_rejected`
10. `view_path_escape_attempt_rejected`
11. `uploads_begin_put_commit_roundtrip`
12. `uploads_destination_mismatch_errors_on_commit`
13. `uploads_abort_idempotent`
14. `uploads_abort_then_commit_errors`
15. `uploads_part_tag_round_trip`
16. `appstate_view_for_returns_consistent_mounts`

### 11.3 Cache lag handling in tests

`View::put_file` returns the storage's `FileMetadata` directly, so post-write read-back from the same operation's return value is deterministic. Tests that need to verify cache state (e.g., `View::list` after a write) either:
- (a) Poll briefly for scanner catch-up via `tokio::time::sleep` + retry, OR
- (b) Bypass the View and call `cache.apply` synchronously in test setup.

Most integration tests use approach (a) with bounded polling (≤500ms total, 25ms increments) — matches the 4b scanner test pattern.

## 12. Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | `cargo xtask check-all` clean (sqlite/mysql/postgres) | CI |
| 2 | `crabcloud-fs` crate exists with View + Uploads + Mount + MountResolver + StorageFactory + UserPath | crate import test |
| 3 | `UserPath` enforces leading `/`, no `..`, no `.`, no NUL, ≤4096 chars | unit |
| 4 | Mount resolution: longest-prefix-match; trims prefix to derive storage-relative path | unit |
| 5 | `HomeMountResolver` returns exactly one mount per user, anchored at root | unit |
| 6 | `LocalStorageFactory` constructs storage at `data_dir/uid/files` (creates dir if absent) | unit |
| 7 | View read ops route through FileCache | integration |
| 8 | View write ops emit through ChannelEventSink | integration |
| 9 | View rename/copy within mount succeed; cross-mount errors `FsError::CrossMount` | integration |
| 10 | `Uploads::begin` → `put_part` → `commit` round-trips | integration |
| 11 | `Uploads::commit` errors on destination mismatch | integration |
| 12 | `Uploads::abort` is idempotent on unknown id | integration |
| 13 | `AppState::view_for(uid)` + `uploads_for(uid)` work | integration |
| 14 | `[storage] data_dir = "..."` config block | unit |
| 15 | Workspace `-D warnings` clean | CI |
| 16 | `git grep -i rustcloud` empty | CI |

## 13. Estimated batches (~5–6 PRs)

| Batch | Theme |
|-------|---|
| **A** | `crabcloud-fs` crate skeleton + `UserPath` + `Mount` + `FsError` + `MountResolver`/`StorageFactory` trait declarations + `HomeMountResolver` + `LocalStorageFactory` + unit tests |
| **B** | `View::stat`/`list`/`read`/`read_range`/`put_file`/`mkdir`/`delete` + integration tests #1-#5, #9 (read path), #10 (path escape) |
| **C** | `View::rename` + `View::copy` + cross-mount error + integration tests #6-#9 |
| **D** | `Uploads` façade: begin/put_part/abort/commit + `upload_id` encode/decode + integration tests #11-#15 |
| **E** | `AppState::view_for` / `uploads_for` + `[storage] data_dir` config + wire `HomeMountResolver` into `AppStateBuilder::build` + integration test #16 |
| **F** | Acceptance docs (sub-project changelog + README workspace-layout bullet + sub-project 5 WebDAV prep notes) |

## 14. Risks + mitigations

- **Scanner lag between View writes and View reads.** Mitigated by `View::put_file` returning the storage's `FileMetadata` directly; tests that need cache state use bounded polling. Production WebDAV will return the PUT response's ETag from the same source.
- **`upload_id` length.** Base64 of path prefix + dest path + backend id can exceed URL length limits if dest path is very deep. Mitigation: paths are capped at 4096 chars (UserPath); base64 inflates by 4/3; backend id is < 64 chars; total worst case ~5500 chars. Most clients support 8 KB URIs. Worth noting in operator docs.
- **Cross-mount can't be tested with `HomeMountResolver` alone.** Integration test #8 uses a synthetic 2-mount `TestResolver` fixture (in `tests/support/mod.rs`) to exercise the cross-mount error path.
- **No upload garbage collection.** Orphaned multiparts (client crashes, never aborts) leak storage. A future `crabcloud uploads:gc` CLI is deferred; LocalStorage's tempdirs can be pruned by file-mtime sweep, S3 has bucket lifecycle policies.
- **`StorageConfig.data_dir` is required, no default.** Operators must set it; missing = startup config error. This is intentional — silently defaulting to a path the operator didn't choose is worse.

## 15. Open questions (deferred)

- **Mount caching on `AppState`?** Currently each `view_for` re-resolves mounts. For a single home-mount-per-user system, the resolver hits one syscall (LocalStorageFactory creates the dir if missing). When share mounts arrive, this might become expensive — revisit then.
- **`Uploads::commit` partial verification?** We currently trust the caller's `parts` Vec. A future hardening could store the part metadata in the storage backend's multipart state + verify the commit list matches.
- **Read-through populate races.** When 1000 concurrent `View::stat` calls hit the same uncached path, the per-path lock in `FileCache::populate` serializes them. This is correct for cache correctness but creates a stat-call bottleneck on cold paths. Probably fine; revisit if profiling shows it as a hotspot.

## 16. Dependencies on other sub-projects

- **Upstream:** 4a (Storage trait + LocalStorage + ChannelEventSink), 4b (FileCache + Scanner).
- **Downstream:** sub-project 5 (WebDAV) consumes `View` + `Uploads` via `AppState::view_for` / `uploads_for`. Sharing sub-project layers share mounts onto `MountResolver`. External-storage sub-project adds non-local `StorageFactory` impls.
