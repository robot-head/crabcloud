# Mount/View + Uploads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `crabcloud-fs` — a new workspace crate with the per-user `View` façade (resolves user paths via mounts; reads through `FileCache`, writes emit through `ChannelEventSink`) and the `Uploads` façade (translates Nextcloud's chunked-upload protocol to the Storage trait's multipart primitives). After 4c, sub-project 5 (WebDAV) becomes a thin HTTP layer over these.

**Architecture:** New crate `crabcloud-fs` — pure façade, no HTTP/DB. `UserPath` newtype (leading-`/` required) is the wire-facing type; `View::resolve` is the only conversion site to `StoragePath`. `MountResolver` + `StorageFactory` traits forward-designed for share + external storage mounts; 4c ships `HomeMountResolver` + `LocalStorageFactory` only. `Uploads` uses an opaque self-describing `upload_id` (base64 of `(path_prefix, dest_path, backend_upload_id)`) — no DB table, resumable across server restarts.

**Tech Stack:** Rust 1.95, `crabcloud-storage` + `crabcloud-filecache` + `crabcloud-config`, `tokio`, `async-trait`, `thiserror`, `tracing`, `base64`.

**Parent spec:** `docs/superpowers/specs/2026-05-12-mount-view-and-uploads-design.md` (merged at master `5f09f2c`).

**Plan-bug callout up front:** spec §3 + §6.7 said add `[storage] data_dir` to `crabcloud-config`. **`FileConfig.datadirectory: PathBuf` already exists** on master (it's the Nextcloud-compatible field name). The plan uses the existing `datadirectory` — no new config block. Documented in Batch E.

**Branch protection:** master is rules-gated (PR required); auto-merge enabled. Each batch lands as one PR; queue with `gh pr merge --squash --delete-branch --auto`.

---

## Conventions

- **Commits:** Conventional Commits (`feat(fs)`, `test(fs)`, `docs(fs)`) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD:** test → fail → implement → pass → commit.
- **rustfmt:** `cargo fmt --all` before push.
- **`cargo xtask check-all` must pass before push.**
- **`-D warnings` workspace-wide.** New deps get real call sites; if a dep is declared but unused in a batch, add `use X as _;` anchor.
- **One PR per batch.** Stop at "PR opened, awaiting merge."

---

## File Structure

```
crates/crabcloud-fs/                            NEW CRATE
├── Cargo.toml
└── src/
    ├── lib.rs                                  UserPath + FsError re-exports
    ├── error.rs                                FsError + FsResult
    ├── path.rs                                 UserPath newtype + tests
    ├── mount.rs                                Mount struct + MountResolver/StorageFactory traits
    ├── resolver/
    │   ├── mod.rs                              HomeMountResolver
    │   └── local.rs                            LocalStorageFactory (uses cfg.datadirectory)
    ├── view.rs                                 View struct + stat/list/read/put/mkdir/delete + rename/copy
    └── uploads.rs                              Uploads facade + UploadHandle + upload_id encode/decode

crates/crabcloud-fs/tests/                      Integration tests
├── support/
│   └── mod.rs                                  Test fixtures (harness, multi-mount resolver)
├── view_reads.rs                               Tests #1-#5 + #10
├── view_moves.rs                               Tests #6-#9
├── uploads.rs                                  Tests #11-#15
└── appstate_wiring.rs                          Test #16

crates/crabcloud-core/Cargo.toml                MODIFIED + crabcloud-fs dep
crates/crabcloud-core/src/state.rs              MODIFIED + mount_resolver field + view_for/uploads_for
Cargo.toml                                       MODIFIED + crabcloud-fs in workspace deps + members
README.md                                        MODIFIED + crabcloud-fs bullet (Batch F)
```

---

## Batches

| Batch | Tasks | Theme |
|-------|-------|---|
| **A** | 1 | crate skeleton + UserPath + Mount + FsError + MountResolver/StorageFactory + HomeMountResolver + LocalStorageFactory + unit tests |
| **B** | 2 | View read ops (stat/list/read/read_range/put_file/mkdir/delete) + 6 integration tests |
| **C** | 3 | View rename/copy + cross-mount error + 4 integration tests |
| **D** | 4 | Uploads façade + upload_id encode/decode + 5 integration tests |
| **E** | 5 | AppState wiring + view_for/uploads_for + test #16 |
| **F** | 6 | Acceptance docs (changelog + README + sub-project 5 prep notes) |

---

## Task 1: Crate skeleton + types (Batch A)

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/crabcloud-fs/Cargo.toml`
- Create: `crates/crabcloud-fs/src/lib.rs`
- Create: `crates/crabcloud-fs/src/error.rs`
- Create: `crates/crabcloud-fs/src/path.rs`
- Create: `crates/crabcloud-fs/src/mount.rs`
- Create: `crates/crabcloud-fs/src/resolver/mod.rs`
- Create: `crates/crabcloud-fs/src/resolver/local.rs`
- Create: `crates/crabcloud-fs/src/view.rs` (stub — implementation in Batch B)
- Create: `crates/crabcloud-fs/src/uploads.rs` (stub — implementation in Batch D)

### Step 1: Branch + workspace member + workspace deps

```
git checkout -b fs-batch-a origin/master
```

Modify root `Cargo.toml`. Find `[workspace] members = [...]` and add `"crates/crabcloud-fs",` alphabetically between `-filecache` and `-http`:

```toml
[workspace]
members = [
    "crates/crabcloud-cache",
    "crates/crabcloud-config",
    "crates/crabcloud-core",
    "crates/crabcloud-db",
    "crates/crabcloud-filecache",
    "crates/crabcloud-fs",
    "crates/crabcloud-http",
    "crates/crabcloud-i18n",
    "crates/crabcloud-ocs",
    "crates/crabcloud-server",
    "crates/crabcloud-storage",
    "crates/crabcloud-ui",
    "crates/crabcloud-users",
    "xtask",
]
```

Find `[workspace.dependencies]` and add (alphabetically near the other `crabcloud-*` entries):

```toml
crabcloud-fs = { path = "crates/crabcloud-fs" }
```

`base64` is already a workspace dep (`base64 = "0.22"`) — no addition needed.

### Step 2: Create `crates/crabcloud-fs/Cargo.toml`

```toml
[package]
name = "crabcloud-fs"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
base64.workspace = true
crabcloud-config.workspace = true
crabcloud-filecache.workspace = true
crabcloud-storage.workspace = true
crabcloud-users.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["fs", "io-util", "sync", "macros"] }
tracing.workspace = true

[dev-dependencies]
crabcloud-cache.workspace = true
crabcloud-config = { workspace = true, features = ["test-support"] }
crabcloud-db.workspace = true
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "fs", "io-util", "sync", "time"] }

[lints]
workspace = true
```

### Step 3: Create `crates/crabcloud-fs/src/error.rs`

```rust
//! Error types for `crabcloud-fs`.

use crabcloud_filecache::FileCacheError;
use crabcloud_storage::StorageError;

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
    Storage(#[from] StorageError),
    #[error("filecache: {0}")]
    FileCache(#[from] FileCacheError),
    #[error("upload: {0}")]
    Upload(String),
}

pub type FsResult<T> = Result<T, FsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_storage_error_wraps_as_storage() {
        let e: FsError = StorageError::NotFound.into();
        assert!(matches!(e, FsError::Storage(_)));
    }

    #[test]
    fn from_filecache_error_wraps() {
        let e: FsError = FileCacheError::NotFound.into();
        assert!(matches!(e, FsError::FileCache(_)));
    }

    #[test]
    fn cross_mount_message() {
        let s = format!("{}", FsError::CrossMount);
        assert!(s.contains("cross-mount"));
    }
}
```

### Step 4: Create `crates/crabcloud-fs/src/path.rs`

```rust
//! `UserPath` — user-facing absolute path under the user's filesystem root.
//!
//! Rules enforced at construction:
//! - **MUST start with `/`.**
//! - No `..` segments.
//! - No `.` segments.
//! - No empty segments (`/a//b`).
//! - No embedded NUL.
//! - No backslash.
//! - Forward-slash separator only.
//! - Max length 4096.
//! - Trailing slash stripped (`/foo/` → `/foo`).
//! - `UserPath::root() == "/"`; `is_root()` true.

use crate::error::{FsError, FsResult};

const MAX_PATH_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserPath(String);

impl UserPath {
    pub fn new(s: impl Into<String>) -> FsResult<Self> {
        let mut s: String = s.into();
        if s.len() > MAX_PATH_LEN {
            return Err(FsError::InvalidPath("path too long".into()));
        }
        if s.contains('\0') {
            return Err(FsError::InvalidPath("embedded NUL".into()));
        }
        if s.contains('\\') {
            return Err(FsError::InvalidPath("backslash is not a path separator".into()));
        }
        if !s.starts_with('/') {
            return Err(FsError::InvalidPath("user path must start with '/'".into()));
        }
        // Trim trailing slash unless this IS the root "/" (preserve single slash).
        while s.len() > 1 && s.ends_with('/') {
            s.pop();
        }
        // Validate every segment after the leading "/".
        if s.len() > 1 {
            for seg in s[1..].split('/') {
                if seg.is_empty() {
                    return Err(FsError::InvalidPath("empty segment".into()));
                }
                if seg == "." || seg == ".." {
                    return Err(FsError::InvalidPath(format!("illegal segment: {seg}")));
                }
            }
        }
        Ok(Self(s))
    }

    /// The user's filesystem root — `"/"`.
    pub fn root() -> Self {
        Self("/".into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    pub fn parent(&self) -> Option<UserPath> {
        if self.is_root() {
            return None;
        }
        match self.0.rfind('/') {
            // Last slash at position 0 → parent is root.
            Some(0) => Some(UserPath::root()),
            Some(i) => Some(UserPath(self.0[..i].to_string())),
            None => None, // Can't happen — `new` enforces leading slash.
        }
    }

    pub fn basename(&self) -> &str {
        if self.is_root() {
            return "";
        }
        match self.0.rfind('/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    pub fn join(&self, child: &str) -> FsResult<UserPath> {
        let combined = if self.is_root() {
            format!("/{}", child)
        } else {
            format!("{}/{}", self.0, child)
        };
        UserPath::new(combined)
    }
}

impl std::fmt::Display for UserPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_slash() {
        let r = UserPath::root();
        assert_eq!(r.as_str(), "/");
        assert!(r.is_root());
        assert!(r.parent().is_none());
        assert_eq!(r.basename(), "");
    }

    #[test]
    fn simple_path_parses() {
        let p = UserPath::new("/photos/cat.jpg").unwrap();
        assert_eq!(p.as_str(), "/photos/cat.jpg");
        assert_eq!(p.basename(), "cat.jpg");
        assert_eq!(p.parent().unwrap().as_str(), "/photos");
    }

    #[test]
    fn trailing_slash_stripped() {
        let p = UserPath::new("/photos/").unwrap();
        assert_eq!(p.as_str(), "/photos");
    }

    #[test]
    fn multiple_trailing_slashes_stripped() {
        let p = UserPath::new("/photos///").unwrap();
        assert_eq!(p.as_str(), "/photos");
    }

    #[test]
    fn root_path_preserved() {
        // "/" alone should NOT have its slash stripped.
        let p = UserPath::new("/").unwrap();
        assert_eq!(p.as_str(), "/");
        assert!(p.is_root());
    }

    #[test]
    fn missing_leading_slash_rejected() {
        assert!(matches!(
            UserPath::new("photos/cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn parent_dot_dot_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos/../etc"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn current_dot_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos/./cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn empty_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos//cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn embedded_nul_rejected() {
        assert!(matches!(
            UserPath::new("/a\0b"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn backslash_rejected() {
        assert!(matches!(
            UserPath::new("/a\\b"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn too_long_rejected() {
        let big = "/".to_string() + &"a".repeat(5000);
        assert!(matches!(UserPath::new(big), Err(FsError::InvalidPath(_))));
    }

    #[test]
    fn empty_string_rejected() {
        assert!(matches!(UserPath::new(""), Err(FsError::InvalidPath(_))));
    }

    #[test]
    fn parent_of_top_level_returns_root() {
        let p = UserPath::new("/file.txt").unwrap();
        assert_eq!(p.parent().unwrap().as_str(), "/");
        assert_eq!(p.basename(), "file.txt");
    }

    #[test]
    fn join_onto_root() {
        let p = UserPath::root().join("a").unwrap();
        assert_eq!(p.as_str(), "/a");
    }

    #[test]
    fn join_onto_path() {
        let p = UserPath::new("/a/b").unwrap().join("c.txt").unwrap();
        assert_eq!(p.as_str(), "/a/b/c.txt");
    }

    #[test]
    fn join_validates_child() {
        let p = UserPath::new("/a").unwrap();
        assert!(p.join("../escape").is_err());
    }
}
```

### Step 5: Create `crates/crabcloud-fs/src/mount.rs`

```rust
//! Mount + MountResolver + StorageFactory traits.
//!
//! A `Mount` binds a user-facing path prefix to a `Storage` backend. The
//! `MountResolver` is queried per-request to get the active mounts for a
//! user. `StorageFactory` is the per-backend constructor (local FS, future
//! S3, external storage).

use crabcloud_storage::{Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;

#[derive(Clone)]
pub struct Mount {
    /// User-facing path prefix. Empty (`StoragePath::root()`) for the home
    /// mount. Non-empty for share / external storage mounts in future
    /// sub-projects (e.g., `"Shared"` for `/Shared/...`).
    pub path_prefix: StoragePath,
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

### Step 6: Create `crates/crabcloud-fs/src/resolver/mod.rs`

```rust
//! `HomeMountResolver` — the 4c default. One home mount per user, anchored
//! at the root. Forward-design: future resolvers can layer share + external
//! mounts on top.

pub mod local;

use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;
use crate::mount::{Mount, MountResolver, StorageFactory};

pub struct HomeMountResolver {
    factory: Arc<dyn StorageFactory>,
}

impl HomeMountResolver {
    pub fn new(factory: Arc<dyn StorageFactory>) -> Self {
        Self { factory }
    }
}

#[async_trait::async_trait]
impl MountResolver for HomeMountResolver {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>> {
        let storage = self.factory.home_storage(uid).await?;
        Ok(vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    struct MemoryFactory;

    #[async_trait::async_trait]
    impl StorageFactory for MemoryFactory {
        async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
            Ok(Arc::new(MemoryStorage::new(uid.as_str())))
        }
    }

    #[tokio::test]
    async fn home_resolver_returns_single_mount_at_root() {
        let resolver = HomeMountResolver::new(Arc::new(MemoryFactory));
        let uid = UserId::new("alice").unwrap();
        let mounts = resolver.mounts_for(&uid).await.unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].path_prefix.is_root());
        assert_eq!(mounts[0].storage.id(), "memory::alice");
    }
}
```

### Step 7: Create `crates/crabcloud-fs/src/resolver/local.rs`

```rust
//! `LocalStorageFactory` — backs each user's home with `<data_dir>/<uid>/files`.
//! `data_dir` comes from `FileConfig.datadirectory` (the existing Nextcloud-
//! compatible field; spec called it `[storage] data_dir` but the existing
//! field has identical semantics).

use crabcloud_storage::local::LocalStorage;
use crabcloud_storage::Storage;
use crabcloud_users::UserId;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{FsError, FsResult};
use crate::mount::StorageFactory;

pub struct LocalStorageFactory {
    data_dir: PathBuf,
}

impl LocalStorageFactory {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait::async_trait]
impl StorageFactory for LocalStorageFactory {
    async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
        let home = self.data_dir.join(uid.as_str()).join("files");
        tokio::fs::create_dir_all(&home)
            .await
            .map_err(|e| FsError::Storage(crabcloud_storage::StorageError::Io(e)))?;
        let storage = LocalStorage::new(home).map_err(FsError::Storage)?;
        Ok(Arc::new(storage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn home_storage_creates_path() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let uid = UserId::new("alice").unwrap();
        let storage = factory.home_storage(&uid).await.unwrap();
        // The storage's id is `local::<canonicalized-path>`. We verify the path
        // ends with `alice/files`.
        assert!(
            storage.id().ends_with("alice/files")
                || storage.id().ends_with(r"alice\files"),
            "unexpected storage id: {}",
            storage.id()
        );
    }

    #[tokio::test]
    async fn home_storage_idempotent() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let uid = UserId::new("alice").unwrap();
        let s1 = factory.home_storage(&uid).await.unwrap();
        let s2 = factory.home_storage(&uid).await.unwrap();
        assert_eq!(s1.id(), s2.id());
    }

    #[tokio::test]
    async fn home_storage_distinct_users_distinct_storages() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let alice = factory
            .home_storage(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        let bob = factory
            .home_storage(&UserId::new("bob").unwrap())
            .await
            .unwrap();
        assert_ne!(alice.id(), bob.id());
    }
}
```

### Step 8: Create `crates/crabcloud-fs/src/view.rs` (stub)

```rust
//! `View` — per-user filesystem façade. Implementation lands in Batches B + C.

use crate::error::FsResult;
use crate::mount::Mount;
use crabcloud_filecache::FileCache;
use crabcloud_storage::ChannelEventSink;
use crabcloud_users::UserId;
use std::sync::Arc;

pub struct View {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) filecache: Arc<FileCache>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
    ) -> Self {
        Self {
            uid,
            mounts,
            filecache,
            storage_sink,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }
}

// Operations land in Batch B (reads) and Batch C (rename/copy). Marker
// import to keep `FsResult` in scope without warnings.
#[allow(dead_code)]
fn _typecheck() -> FsResult<()> {
    Ok(())
}
```

### Step 9: Create `crates/crabcloud-fs/src/uploads.rs` (stub)

```rust
//! `Uploads` — chunked upload facade. Implementation lands in Batch D.

use crate::error::FsResult;
use crate::mount::Mount;
use crabcloud_filecache::FileCache;
use crabcloud_storage::ChannelEventSink;
use crabcloud_users::UserId;
use std::sync::Arc;

pub struct Uploads {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
    pub(crate) filecache: Arc<FileCache>,
}

impl Uploads {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        storage_sink: Arc<ChannelEventSink>,
        filecache: Arc<FileCache>,
    ) -> Self {
        Self {
            uid,
            mounts,
            storage_sink,
            filecache,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }
}

#[allow(dead_code)]
fn _typecheck() -> FsResult<()> {
    Ok(())
}
```

### Step 10: Create `crates/crabcloud-fs/src/lib.rs`

```rust
//! `crabcloud-fs` — per-user filesystem façade.
//!
//! The [`View`] resolves user-facing paths (`/photos/cat.jpg`) to the
//! appropriate `(Storage, StoragePath)` tuple via the user's mounts, then
//! routes reads through [`FileCache`] and writes through the storage backend
//! (which emits events on the shared `ChannelEventSink`).
//!
//! The [`Uploads`] façade translates Nextcloud's chunked-upload HTTP protocol
//! (`/dav/uploads/<user>/<upload_id>/<n>` PUTs + MOVE-with-Destination) to
//! the Storage trait's multipart primitives.
//!
//! `MountResolver` + `StorageFactory` traits are forward-designed for share
//! and external storage mounts; sub-project 4c only ships `HomeMountResolver`
//! + `LocalStorageFactory`.

pub mod error;
pub mod mount;
pub mod path;
pub mod resolver;
pub mod uploads;
pub mod view;

pub use error::{FsError, FsResult};
pub use mount::{Mount, MountResolver, StorageFactory};
pub use path::UserPath;
pub use resolver::local::LocalStorageFactory;
pub use resolver::HomeMountResolver;
pub use uploads::Uploads;
pub use view::View;

// Anchor workspace deps whose real call sites land in Batches B–D. Each
// anchor goes away as the corresponding feature is wired up.
use base64 as _; // used in Batch D (upload_id encode/decode)
use crabcloud_config as _; // used in Batch E (datadirectory resolution + AppState)
use tokio as _; // used in Batch B via async stream IO
use tracing as _; // used in Batches B-D for warn!/info!
```

### Step 11: Run + commit + push + open Batch A PR

```
cargo build -p crabcloud-fs
cargo test -p crabcloud-fs --lib
cargo xtask check-all
```

Expected: builds clean; ~22 unit tests pass (error: 3, path: 17, resolver: 4 = 24 — adjust the doc-counts post-implementation if the counts shift due to clippy fixes); workspace check-all green.

```
git add Cargo.toml crates/crabcloud-fs
git commit -m "feat(fs): crabcloud-fs crate skeleton — types + MountResolver + factories

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-a
gh pr create --base master --head fs-batch-a \
  --title "fs: batch A — crate skeleton + UserPath + Mount + resolvers" \
  --body "Sub-project 4c, batch A: new \`crabcloud-fs\` crate with \`UserPath\` newtype (leading-/ required, ..-rejected, ≤4096 chars), \`Mount\` struct + \`MountResolver\`/\`StorageFactory\` traits, \`HomeMountResolver\` (one home mount per user), \`LocalStorageFactory\` (<data_dir>/<uid>/files). View/Uploads are stubs — implementations land in B–D. ~22 unit tests cover path normalization + resolver construction."
```

**STOP.** Do NOT call `gh pr merge`.

---

## Task 2: View read operations (Batch B)

**Files:**
- Modify: `crates/crabcloud-fs/src/view.rs` (replace stub)
- Create: `crates/crabcloud-fs/tests/support/mod.rs`
- Create: `crates/crabcloud-fs/tests/view_reads.rs`

### Step 1: Branch

```
git checkout -b fs-batch-b origin/master
```

### Step 2: Replace `crates/crabcloud-fs/src/view.rs`

```rust
//! `View` — per-user filesystem façade. Resolves user paths to
//! `(Mount, StoragePath)` via longest-prefix match; reads route through
//! the `FileCache`; writes go to storage with events emitted via the
//! shared `ChannelEventSink`.

use crate::error::{FsError, FsResult};
use crate::mount::Mount;
use crate::path::UserPath;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{
    ChannelEventSink, DirEntry, FileMetadata, StoragePath,
};
use crabcloud_users::UserId;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

pub struct View {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) filecache: Arc<FileCache>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
    ) -> Self {
        Self {
            uid,
            mounts,
            filecache,
            storage_sink,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// Resolve a user-facing path to the responsible mount + the storage-
    /// relative path under that mount.
    ///
    /// Longest-prefix match against `self.mounts`. Strips the mount's
    /// `path_prefix` to produce the storage-relative `StoragePath`. Errors
    /// `MountNotFound` if no mount matches (shouldn't happen with a home
    /// mount anchored at `/`).
    pub(crate) fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)> {
        // Strip leading `/` — `UserPath` guarantees one.
        let trimmed = user_path.as_str().trim_start_matches('/');
        let best = self
            .mounts
            .iter()
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
            trimmed
                .strip_prefix(&with_slash)
                .map(String::from)
                .unwrap_or_default()
        };
        let storage_path = StoragePath::new(suffix)?;
        Ok((best, storage_path))
    }

    /// Cached stat. Routes through `FileCache::stat` which populates on
    /// miss via the backing storage.
    pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let meta = self.filecache.stat(&mount.storage, &storage_path).await?;
        Ok(meta)
    }

    /// Cached directory listing.
    pub async fn list(&self, user_path: &UserPath) -> FsResult<Vec<DirEntry>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let entries = self.filecache.list(&mount.storage, &storage_path).await?;
        Ok(entries)
    }

    pub async fn read(
        &self,
        user_path: &UserPath,
    ) -> FsResult<Pin<Box<dyn AsyncRead + Send>>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let r = mount.storage.read(&storage_path).await?;
        Ok(r)
    }

    pub async fn read_range(
        &self,
        user_path: &UserPath,
        range: Range<u64>,
    ) -> FsResult<Pin<Box<dyn AsyncRead + Send>>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let r = mount.storage.read_range(&storage_path, range).await?;
        Ok(r)
    }

    /// Write through the storage backend. The storage emits a `Written`
    /// event on `storage_sink`; the scanner asynchronously updates the
    /// filecache. The caller gets the storage's fresh `FileMetadata`
    /// directly — no need to wait for the scanner to catch up.
    pub async fn put_file(
        &self,
        user_path: &UserPath,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let meta = mount
            .storage
            .put_file(&storage_path, body, &*self.storage_sink)
            .await?;
        Ok(meta)
    }

    pub async fn mkdir(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let meta = mount
            .storage
            .mkdir(&storage_path, &*self.storage_sink)
            .await?;
        Ok(meta)
    }

    pub async fn delete(&self, user_path: &UserPath) -> FsResult<()> {
        let (mount, storage_path) = self.resolve(user_path)?;
        mount
            .storage
            .delete(&storage_path, &*self.storage_sink)
            .await?;
        Ok(())
    }

    // rename / copy land in Batch C.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    fn build_view_with_mounts(mounts: Vec<Mount>) -> View {
        // Construct a minimal View for resolve()-only unit tests. The
        // filecache + storage_sink are unused on the resolve path; we
        // use a Storage-less stub that satisfies the type but never
        // sees a method call.
        //
        // For unit-testing resolve only, we build with dummy fields the
        // compiler accepts. Integration tests in `tests/view_reads.rs`
        // exercise real stat/list/etc.
        use crabcloud_cache::MemoryCache;
        use crabcloud_db::{core_set, DbPool, MigrationRunner};
        use crabcloud_storage::ChannelEventSink;

        // Build a stub pool synchronously for resolve-only tests by
        // tokio::runtime block_on. This is acceptable in a small unit
        // test; integration tests use the async harness in tests/support.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pool = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let cfg = crabcloud_config::test_support::minimal_sqlite_config(
                dir.path().join("v.db"),
            );
            std::mem::forget(dir);
            let pool = DbPool::connect(&cfg).await.unwrap();
            let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
            runner.register(core_set());
            runner.run().await.unwrap();
            pool
        });
        let _ = MemoryCache::new(); // anchor crabcloud_cache

        View::new(
            UserId::new("alice").unwrap(),
            mounts,
            Arc::new(FileCache::new(pool)),
            Arc::new(ChannelEventSink::new(8)),
        )
    }

    #[test]
    fn resolve_home_mount_strips_leading_slash() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        let mount = Mount {
            path_prefix: StoragePath::root(),
            storage,
        };
        let view = build_view_with_mounts(vec![mount]);
        let (m, sp) = view
            .resolve(&UserPath::new("/photos/cat.jpg").unwrap())
            .unwrap();
        assert!(m.path_prefix.is_root());
        assert_eq!(sp.as_str(), "photos/cat.jpg");
    }

    #[test]
    fn resolve_root_user_path_yields_storage_root() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        let mount = Mount {
            path_prefix: StoragePath::root(),
            storage,
        };
        let view = build_view_with_mounts(vec![mount]);
        let (_, sp) = view.resolve(&UserPath::root()).unwrap();
        assert!(sp.is_root());
    }

    #[test]
    fn resolve_picks_longest_matching_prefix() {
        let s1: Arc<dyn Storage> = Arc::new(MemoryStorage::new("home"));
        let s2: Arc<dyn Storage> = Arc::new(MemoryStorage::new("shared"));
        let mounts = vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: s1,
            },
            Mount {
                path_prefix: StoragePath::new("Shared").unwrap(),
                storage: s2,
            },
        ];
        let view = build_view_with_mounts(mounts);
        let (m, sp) = view
            .resolve(&UserPath::new("/Shared/joe/photo.jpg").unwrap())
            .unwrap();
        assert_eq!(m.storage.id(), "memory::shared");
        assert_eq!(sp.as_str(), "joe/photo.jpg");
    }

    #[test]
    fn resolve_no_match_errors() {
        // Empty mounts list — pathological but the wire shape is set.
        let view = build_view_with_mounts(vec![]);
        let r = view.resolve(&UserPath::new("/a").unwrap());
        assert!(matches!(r, Err(FsError::MountNotFound)));
    }
}
```

### Step 3: Create `crates/crabcloud-fs/tests/support/mod.rs`

```rust
//! Shared test fixtures for `crabcloud-fs` integration tests.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::FileCache;
use crabcloud_fs::{Mount, View};
use crabcloud_storage::{memory::MemoryStorage, ChannelEventSink, Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;
use tempfile::TempDir;

pub struct Harness {
    pub pool: DbPool,
    pub filecache: Arc<FileCache>,
    pub sink: Arc<ChannelEventSink>,
    pub storage: Arc<dyn Storage>,
    pub _tempdir: TempDir,
}

pub async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("h.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let filecache = Arc::new(FileCache::new(pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    Harness {
        pool,
        filecache,
        sink,
        storage,
        _tempdir: dir,
    }
}

/// Build a single-home-mount `View` against the harness's storage.
pub fn view_home(h: &Harness) -> View {
    View::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: h.storage.clone(),
        }],
        h.filecache.clone(),
        h.sink.clone(),
    )
}

/// Build a 2-mount view: home at `/` + a synthetic mount at `/Shared`.
/// Used to exercise the cross-mount error path in Batch C tests.
pub fn view_with_two_mounts(h: &Harness) -> View {
    let shared: Arc<dyn Storage> = Arc::new(MemoryStorage::new("shared"));
    View::new(
        UserId::new("alice").unwrap(),
        vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: h.storage.clone(),
            },
            Mount {
                path_prefix: StoragePath::new("Shared").unwrap(),
                storage: shared,
            },
        ],
        h.filecache.clone(),
        h.sink.clone(),
    )
}
```

### Step 4: Create `crates/crabcloud-fs/tests/view_reads.rs`

```rust
mod support;

use crabcloud_fs::{FsError, UserPath};
use crabcloud_storage::{FileKind, NoopEventSink, Storage, StoragePath};
use support::{harness, view_home};
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn view_stat_returns_metadata_for_existing_file() {
    let h = harness().await;
    // Seed via the storage directly (NoopEventSink — the View's stat goes
    // through cache populate on miss, hitting the backend stat).
    h.storage
        .put_file(
            &StoragePath::new("hello.txt").unwrap(),
            body(b"hi".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let view = view_home(&h);
    let meta = view.stat(&UserPath::new("/hello.txt").unwrap()).await.unwrap();
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.size, 2);
}

#[tokio::test]
async fn view_put_then_read_roundtrip() {
    let h = harness().await;
    let view = view_home(&h);

    let meta = view
        .put_file(&UserPath::new("/hi.txt").unwrap(), body(b"hello".to_vec()))
        .await
        .unwrap();
    assert_eq!(meta.size, 5);
    let fresh_etag = meta.etag.clone();

    // Read back via the View.
    let mut reader = view.read(&UserPath::new("/hi.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hello");

    // The fresh etag is returned by put_file directly (no scanner lag).
    assert_eq!(fresh_etag.as_str().len(), 40);
}

#[tokio::test]
async fn view_list_returns_children() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/d").unwrap()).await.unwrap();
    view.put_file(&UserPath::new("/d/a.txt").unwrap(), body(b"a".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/b.txt").unwrap(), body(b"b".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/c.txt").unwrap(), body(b"c".to_vec()))
        .await
        .unwrap();

    let entries = view.list(&UserPath::new("/d").unwrap()).await.unwrap();
    assert_eq!(entries.len(), 3);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"c.txt"));
}

#[tokio::test]
async fn view_mkdir_creates_directory() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/newdir").unwrap()).await.unwrap();
    let meta = view.stat(&UserPath::new("/newdir").unwrap()).await.unwrap();
    assert_eq!(meta.kind, FileKind::Directory);
}

#[tokio::test]
async fn view_delete_removes_file() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(&UserPath::new("/del.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    view.delete(&UserPath::new("/del.txt").unwrap()).await.unwrap();
    let r = view.stat(&UserPath::new("/del.txt").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound))
    ));
}

#[tokio::test]
async fn view_delete_removes_empty_directory() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/empty").unwrap()).await.unwrap();
    view.delete(&UserPath::new("/empty").unwrap()).await.unwrap();
    let r = view.stat(&UserPath::new("/empty").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound))
    ));
}

#[tokio::test]
async fn view_invalid_user_path_rejected() {
    // No leading slash.
    assert!(matches!(
        UserPath::new("photos/cat.jpg"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_path_escape_via_dotdot_rejected() {
    // Path escape via .. is caught at UserPath construction.
    assert!(matches!(
        UserPath::new("/photos/../../etc/passwd"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_read_range_returns_slice() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(
        &UserPath::new("/range.txt").unwrap(),
        body(b"abcdefghij".to_vec()),
    )
    .await
    .unwrap();
    let mut reader = view
        .read_range(&UserPath::new("/range.txt").unwrap(), 2..5)
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"cde");
}
```

### Step 5: Run + commit + push + open Batch B PR

```
cargo test -p crabcloud-fs --tests
cargo xtask check-all
```

Expected: 9 new integration tests pass + 4 unit tests in view.rs + previous Batch A tests still pass.

```
git add crates/crabcloud-fs
git commit -m "feat(fs): View read operations (stat/list/read/put/mkdir/delete)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-b
gh pr create --base master --head fs-batch-b \
  --title "fs: batch B — View read ops + path resolution" \
  --body "Sub-project 4c, batch B: View::stat/list/read/read_range/put_file/mkdir/delete. Longest-prefix mount resolution. Reads route through FileCache; writes emit through ChannelEventSink. 9 integration tests cover the full read+write surface + path-escape rejection. rename/copy land in Batch C."
```

**STOP.**

---

## Task 3: View rename + copy (Batch C)

**Files:**
- Modify: `crates/crabcloud-fs/src/view.rs` (append rename + copy methods)
- Create: `crates/crabcloud-fs/tests/view_moves.rs`

### Step 1: Branch

```
git checkout -b fs-batch-c origin/master
```

### Step 2: Append rename + copy to `crates/crabcloud-fs/src/view.rs`

Find the `// rename / copy land in Batch C.` line and replace with:

```rust
    /// Within-mount rename. Errors `FsError::CrossMount` if `from` and
    /// `to` resolve to different mounts (4c only ships one mount per
    /// user; this can't fire in practice but the wire shape is set).
    pub async fn rename(&self, from: &UserPath, to: &UserPath) -> FsResult<()> {
        let (from_mount, from_path) = self.resolve(from)?;
        let (to_mount, to_path) = self.resolve(to)?;
        if from_mount.path_prefix != to_mount.path_prefix {
            return Err(FsError::CrossMount);
        }
        from_mount
            .storage
            .rename(&from_path, &to_path, &*self.storage_sink)
            .await?;
        Ok(())
    }

    /// Within-mount copy. Same cross-mount restriction.
    pub async fn copy(&self, from: &UserPath, to: &UserPath) -> FsResult<()> {
        let (from_mount, from_path) = self.resolve(from)?;
        let (to_mount, to_path) = self.resolve(to)?;
        if from_mount.path_prefix != to_mount.path_prefix {
            return Err(FsError::CrossMount);
        }
        from_mount
            .storage
            .copy(&from_path, &to_path, &*self.storage_sink)
            .await?;
        Ok(())
    }
```

### Step 3: Create `crates/crabcloud-fs/tests/view_moves.rs`

```rust
mod support;

use crabcloud_fs::{FsError, UserPath};
use support::{harness, view_home, view_with_two_mounts};
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn view_rename_within_mount_moves_file() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(&UserPath::new("/from.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();

    view.rename(
        &UserPath::new("/from.txt").unwrap(),
        &UserPath::new("/to.txt").unwrap(),
    )
    .await
    .unwrap();

    let from_stat = view.stat(&UserPath::new("/from.txt").unwrap()).await;
    assert!(matches!(
        from_stat,
        Err(FsError::FileCache(crabcloud_filecache::FileCacheError::NotFound))
    ));

    let mut reader = view.read(&UserPath::new("/to.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"x");
}

#[tokio::test]
async fn view_copy_within_mount_preserves_source_and_creates_dest() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(
        &UserPath::new("/src.txt").unwrap(),
        body(b"copy-me".to_vec()),
    )
    .await
    .unwrap();
    let src_meta = view.stat(&UserPath::new("/src.txt").unwrap()).await.unwrap();

    view.copy(
        &UserPath::new("/src.txt").unwrap(),
        &UserPath::new("/dst.txt").unwrap(),
    )
    .await
    .unwrap();

    // Source still exists.
    let mut reader = view.read(&UserPath::new("/src.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"copy-me");

    // Dest has the same contents but a fresh ETag.
    let mut reader = view.read(&UserPath::new("/dst.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"copy-me");

    let dst_meta = view.stat(&UserPath::new("/dst.txt").unwrap()).await.unwrap();
    assert_ne!(src_meta.etag, dst_meta.etag);
}

#[tokio::test]
async fn view_rename_cross_mount_errors() {
    let h = harness().await;
    let view = view_with_two_mounts(&h);
    // Set up a file in the home mount.
    view.put_file(&UserPath::new("/from.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    // /Shared is a different mount. Attempting to rename across should fail.
    let r = view
        .rename(
            &UserPath::new("/from.txt").unwrap(),
            &UserPath::new("/Shared/to.txt").unwrap(),
        )
        .await;
    assert!(matches!(r, Err(FsError::CrossMount)));
}

#[tokio::test]
async fn view_copy_cross_mount_errors() {
    let h = harness().await;
    let view = view_with_two_mounts(&h);
    view.put_file(&UserPath::new("/src.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    let r = view
        .copy(
            &UserPath::new("/src.txt").unwrap(),
            &UserPath::new("/Shared/dst.txt").unwrap(),
        )
        .await;
    assert!(matches!(r, Err(FsError::CrossMount)));
}
```

### Step 4: Run + commit + push + open Batch C PR

```
cargo test -p crabcloud-fs --tests
cargo xtask check-all
```

Expected: 4 new integration tests pass; prior tests still pass.

```
git add crates/crabcloud-fs
git commit -m "feat(fs): View rename + copy + cross-mount error

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-c
gh pr create --base master --head fs-batch-c \
  --title "fs: batch C — View rename/copy + cross-mount error" \
  --body "Sub-project 4c, batch C: View::rename and View::copy. Cross-mount operations error FsError::CrossMount (forward-design for future share mounts; can't fire for 4c's home-only resolver). 4 integration tests cover within-mount happy paths + cross-mount rejection via a synthetic 2-mount test fixture."
```

**STOP.**

---

## Task 4: Uploads façade (Batch D)

**Files:**
- Modify: `crates/crabcloud-fs/src/uploads.rs` (replace stub)
- Modify: `crates/crabcloud-fs/src/lib.rs` (remove `base64 as _` anchor — real call site lands here)
- Create: `crates/crabcloud-fs/tests/uploads.rs`

### Step 1: Branch

```
git checkout -b fs-batch-d origin/master
```

### Step 2: Replace `crates/crabcloud-fs/src/uploads.rs`

```rust
//! `Uploads` — chunked upload façade. Translates Nextcloud's chunked-upload
//! HTTP protocol (PUT chunks to `/dav/uploads/<user>/<upload_id>/<n>` +
//! MOVE-with-Destination to commit) into the Storage trait's multipart
//! primitives.
//!
//! The `upload_id` returned to the client is opaque + self-describing.
//! Format: `"{path_prefix_b64}:{dest_path_b64}:{backend_upload_id}"`. Each
//! `*_b64` is URL-safe base64 of the raw UTF-8 string. The backend id is
//! whatever the storage backend returned (e.g., `local-mp-<random>` for
//! LocalStorage).

use crate::error::{FsError, FsResult};
use crate::mount::Mount;
use crate::path::UserPath;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{
    ChannelEventSink, FileMetadata, MultipartHandle, PartTag, StoragePath,
};
use crabcloud_users::UserId;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

pub struct Uploads {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
    pub(crate) filecache: Arc<FileCache>,
}

#[derive(Debug, Clone)]
pub struct UploadHandle {
    /// Opaque, self-describing upload id. Pass back to `put_part`/`commit`/
    /// `abort`. Survives server restarts as long as the backing storage's
    /// multipart state survives (LocalStorage tempdir / S3 UploadId).
    pub upload_id: String,
    pub destination: UserPath,
}

impl Uploads {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        storage_sink: Arc<ChannelEventSink>,
        filecache: Arc<FileCache>,
    ) -> Self {
        Self {
            uid,
            mounts,
            storage_sink,
            filecache,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)> {
        // Same algorithm as View::resolve. Duplicated to keep Uploads
        // independent of View — they share a trait surface naturally
        // (both consume `Vec<Mount>`).
        let trimmed = user_path.as_str().trim_start_matches('/');
        let best = self
            .mounts
            .iter()
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
            trimmed
                .strip_prefix(&with_slash)
                .map(String::from)
                .unwrap_or_default()
        };
        let storage_path = StoragePath::new(suffix)?;
        Ok((best, storage_path))
    }

    /// Begin a new upload. Returns an opaque `upload_id`.
    pub async fn begin(&self, destination: &UserPath) -> FsResult<UploadHandle> {
        let (mount, storage_path) = self.resolve(destination)?;
        let handle = mount
            .storage
            .begin_multipart(&storage_path, &*self.storage_sink)
            .await?;
        let upload_id = encode_upload_id(&mount.path_prefix, &storage_path, &handle.upload_id);
        Ok(UploadHandle {
            upload_id,
            destination: destination.clone(),
        })
    }

    /// Receive a chunk for an in-progress upload.
    pub async fn put_part(
        &self,
        upload_id: &str,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<PartTag> {
        let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let tag = mount.storage.put_part(&handle, part_number, body).await?;
        Ok(tag)
    }

    /// Abort an in-progress upload. Idempotent on unknown `upload_id`.
    pub async fn abort(&self, upload_id: &str) -> FsResult<()> {
        let (mount, storage_path, backend_id) =
            match decode_upload_id(upload_id, &self.mounts) {
                Ok(x) => x,
                Err(_) => return Ok(()),
            };
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let _ = mount.storage.abort_multipart(handle).await;
        Ok(())
    }

    /// Commit the upload at the supplied destination. Errors if
    /// `destination` doesn't match what was passed to `begin`.
    pub async fn commit(
        &self,
        upload_id: &str,
        destination: &UserPath,
        parts: Vec<PartTag>,
    ) -> FsResult<FileMetadata> {
        let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
        let (dest_mount, dest_path) = self.resolve(destination)?;
        if dest_mount.path_prefix != mount.path_prefix || dest_path != storage_path {
            return Err(FsError::Upload("destination mismatch".into()));
        }
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let meta = mount
            .storage
            .commit_multipart(handle, parts, &*self.storage_sink)
            .await?;
        Ok(meta)
    }
}

fn encode_upload_id(prefix: &StoragePath, dest: &StoragePath, backend: &str) -> String {
    let p = URL_SAFE_NO_PAD.encode(prefix.as_str().as_bytes());
    let d = URL_SAFE_NO_PAD.encode(dest.as_str().as_bytes());
    let b = URL_SAFE_NO_PAD.encode(backend.as_bytes());
    format!("{p}:{d}:{b}")
}

/// Decode an `upload_id` produced by `encode_upload_id`. Returns
/// `(mount, dest_path, backend_upload_id)`. Errors if the id is malformed
/// or if no mount matches the encoded prefix.
fn decode_upload_id<'m>(
    encoded: &str,
    mounts: &'m [Mount],
) -> FsResult<(&'m Mount, StoragePath, String)> {
    let parts: Vec<&str> = encoded.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(FsError::Upload("malformed upload id".into()));
    }
    let prefix_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|_| FsError::Upload("malformed upload id (prefix not base64)".into()))?;
    let dest_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| FsError::Upload("malformed upload id (dest not base64)".into()))?;
    let backend_bytes = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| FsError::Upload("malformed upload id (backend not base64)".into()))?;
    let prefix_str =
        String::from_utf8(prefix_bytes).map_err(|_| FsError::Upload("prefix not utf-8".into()))?;
    let dest_str =
        String::from_utf8(dest_bytes).map_err(|_| FsError::Upload("dest not utf-8".into()))?;
    let backend_str = String::from_utf8(backend_bytes)
        .map_err(|_| FsError::Upload("backend not utf-8".into()))?;

    let mount = mounts
        .iter()
        .find(|m| m.path_prefix.as_str() == prefix_str)
        .ok_or_else(|| FsError::Upload("unknown mount".into()))?;
    let storage_path =
        StoragePath::new(dest_str).map_err(|e| FsError::Upload(format!("invalid dest: {e}")))?;
    Ok((mount, storage_path, backend_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    fn mount_for(prefix: &str, id: &str) -> Mount {
        let p = if prefix.is_empty() {
            StoragePath::root()
        } else {
            StoragePath::new(prefix).unwrap()
        };
        Mount {
            path_prefix: p,
            storage: Arc::new(MemoryStorage::new(id)) as Arc<dyn Storage>,
        }
    }

    #[test]
    fn upload_id_round_trip_root_mount() {
        let prefix = StoragePath::root();
        let dest = StoragePath::new("photos/cat.jpg").unwrap();
        let backend = "local-mp-abc123";
        let encoded = encode_upload_id(&prefix, &dest, backend);
        assert!(encoded.contains(':'));

        let mounts = vec![mount_for("", "home")];
        let (mount, decoded_dest, decoded_backend) =
            decode_upload_id(&encoded, &mounts).unwrap();
        assert_eq!(mount.path_prefix, prefix);
        assert_eq!(decoded_dest, dest);
        assert_eq!(decoded_backend, backend);
    }

    #[test]
    fn upload_id_round_trip_shared_mount() {
        let prefix = StoragePath::new("Shared").unwrap();
        let dest = StoragePath::new("joe/photos/cat.jpg").unwrap();
        let backend = "local-mp-xyz";
        let encoded = encode_upload_id(&prefix, &dest, backend);

        let mounts = vec![
            mount_for("", "home"),
            mount_for("Shared", "shared"),
        ];
        let (mount, decoded_dest, decoded_backend) =
            decode_upload_id(&encoded, &mounts).unwrap();
        assert_eq!(mount.path_prefix.as_str(), "Shared");
        assert_eq!(decoded_dest, dest);
        assert_eq!(decoded_backend, backend);
    }

    #[test]
    fn malformed_upload_id_rejected() {
        let mounts = vec![mount_for("", "home")];
        assert!(matches!(
            decode_upload_id("not-base64", &mounts),
            Err(FsError::Upload(_))
        ));
        assert!(matches!(
            decode_upload_id("a:b", &mounts),
            Err(FsError::Upload(_))
        ));
        assert!(matches!(
            decode_upload_id("!@:#$:%^", &mounts),
            Err(FsError::Upload(_))
        ));
    }

    #[test]
    fn unknown_mount_prefix_rejected() {
        let mounts = vec![mount_for("", "home")];
        // Encode with a "Phantom" prefix that doesn't exist in mounts.
        let encoded = encode_upload_id(
            &StoragePath::new("Phantom").unwrap(),
            &StoragePath::new("x").unwrap(),
            "id",
        );
        assert!(matches!(
            decode_upload_id(&encoded, &mounts),
            Err(FsError::Upload(_))
        ));
    }
}
```

### Step 3: Remove the `base64 as _` anchor in lib.rs

In `crates/crabcloud-fs/src/lib.rs`, remove the line:

```rust
use base64 as _; // used in Batch D (upload_id encode/decode)
```

(Uploads now references `base64` directly.)

### Step 4: Create `crates/crabcloud-fs/tests/uploads.rs`

```rust
mod support;

use crabcloud_fs::{FsError, Uploads, UserPath};
use crabcloud_storage::{Mount as _, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;
use support::harness;
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

fn uploads_home(h: &support::Harness) -> Uploads {
    use crabcloud_fs::Mount;
    Uploads::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: h.storage.clone(),
        }],
        h.sink.clone(),
        h.filecache.clone(),
    )
}

#[tokio::test]
async fn uploads_begin_put_commit_roundtrip() {
    let h = harness().await;
    let u = uploads_home(&h);

    let dest = UserPath::new("/big.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();
    assert!(!handle.upload_id.is_empty());

    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let t2 = u
        .put_part(&handle.upload_id, 2, body(b"BBB".to_vec()))
        .await
        .unwrap();

    let meta = u
        .commit(&handle.upload_id, &dest, vec![t1, t2])
        .await
        .unwrap();
    assert_eq!(meta.size, 6);

    // Read assembled bytes back through the storage directly.
    let mut reader = h
        .storage
        .read(&StoragePath::new("big.bin").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"AAABBB");
}

#[tokio::test]
async fn uploads_destination_mismatch_errors_on_commit() {
    let h = harness().await;
    let u = uploads_home(&h);
    let begin_dest = UserPath::new("/a.bin").unwrap();
    let handle = u.begin(&begin_dest).await.unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"x".to_vec()))
        .await
        .unwrap();
    // Commit to a DIFFERENT destination — should error.
    let wrong = UserPath::new("/b.bin").unwrap();
    let r = u.commit(&handle.upload_id, &wrong, vec![t1]).await;
    assert!(matches!(r, Err(FsError::Upload(_))));
}

#[tokio::test]
async fn uploads_abort_idempotent_on_unknown_id() {
    let h = harness().await;
    let u = uploads_home(&h);
    // Never call begin — just abort a fabricated id.
    u.abort("AA:BB:CC").await.unwrap();
    // And again.
    u.abort("AA:BB:CC").await.unwrap();
}

#[tokio::test]
async fn uploads_abort_then_commit_errors() {
    let h = harness().await;
    let u = uploads_home(&h);
    let dest = UserPath::new("/aborted.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"x".to_vec()))
        .await
        .unwrap();
    u.abort(&handle.upload_id).await.unwrap();

    // Commit on the same upload_id should now fail (the backend's
    // multipart state is gone).
    let r = u.commit(&handle.upload_id, &dest, vec![t1]).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn uploads_part_tag_round_trip_assembles_in_order() {
    let h = harness().await;
    let u = uploads_home(&h);
    let dest = UserPath::new("/ordered.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();

    // Submit parts out of natural order; tags carry their part_number.
    let t3 = u
        .put_part(&handle.upload_id, 3, body(b"CCC".to_vec()))
        .await
        .unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let t2 = u
        .put_part(&handle.upload_id, 2, body(b"BBB".to_vec()))
        .await
        .unwrap();

    // Pass tags in arbitrary order; storage layer sorts by part_number.
    let meta = u
        .commit(&handle.upload_id, &dest, vec![t3, t1, t2])
        .await
        .unwrap();
    assert_eq!(meta.size, 9);

    let mut reader = h
        .storage
        .read(&StoragePath::new("ordered.bin").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"AAABBBCCC");
}
```

Note on the line `use crabcloud_storage::{Mount as _, StoragePath};` — `crabcloud_storage` doesn't export a `Mount`. Fix that import by removing `Mount as _,`:

```rust
use crabcloud_storage::StoragePath;
```

Also remove the unused `Arc` and `UserId` imports if rustc flags them.

### Step 5: Run + commit + push + open Batch D PR

```
cargo test -p crabcloud-fs --tests
cargo xtask check-all
```

Expected: 5 new integration tests + 4 new unit tests in uploads.rs pass; prior tests still pass.

```
git add crates/crabcloud-fs
git commit -m "feat(fs): Uploads facade + upload_id encode/decode

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-d
gh pr create --base master --head fs-batch-d \
  --title "fs: batch D — Uploads facade + opaque self-describing upload_id" \
  --body "Sub-project 4c, batch D: Uploads::begin/put_part/abort/commit. Opaque \`upload_id\` encodes (path_prefix, dest_path, backend_id) as URL-safe base64 triplet — survives server restarts as long as backing-storage multipart state survives; no DB table needed. Destination-mismatch defense on commit. Abort idempotent on unknown id. 5 integration tests + 4 unit tests."
```

**STOP.**

---

## Task 5: AppState wiring + factory methods (Batch E)

**Files:**
- Modify: `Cargo.toml` (root) — already has `crabcloud-fs` workspace dep from Batch A; verify
- Modify: `crates/crabcloud-core/Cargo.toml` — add `crabcloud-fs` dep
- Modify: `crates/crabcloud-core/src/state.rs` — add `mount_resolver` field + `view_for`/`uploads_for` methods
- Modify: `crates/crabcloud-fs/src/lib.rs` — remove `crabcloud_config as _` + `tokio as _` + `tracing as _` anchors (real call sites land — verify)
- Create: `crates/crabcloud-fs/tests/appstate_wiring.rs`

### Step 1: Branch

```
git checkout -b fs-batch-e origin/master
```

### Step 2: Add `crabcloud-fs` to `crabcloud-core/Cargo.toml`

In `crates/crabcloud-core/Cargo.toml`, find the `[dependencies]` block and add `crabcloud-fs.workspace = true` alphabetically (between `crabcloud-filecache` and `crabcloud-http` or similar — match the existing pattern):

```toml
crabcloud-filecache.workspace = true
crabcloud-fs.workspace = true
```

### Step 3: Modify `crates/crabcloud-core/src/state.rs`

In the imports block, add:

```rust
use crabcloud_fs::{HomeMountResolver, LocalStorageFactory, MountResolver, Uploads, View};
```

Find the `AppState` struct definition (around line 19) and add a new field after `scanner`:

```rust
    /// Resolves per-user mounts. 4c default: `HomeMountResolver` over
    /// `LocalStorageFactory` (which uses `config.datadirectory`). Later
    /// sub-projects (sharing, external storage) layer additional resolvers.
    pub mount_resolver: Arc<dyn MountResolver>,
```

Find the `AppStateBuilder::build` body (around line 215) and INSERT after the `if self.config.filecache.enabled { ... }` block, BEFORE the `let state = AppState { ... };`:

```rust
        // Mount resolver: 4c ships home-only via LocalStorageFactory.
        let factory = Arc::new(LocalStorageFactory::new(self.config.datadirectory.clone()));
        let mount_resolver: Arc<dyn MountResolver> = Arc::new(HomeMountResolver::new(factory));
```

Modify the `let state = AppState { ... };` literal — add the new field:

```rust
        let state = AppState {
            config: self.config.clone(),
            pool,
            cache,
            i18n,
            appconfig,
            capability_providers: Arc::new(Mutex::new(Vec::new())),
            users,
            storage_sink,
            filecache,
            scanner,
            mount_resolver,
        };
```

Add the factory methods to the `impl AppState` block. Find the `pub async fn register_capability_provider(...)` and add adjacent:

```rust
    /// Construct a per-request `View` for `uid`. Resolves the user's
    /// mounts via `mount_resolver`.
    pub async fn view_for(&self, uid: &crabcloud_users::UserId) -> crabcloud_fs::FsResult<View> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(View::new(
            uid.clone(),
            mounts,
            self.filecache.clone(),
            self.storage_sink.clone(),
        ))
    }

    /// Construct a per-request `Uploads` façade for `uid`.
    pub async fn uploads_for(
        &self,
        uid: &crabcloud_users::UserId,
    ) -> crabcloud_fs::FsResult<Uploads> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(Uploads::new(
            uid.clone(),
            mounts,
            self.storage_sink.clone(),
            self.filecache.clone(),
        ))
    }
```

### Step 4: Clean up unused-crate anchors in `crabcloud-fs/src/lib.rs`

By this batch, `crabcloud_config` is no longer needed in `crabcloud-fs` (resolver/local.rs takes a `PathBuf` directly, not a `FileConfig`). Same for `tokio` (real call sites in view.rs/uploads.rs) and `tracing` (none yet; keep the anchor until a later batch needs it OR remove it now).

Update `crates/crabcloud-fs/src/lib.rs`:

```rust
//! `crabcloud-fs` — per-user filesystem façade.
//!
//! The [`View`] resolves user-facing paths (`/photos/cat.jpg`) to the
//! appropriate `(Storage, StoragePath)` tuple via the user's mounts, then
//! routes reads through [`FileCache`] and writes through the storage backend
//! (which emits events on the shared `ChannelEventSink`).
//!
//! The [`Uploads`] façade translates Nextcloud's chunked-upload HTTP protocol
//! (`/dav/uploads/<user>/<upload_id>/<n>` PUTs + MOVE-with-Destination) to
//! the Storage trait's multipart primitives.
//!
//! `MountResolver` + `StorageFactory` traits are forward-designed for share
//! and external storage mounts; sub-project 4c only ships `HomeMountResolver`
//! + `LocalStorageFactory`.

pub mod error;
pub mod mount;
pub mod path;
pub mod resolver;
pub mod uploads;
pub mod view;

pub use error::{FsError, FsResult};
pub use mount::{Mount, MountResolver, StorageFactory};
pub use path::UserPath;
pub use resolver::local::LocalStorageFactory;
pub use resolver::HomeMountResolver;
pub use uploads::Uploads;
pub use view::View;

// Anchor crates whose real call sites are intentionally test-only or
// reserved for follow-up. `tracing` will be picked up by future warn!/info!
// calls inside Uploads when error logging gets added.
#[cfg(test)]
use crabcloud_config as _;
use tracing as _;
```

(The `base64 as _` and `tokio as _` anchors are removed because they're now used directly. `crabcloud_config` is needed only for test_support; gate it `#[cfg(test)]`.)

### Step 5: Create `crates/crabcloud-fs/tests/appstate_wiring.rs`

```rust
//! Integration tests for `AppState::view_for` / `uploads_for`. Verifies the
//! resolver is wired correctly and that two calls for the same uid return
//! views over the same mount.

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use tempfile::tempdir;
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn appstate_view_for_round_trip_through_local_storage() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let view = state.view_for(&uid).await.unwrap();

    // Write a file via the View.
    let meta = view
        .put_file(&UserPath::new("/hello.txt").unwrap(), body(b"hi".to_vec()))
        .await
        .unwrap();
    assert_eq!(meta.size, 2);

    // Read it back via a fresh view_for (different request).
    let view2 = state.view_for(&uid).await.unwrap();
    let mut reader = view2
        .read(&UserPath::new("/hello.txt").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hi");
}

#[tokio::test]
async fn appstate_view_for_distinct_users_get_distinct_storages() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let alice = state
        .view_for(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let bob = state.view_for(&UserId::new("bob").unwrap()).await.unwrap();

    alice
        .put_file(&UserPath::new("/a.txt").unwrap(), body(b"alice".to_vec()))
        .await
        .unwrap();
    bob.put_file(&UserPath::new("/a.txt").unwrap(), body(b"bob".to_vec()))
        .await
        .unwrap();

    // Each user's /a.txt is independent.
    let mut reader = alice
        .read(&UserPath::new("/a.txt").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"alice");

    let mut reader = bob.read(&UserPath::new("/a.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"bob");
}

#[tokio::test]
async fn appstate_uploads_for_round_trip() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let uploads = state.uploads_for(&uid).await.unwrap();
    let dest = UserPath::new("/upload.bin").unwrap();

    let handle = uploads.begin(&dest).await.unwrap();
    let t1 = uploads
        .put_part(&handle.upload_id, 1, body(b"DATA".to_vec()))
        .await
        .unwrap();
    let meta = uploads
        .commit(&handle.upload_id, &dest, vec![t1])
        .await
        .unwrap();
    assert_eq!(meta.size, 4);
}
```

### Step 6: Run + commit + push + open Batch E PR

```
cargo test -p crabcloud-core
cargo test -p crabcloud-fs --tests
cargo xtask check-all
```

Expected: existing `build_assembles_state_from_minimal_config` still passes; 3 new appstate-wiring tests pass.

```
git add Cargo.toml crates/crabcloud-core crates/crabcloud-fs
git commit -m "feat(fs): AppState.view_for / uploads_for + HomeMountResolver wiring

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-e
gh pr create --base master --head fs-batch-e \
  --title "fs: batch E — AppState wiring + view_for/uploads_for" \
  --body "Sub-project 4c, batch E: AppState gains \`mount_resolver: Arc<dyn MountResolver>\` (\`HomeMountResolver\` over \`LocalStorageFactory\` using \`config.datadirectory\`). Factory methods \`view_for(uid)\` + \`uploads_for(uid)\` construct per-request façades from \`Vec<Mount>\`. **Spec deviation:** spec said add \`[storage] data_dir\` config block; \`FileConfig.datadirectory: PathBuf\` already exists on master with identical semantics, so the plan uses that existing field. 3 integration tests cover round-trip through AppState."
```

**STOP.**

---

## Task 6: Acceptance docs (Batch F)

**Files:**
- Create: `docs/superpowers/plans/2026-05-12-mount-view-and-uploads-implementation.changelog.md`
- Create: `docs/superpowers/specs/2026-05-12-mount-view-and-uploads-design.followup-sp5.md`
- Modify: `README.md`

### Step 1: Branch

```
git checkout -b fs-batch-f origin/master
```

### Step 2: Write the changelog

Create `docs/superpowers/plans/2026-05-12-mount-view-and-uploads-implementation.changelog.md`:

```markdown
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
```

### Step 3: Write the sub-project 5 (WebDAV) prep notes

Create `docs/superpowers/specs/2026-05-12-mount-view-and-uploads-design.followup-sp5.md`:

```markdown
# Sub-project 5 prep — WebDAV (and Files API)

Notes captured during 4c implementation that should inform the sub-project 5 spec when we brainstorm it. **These are prep notes, not a spec** — the actual SP5 spec will be authored via the brainstorming skill before implementation begins.

## Scope sketch

Sub-project 5 adds HTTP routes that:

1. Implement WebDAV at `/remote.php/dav/files/<user>/<path>` (`PROPFIND`/`GET`/`PUT`/`MKCOL`/`DELETE`/`MOVE`/`COPY`).
2. Implement Nextcloud's chunked-upload protocol at `/remote.php/dav/uploads/<user>/<upload_id>/...`.
3. Re-export the same paths under `/dav/` (Nextcloud's modern alias).

All HTTP handlers call into `AppState::view_for(uid)` and `AppState::uploads_for(uid)`. The View+Uploads façades from 4c are the only state-mutation surface; WebDAV is a thin protocol layer.

## Trait-shape implications confirmed in 4c

- `View::stat` returns `FileMetadata` which is the right shape for `PROPFIND`'s response.
- `View::list` returns `Vec<DirEntry>` — matches PROPFIND's `Depth: 1` children.
- `View::read_range` matches HTTP `Range` requests (use `Range<u64>` from the `bytes=` header).
- `Uploads::commit` accepts a `Vec<PartTag>` — WebDAV must communicate part tags client-side. Two options:
  - (a) Server returns each `PartTag.etag` in the `PUT /uploads/<id>/<n>` response's `ETag` header; client sends them back in the MOVE request's `X-Crabcloud-Part-Tags` header (JSON-encoded).
  - (b) Server stores `PartTag`s in a per-upload state file in the storage's tempdir; commit re-reads them. Simpler protocol, more storage I/O.

Recommend (a) — keeps the storage layer stateless beyond the multipart primitives themselves.

## Operation mapping (WebDAV → View/Uploads)

| WebDAV request | Crabcloud handler call |
|---|---|
| `GET /dav/files/<user>/<path>` | `View::read(user_path)` |
| `GET /dav/files/<user>/<path>` with `Range:` header | `View::read_range(user_path, range)` |
| `PUT /dav/files/<user>/<path>` body | `View::put_file(user_path, body)` |
| `MKCOL /dav/files/<user>/<path>` | `View::mkdir(user_path)` |
| `DELETE /dav/files/<user>/<path>` | `View::delete(user_path)` |
| `MOVE /dav/files/<user>/<from>` with `Destination: /dav/files/<user>/<to>` | `View::rename(from, to)` |
| `COPY /dav/files/<user>/<from>` with `Destination:` header | `View::copy(from, to)` |
| `PROPFIND /dav/files/<user>/<path>` (Depth: 0) | `View::stat(user_path)` |
| `PROPFIND /dav/files/<user>/<path>` (Depth: 1) | `View::stat` + `View::list` |
| `MKCOL /dav/uploads/<user>/<id>` (after client computes random id) | `Uploads::begin(destination via Destination: header)` |
| `PUT /dav/uploads/<user>/<id>/<n>` | `Uploads::put_part(id, n, body)` |
| `MOVE /dav/uploads/<user>/<id>/.file` with `Destination:` | `Uploads::commit(id, destination, parts)` |
| `DELETE /dav/uploads/<user>/<id>` | `Uploads::abort(id)` |

## Open questions for sub-project 5 brainstorming

- **PROPFIND XML schema:** match Nextcloud's exact prop set (`{DAV:}getcontentlength`, `{DAV:}getetag`, `{DAV:}getlastmodified`, `{DAV:}getcontenttype`, `{DAV:}resourcetype`, `{http://owncloud.org/ns}id`, `{http://owncloud.org/ns}permissions`, etc.). XML library choice: `quick-xml`? Hand-rolled?
- **`Destination` header parsing:** absolute URL vs. path-only. Nextcloud accepts both.
- **Part tag transport:** as suggested above, ETag-in-response + custom-header-on-commit?
- **Auth on WebDAV routes:** `AuthLayer` (Bearer/Basic/Cookie) from 2b already works; just attach to the route tree.
- **`Depth` header validation:** Nextcloud limits PROPFIND to Depth ≤ 1 by default. Same here?
- **Range request semantics:** support multiple ranges? Nextcloud doesn't.
- **`If-Match` / `If-None-Match`:** for conditional PUT (concurrent-write safety). Needs View to expose etag verification.

## Estimated scope

5–7 batches: WebDAV routes (PROPFIND/GET/PUT/MKCOL/DELETE/MOVE/COPY) + chunked-upload routes + auth wiring + Playwright e2e using a real Nextcloud desktop client SDK (or the same Playwright HTTP patterns from earlier tests) + acceptance docs.
```

### Step 4: Update README.md

Read `README.md` and find the workspace-layout block where `crabcloud-fs` should appear in alphabetical order. Insert between `-filecache` and `-http`:

```
- `crates/crabcloud-fs` — per-user filesystem facade (View + Uploads + mount resolution).
```

Match the bullet style of the existing entries.

### Step 5: Run + commit + push + open Batch F PR

```
cargo xtask check-all
git add docs/superpowers README.md
git commit -m "docs(fs): sub-project 4c acceptance — changelog + README + sub-project 5 prep notes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin fs-batch-f
gh pr create --base master --head fs-batch-f \
  --title "fs: batch F — sub-project 4c acceptance docs" \
  --body "Sub-project 4c final batch: changelog with 16-row acceptance table (notes the \`datadirectory\` deviation from spec's proposed \`[storage] data_dir\`), README workspace-layout bullet for crabcloud-fs, and prep notes for the eventual sub-project 5 (WebDAV) brainstorming session (operation mapping, part-tag transport, PROPFIND XML, open questions)."
```

**STOP.**

---

## Final acceptance

After all 6 PRs land:

1. `git pull --ff-only origin master`.
2. `cargo xtask check-all` green.
3. Update program memory: mark 4c complete, point to sub-project 5 prep notes.
4. Brainstorm sub-project 5 (WebDAV + Files API) when ready.

## Open questions deferred

- See changelog "What's deferred" and "Known limitations".
- See sub-project 5 prep doc for WebDAV-design decisions.
- See spec §15 (Open questions) for mount caching + commit verification + read-through populate considerations.
