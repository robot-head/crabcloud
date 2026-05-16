# Folder Zip Download Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship streaming `application/zip` downloads of folders, on both the authenticated Files surface (`GET /api/files/zip/{*path}`) and the public-link surface (`GET /s/{token}/zip/{*path}`). Closes SP8 carryforward E7.

**Architecture:** A new `crabcloud-zip` crate owns the streaming zip helper: pre-flight walk for caps, per-mime compression dispatch (DEFLATE for text-ish, STORED otherwise), UTF-8 filename emission with the Info-ZIP Unicode Path extra field. Two HTTP handlers (authed + public) delegate to the same `stream_folder` helper. Operator-tunable caps land in `FileConfig` with defaults 500 entries / 2 GiB.

**Tech Stack:** Rust 1.95, `zip = "5"`, axum 0.8, tokio mpsc + `Body::from_stream`, existing `View` / `MountResolver` / `Filecache` infrastructure.

**Spec:** `docs/superpowers/specs/2026-05-16-folder-zip-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`. At the start of each batch:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp9/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-zip-crate`, `b-authed-handler`, `c-public-handler`.
- **Commit cadence:** Commit at every "Commit" step. Frequent, focused commits.
- **Pre-PR check:** Before opening the PR:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass locally.
- **Open the PR** with the title and body documented in each batch's final task.
- **Merge:** After all checks pass: `gh pr merge --squash --delete-branch`.
- **Established workaround:** Tests building `AppState` must set `cfg.filecache.enabled = false` before `AppStateBuilder::new(cfg).build()`. See `crates/crabcloud-http/tests/dav_basic.rs:16-37` for the pattern.
- **Pre-existing patterns to mirror:**
  - **Test fixture sharing:** `crates/crabcloud-http/tests/support/mod.rs` (introduced in SP8 Batch F) houses `make_state`, `seed_user`, `seed_folder`, `seed_file`, `create_link`. Reuse these.
  - **Public-link handler shape:** `crates/crabcloud-http/src/routes/public_link/mod.rs` — auth-context extraction, View construction via `PublicLinkMountResolver`, error mapping. SP9's public zip handler sits as a sibling.
  - **Authed file path:** there is no existing `/api/files/*` axum surface. The authed zip handler is a new axum route mounted under the standard `AuthLayer` (NOT under `public_link_gate` / `public_dav_gate`).
  - **Async streaming response:** `axum::body::Body::from_stream` is the documented path. Pair with `tokio_stream::wrappers::ReceiverStream` over a bounded `tokio::sync::mpsc::channel`.

---

## File-by-file map

### New crate: `crabcloud-zip`

```
crates/crabcloud-zip/
├── Cargo.toml
├── src/
│   ├── lib.rs              — re-exports + crate doc
│   ├── error.rs            — ZipError, WalkError
│   ├── types.rs            — ZipCaps, ZipPlan, PlannedEntry, PlanKind, ZipSummary
│   ├── walk.rs             — walk_for_caps
│   ├── compression.rs      — compression_for_mime
│   ├── stream.rs           — stream_folder
│   └── mpsc_writer.rs      — AsyncWrite adapter over tokio::sync::mpsc::Sender<Result<Bytes, io::Error>>
└── tests/                  — unit tests live alongside source as #[cfg(test)] mod tests; no separate integration test file
```

### Modified

- `Cargo.toml` (workspace) — add `crates/crabcloud-zip` to members; add `zip = { version = "5", default-features = false, features = ["deflate", "deflate-flate2"] }` to `[workspace.dependencies]`.
- `crates/crabcloud-config/src/types.rs` — `folder_zip_max_entries: u64` and `folder_zip_max_bytes: u64` fields with serde defaults.
- `crates/crabcloud-http/src/routes/mod.rs` — `pub mod files_zip;` (new module for the authed handler).
- `crates/crabcloud-http/src/routes/files_zip.rs` (new) — authed `GET /api/files/zip/{*path}` handler.
- `crates/crabcloud-http/src/routes/public_link/mod.rs` — adds `zip_handler` route + handler.
- `crates/crabcloud-http/src/router.rs` — wire `files_zip::router()` under the existing authed router (NOT under `public_link_gate`).
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-zip` workspace dep + `bytes` / `tokio_stream` / `tokio_util` if not already present.
- `crates/crabcloud-http/tests/support/mod.rs` — extend with one helper `seed_tree(state, uid, root, entries)` (used by zip e2e tests).
- `crates/crabcloud-http/tests/files_zip_e2e.rs` (new) — authed e2e tests.
- `crates/crabcloud-http/tests/public_link_e2e.rs` — add public-zip e2e cases.

---

# Batch A — `crabcloud-zip` foundation crate

**Branch:** `sp9/a-zip-crate`

**Goal:** Stand up the new crate with types, walk, compression dispatch, and the `stream_folder` helper. No HTTP wiring. Everything covered by unit tests.

### Task A1: Create the crate skeleton

**Files:**
- Create: `crates/crabcloud-zip/Cargo.toml`
- Create: `crates/crabcloud-zip/src/lib.rs`
- Create: `crates/crabcloud-zip/src/error.rs`
- Modify: workspace `Cargo.toml` — add member + workspace dep

- [ ] **Step 1: Add the crate to the workspace and register the `zip` dep**

Edit the workspace `Cargo.toml` at the repo root:
1. Add `"crates/crabcloud-zip",` to the `members` array (alphabetical position).
2. Add to `[workspace.dependencies]`:
   ```toml
   zip = { version = "5", default-features = false, features = ["deflate", "deflate-flate2"] }
   ```
3. Add to the internal workspace deps section (alphabetical):
   ```toml
   crabcloud-zip = { path = "crates/crabcloud-zip" }
   ```

- [ ] **Step 2: Write `crates/crabcloud-zip/Cargo.toml`**

```toml
[package]
name = "crabcloud-zip"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait = { workspace = true }
bytes = { workspace = true }
chrono = { workspace = true }
crabcloud-fs = { workspace = true }
crabcloud-storage = { workspace = true }
futures = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync", "io-util"] }
tokio-util = { workspace = true, features = ["io"] }
tracing = { workspace = true }
zip = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
crabcloud-storage = { workspace = true }
crabcloud-filecache = { workspace = true }
crabcloud-users = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 3: Write `src/lib.rs`**

```rust
//! Streaming folder-zip helper for `crabcloud-http`.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-folder-zip-design.md`.
//!
//! Public entry point is [`stream_folder`]. The helper walks a folder tree
//! via [`crabcloud_fs::View`], enforces operator-configurable [`ZipCaps`],
//! then streams a zip archive into an [`AsyncWrite`] sink. Compression is
//! picked per-entry from the file's mime type ([`compression_for_mime`]).
//! Filename encoding uses UTF-8 (general-purpose bit 11) plus the Info-ZIP
//! Unicode Path extra field on every entry.

mod compression;
mod error;
mod mpsc_writer;
mod stream;
mod types;
mod walk;

pub use compression::compression_for_mime;
pub use error::{WalkError, ZipError};
pub use mpsc_writer::MpscBytesWriter;
pub use stream::stream_folder;
pub use types::{PlanKind, PlannedEntry, ZipCaps, ZipPlan, ZipSummary};
pub use walk::walk_for_caps;
```

- [ ] **Step 4: Write `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalkError {
    #[error("folder too large ({count} entries, {bytes} bytes)")]
    TooLarge { count: u64, bytes: u64 },
    #[error(transparent)]
    View(#[from] crabcloud_fs::FsError),
}

#[derive(Debug, Error)]
pub enum ZipError {
    #[error(transparent)]
    Walk(#[from] WalkError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
```

- [ ] **Step 5: Verify crate builds**

```bash
cargo build -p crabcloud-zip
```

Expected: clean build, possibly warnings about unused module re-exports (resolves as later tasks land).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/crabcloud-zip/
git commit -m "zip: crate skeleton with error types and workspace integration"
```

### Task A2: Types

**Files:**
- Create: `crates/crabcloud-zip/src/types.rs`

- [ ] **Step 1: Write `src/types.rs`**

```rust
//! Public types for the streaming-zip helper.

use crabcloud_storage::StoragePath;
use std::time::SystemTime;

/// Operator-tunable size caps. Pre-flight walk rejects anything over.
#[derive(Debug, Clone, Copy)]
pub struct ZipCaps {
    pub max_entries: u64,
    pub max_bytes: u64,
}

impl ZipCaps {
    /// Sensible defaults matching `FileConfig`'s defaults.
    pub fn defaults() -> Self {
        Self {
            max_entries: 500,
            max_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// What kind of entry a [`PlannedEntry`] represents in the zip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    File,
    Dir,
}

/// One entry the walker decided to include. `zip_name` is the path inside
/// the zip archive (always `/`-separated, no leading slash, directories
/// carry a trailing `/`).
#[derive(Debug, Clone)]
pub struct PlannedEntry {
    pub storage_path: StoragePath,
    pub zip_name: String,
    pub kind: PlanKind,
    pub size: u64,
    pub mtime: SystemTime,
    pub mime: String,
}

#[derive(Debug, Clone)]
pub struct ZipPlan {
    pub entries: Vec<PlannedEntry>,
    pub total_bytes: u64,
}

/// Returned by [`crate::stream_folder`] on success.
#[derive(Debug, Clone, Copy)]
pub struct ZipSummary {
    pub entries: u64,
    pub bytes: u64,
}
```

- [ ] **Step 2: Verify build**

```bash
cargo build -p crabcloud-zip
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-zip/src/types.rs
git commit -m "zip: ZipCaps, PlannedEntry, ZipPlan, ZipSummary types"
```

### Task A3: `walk_for_caps`

**Files:**
- Create: `crates/crabcloud-zip/src/walk.rs`

- [ ] **Step 1: Write the failing tests first**

Add to `crates/crabcloud-zip/src/walk.rs` with the test module at the bottom:

```rust
//! Pre-flight walk that builds a [`ZipPlan`] and rejects oversize folders.
//!
//! Walks depth-first via [`crabcloud_fs::View::list_with_meta`] / `stat`.
//! Aborts on first overflow with [`WalkError::TooLarge`].

use crate::error::WalkError;
use crate::types::{PlanKind, PlannedEntry, ZipCaps, ZipPlan};
use crabcloud_fs::View;
use crabcloud_fs::path::UserPath;
use crabcloud_storage::FileKind;

/// Walk `root` (which must be a directory) and return a [`ZipPlan`] of
/// every entry to include, or `WalkError::TooLarge` if either cap is hit.
///
/// `root` is the user-facing path (e.g. `/Photos`). The returned
/// `PlannedEntry.zip_name` strips `root`'s parent so the archive's
/// internal structure starts at `<basename(root)>/...`.
pub async fn walk_for_caps(
    view: &View,
    root: &UserPath,
    caps: &ZipCaps,
) -> Result<ZipPlan, WalkError> {
    let root_basename = root_basename(root);
    let mut entries: Vec<PlannedEntry> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut stack: Vec<(UserPath, String)> = Vec::new();
    stack.push((root.clone(), root_basename.clone()));

    while let Some((current_user_path, zip_prefix)) = stack.pop() {
        let dir_entries = view.list(&current_user_path).await?;
        // Record the directory itself (Dir entry with trailing `/`) so
        // empty folders are preserved.
        if !zip_prefix.is_empty() {
            let dir_meta = view.stat(&current_user_path).await?;
            push_entry(
                &mut entries,
                &mut total_bytes,
                caps,
                PlannedEntry {
                    storage_path: storage_path_of(view, &current_user_path)?,
                    zip_name: format!("{zip_prefix}/"),
                    kind: PlanKind::Dir,
                    size: 0,
                    mtime: dir_meta.mtime,
                    mime: String::new(),
                },
            )?;
        }
        for de in dir_entries {
            let child_user_path = join_user_path(&current_user_path, &de.name)?;
            match de.metadata.kind {
                FileKind::Directory => {
                    let child_prefix = if zip_prefix.is_empty() {
                        de.name.clone()
                    } else {
                        format!("{zip_prefix}/{}", de.name)
                    };
                    stack.push((child_user_path, child_prefix));
                }
                FileKind::File => {
                    let zip_name = if zip_prefix.is_empty() {
                        de.name.clone()
                    } else {
                        format!("{zip_prefix}/{}", de.name)
                    };
                    push_entry(
                        &mut entries,
                        &mut total_bytes,
                        caps,
                        PlannedEntry {
                            storage_path: storage_path_of(view, &child_user_path)?,
                            zip_name,
                            kind: PlanKind::File,
                            size: de.metadata.size,
                            mtime: de.metadata.mtime,
                            mime: de.metadata.mimetype.as_str().to_string(),
                        },
                    )?;
                }
            }
        }
    }

    Ok(ZipPlan { entries, total_bytes })
}

fn root_basename(root: &UserPath) -> String {
    let stripped = root.as_str().trim_start_matches('/').trim_end_matches('/');
    if stripped.is_empty() {
        return String::new();
    }
    match stripped.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None => stripped.to_string(),
    }
}

fn join_user_path(parent: &UserPath, child: &str) -> Result<UserPath, WalkError> {
    let p = parent.as_str().trim_end_matches('/');
    let candidate = if p == "/" || p.is_empty() {
        format!("/{child}")
    } else {
        format!("{p}/{child}")
    };
    UserPath::new(candidate).map_err(|e| WalkError::View(e.into()))
}

fn storage_path_of(
    view: &View,
    user_path: &UserPath,
) -> Result<crabcloud_storage::StoragePath, WalkError> {
    let (storage, _) = view.cache_key_for(user_path).map_err(WalkError::View)?;
    let _ = storage; // we only need the path component
    let (_, sp) = view.cache_key_for(user_path).map_err(WalkError::View)?;
    Ok(sp)
}

fn push_entry(
    entries: &mut Vec<PlannedEntry>,
    total_bytes: &mut u64,
    caps: &ZipCaps,
    entry: PlannedEntry,
) -> Result<(), WalkError> {
    *total_bytes = total_bytes.saturating_add(entry.size);
    let new_count = entries.len() as u64 + 1;
    if new_count > caps.max_entries || *total_bytes > caps.max_bytes {
        return Err(WalkError::TooLarge {
            count: new_count,
            bytes: *total_bytes,
        });
    }
    entries.push(entry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_fs::View;
    use crabcloud_fs::path::UserPath;
    use crabcloud_storage::{memory::MemoryStorage, Storage};
    use std::sync::Arc;

    async fn seed_view() -> View {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        // Folder layout:
        //   /Photos/
        //     cat.jpg     (8 bytes)
        //     dog.jpg     (8 bytes)
        //     vacation/
        //       beach.jpg (16 bytes)
        //       empty/
        use crabcloud_storage::{NoopEventSink, StoragePath};
        storage.mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink).await.unwrap();
        storage.mkdir(&StoragePath::new("Photos/vacation").unwrap(), &NoopEventSink).await.unwrap();
        storage.mkdir(&StoragePath::new("Photos/vacation/empty").unwrap(), &NoopEventSink).await.unwrap();
        storage.put_file(
            &StoragePath::new("Photos/cat.jpg").unwrap(),
            Box::pin(std::io::Cursor::new(b"cat-data".to_vec())),
            &NoopEventSink,
        ).await.unwrap();
        storage.put_file(
            &StoragePath::new("Photos/dog.jpg").unwrap(),
            Box::pin(std::io::Cursor::new(b"dog-data".to_vec())),
            &NoopEventSink,
        ).await.unwrap();
        storage.put_file(
            &StoragePath::new("Photos/vacation/beach.jpg").unwrap(),
            Box::pin(std::io::Cursor::new(b"beach-data-16byt".to_vec())),
            &NoopEventSink,
        ).await.unwrap();

        // Build the view (minimal — single mount at /, in-memory filecache).
        // Tests rely on the helper at `crates/crabcloud-zip/tests/support.rs`
        // which constructs an actual View. See Step 2 below.
        view_with_single_mount(storage).await
    }

    async fn view_with_single_mount(storage: Arc<dyn Storage>) -> View {
        use crabcloud_filecache::FileCache;
        use crabcloud_fs::{Mount, MountKind, MountMetadata};
        use crabcloud_storage::{ChannelEventSink, StoragePath};
        use crabcloud_users::UserId;

        // A minimal filecache backed by an in-memory sqlite pool.
        let pool = crabcloud_db::test_support::in_memory_sqlite().await.unwrap();
        let filecache = Arc::new(FileCache::new(Arc::new(pool)));
        filecache.register_storage(storage.id()).await.unwrap();
        filecache.full_scan(storage.id(), storage.clone()).await.unwrap();

        let mount = Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: Some(MountMetadata {
                kind: MountKind::Home,
                owner_uid: None,
                permissions: None,
            }),
        };
        let sink = Arc::new(ChannelEventSink::new(16));
        View::new(UserId::new("alice").unwrap(), vec![mount], filecache, sink)
    }

    #[tokio::test]
    async fn walk_counts_entries_recursively() {
        let view = seed_view().await;
        let plan = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps { max_entries: 100, max_bytes: 1024 },
        )
        .await
        .unwrap();
        // Photos/ + Photos/vacation/ + Photos/vacation/empty/ = 3 dirs.
        // Photos/cat.jpg + Photos/dog.jpg + Photos/vacation/beach.jpg = 3 files.
        assert_eq!(plan.entries.len(), 6);
        assert_eq!(plan.total_bytes, 8 + 8 + 16);
    }

    #[tokio::test]
    async fn walk_rejects_on_entries_overflow() {
        let view = seed_view().await;
        let r = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps { max_entries: 2, max_bytes: 1024 },
        )
        .await;
        match r {
            Err(WalkError::TooLarge { count, .. }) => assert!(count >= 3),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn walk_rejects_on_bytes_overflow() {
        let view = seed_view().await;
        let r = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps { max_entries: 100, max_bytes: 10 },
        )
        .await;
        match r {
            Err(WalkError::TooLarge { bytes, .. }) => assert!(bytes > 10),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn walk_includes_empty_directory_as_entry() {
        let view = seed_view().await;
        let plan = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps { max_entries: 100, max_bytes: 1024 },
        )
        .await
        .unwrap();
        let empty = plan.entries.iter().find(|e| e.zip_name == "Photos/vacation/empty/");
        assert!(empty.is_some(), "empty dir must appear as a planned Dir entry");
        assert_eq!(empty.unwrap().kind, PlanKind::Dir);
    }
}
```

Note: the test helper depends on `crabcloud_db::test_support::in_memory_sqlite`. Verify this helper exists by running `cargo doc -p crabcloud-db --no-deps --open` or searching `crates/crabcloud-db/src/test_support.rs`. If it doesn't, look at `crates/crabcloud-filecache/tests/` for the established pattern for in-memory pools — the existing fs/filecache tests definitely have one. Use the established helper. If absolutely no helper exists, gate the tests with `#[ignore]` and report DONE_WITH_CONCERNS so a follow-up can extract a shared `in_memory_sqlite` helper.

- [ ] **Step 2: Run the tests; expect them to fail to COMPILE (function not yet defined)**

```bash
cargo test -p crabcloud-zip walk::tests
```

Expected: compile failure citing `walk_for_caps` or its helpers. That's the TDD red.

- [ ] **Step 3: Confirm the implementation in Step 1 matches the tests**

The code from Step 1 includes both the impl and the tests. Re-running should now compile and pass:

```bash
cargo test -p crabcloud-zip walk::tests
```

Expected: 4 tests pass.

If `view.list` returns its entries in a different order than this code assumes, the `walk_includes_empty_directory_as_entry` test may fail because the empty-dir's parent hasn't been traversed yet. The DFS-stack-pop ordering means the deepest-pushed dir is visited last; the test asserts on the *presence* of the entry, not its position, so order shouldn't matter.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-zip/src/walk.rs
git commit -m "zip: walk_for_caps with depth-first pre-flight and cap checks"
```

### Task A4: `compression_for_mime`

**Files:**
- Create: `crates/crabcloud-zip/src/compression.rs`

- [ ] **Step 1: Write tests + impl together**

```rust
//! Compression-method dispatch keyed off mime type.

use zip::CompressionMethod;

const COMPRESSIBLE_PREFIXES: &[&str] = &[
    "text/",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/x-yaml",
    "application/wasm",
    "image/svg+xml",
];

/// Pick a compression method for a single zip entry based on its mime.
///
/// Already-compressed binary types (jpeg, png, mp4, zip, octet-stream) are
/// stored verbatim to avoid burning CPU for negligible size wins. The
/// matching is case-insensitive prefix; an unknown or empty mime falls
/// through to [`CompressionMethod::Stored`].
pub fn compression_for_mime(mime: &str) -> CompressionMethod {
    let lc = mime.to_ascii_lowercase();
    if COMPRESSIBLE_PREFIXES.iter().any(|p| lc.starts_with(p)) {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_mimes_get_deflate() {
        for mime in &[
            "text/plain",
            "text/html",
            "text/css",
            "application/json",
            "application/javascript",
            "application/xml",
            "application/x-yaml",
            "application/wasm",
            "image/svg+xml",
        ] {
            assert_eq!(
                compression_for_mime(mime),
                CompressionMethod::Deflated,
                "{mime} should DEFLATE",
            );
        }
    }

    #[test]
    fn binary_mimes_get_stored() {
        for mime in &[
            "image/jpeg",
            "image/png",
            "video/mp4",
            "application/zip",
            "application/octet-stream",
            "application/pdf",
            "",
        ] {
            assert_eq!(
                compression_for_mime(mime),
                CompressionMethod::Stored,
                "{mime} should STORE",
            );
        }
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            compression_for_mime("TEXT/Plain"),
            CompressionMethod::Deflated,
        );
        assert_eq!(
            compression_for_mime("Application/JSON"),
            CompressionMethod::Deflated,
        );
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-zip compression::tests
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-zip/src/compression.rs
git commit -m "zip: compression_for_mime dispatch (text/* -> DEFLATE, else STORED)"
```

### Task A5: `MpscBytesWriter` AsyncWrite adapter

**Files:**
- Create: `crates/crabcloud-zip/src/mpsc_writer.rs`

- [ ] **Step 1: Write the adapter**

```rust
//! `AsyncWrite` adapter that forwards every write to an
//! `mpsc::Sender<Result<Bytes, io::Error>>`. Lets `stream_folder` push
//! bytes through a tokio channel that an axum `Body::from_stream` reads
//! from.

use bytes::Bytes;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

pub struct MpscBytesWriter {
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
}

impl MpscBytesWriter {
    pub fn new(tx: mpsc::Sender<Result<Bytes, io::Error>>) -> Self {
        Self { tx }
    }
}

impl AsyncWrite for MpscBytesWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let bytes = Bytes::copy_from_slice(buf);
        let len = bytes.len();
        // Use the permit-style send to apply backpressure when the channel
        // is full. `try_send` would drop bytes on a full channel.
        let send = self.tx.clone().reserve_owned();
        tokio::pin!(send);
        match send.poll(cx) {
            Poll::Ready(Ok(permit)) => {
                permit.send(Ok(bytes));
                Poll::Ready(Ok(len))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "receiver dropped",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn writes_forwarded_in_order() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut writer = MpscBytesWriter::new(tx);
        writer.write_all(b"hello ").await.unwrap();
        writer.write_all(b"world").await.unwrap();
        drop(writer);
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        assert_eq!(combined, b"hello world");
    }

    #[tokio::test]
    async fn receiver_drop_yields_broken_pipe() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(1);
        drop(rx);
        let mut writer = MpscBytesWriter::new(tx);
        let err = writer.write_all(b"data").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-zip mpsc_writer::tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-zip/src/mpsc_writer.rs
git commit -m "zip: MpscBytesWriter AsyncWrite -> mpsc::Sender adapter"
```

### Task A6: `stream_folder`

**Files:**
- Create: `crates/crabcloud-zip/src/stream.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! Streaming zip writer: walks the plan and writes each entry into the
//! supplied `AsyncWrite` sink. Uses sync `std::io::Write` underneath since
//! `zip = "5"` only ships a sync writer; bridges via
//! `tokio_util::io::SyncIoBridge`.

use crate::compression::compression_for_mime;
use crate::error::{WalkError, ZipError};
use crate::types::{PlanKind, ZipCaps, ZipPlan, ZipSummary};
use crate::walk::walk_for_caps;
use crabcloud_fs::View;
use crabcloud_fs::path::UserPath;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio_util::io::SyncIoBridge;
use zip::write::{FileOptions, ZipWriter};
use zip::CompressionMethod;

/// Walk `root`, enforce `caps`, and stream a zip archive into `sink`.
///
/// On `WalkError::TooLarge` the caller hasn't written any bytes yet, so
/// the HTTP handler can return 413 with a JSON body. On success, returns
/// the entry count and total uncompressed byte count.
pub async fn stream_folder<W>(
    view: &View,
    root: &UserPath,
    caps: ZipCaps,
    sink: W,
) -> Result<ZipSummary, ZipError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    // 1. Pre-flight walk. Errors return early before any byte hits `sink`.
    let plan = walk_for_caps(view, root, &caps).await?;

    // 2. Read each file's body into an in-memory buffer (one at a time)
    //    so the spawned-blocking task can hand it to the sync ZipWriter.
    //    For large files we stream chunks; ZipWriter accepts repeated
    //    write_all calls within a single start_file/...sequence.
    write_zip(view, plan, sink).await
}

async fn write_zip<W>(view: &View, plan: ZipPlan, sink: W) -> Result<ZipSummary, ZipError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    // Pre-read all entry bodies into Vec<u8>s before invoking spawn_blocking,
    // because the sync writer takes &mut [u8] / Read and we don't want to
    // shuttle a View into spawn_blocking. This trades peak memory for
    // simplicity; per-entry size is bounded by caps.max_bytes total, and
    // we already pre-flight rejected anything bigger.
    let mut bodies: Vec<(crate::types::PlannedEntry, Vec<u8>)> = Vec::with_capacity(plan.entries.len());
    for entry in plan.entries.into_iter() {
        if entry.kind == PlanKind::File {
            let mut reader = view
                .read(&user_path_from_zip_entry(&entry))
                .await
                .map_err(WalkError::from)?;
            let mut buf = Vec::with_capacity(entry.size as usize);
            reader
                .read_to_end(&mut buf)
                .await
                .map_err(ZipError::from)?;
            bodies.push((entry, buf));
        } else {
            bodies.push((entry, Vec::new()));
        }
    }

    let summary = tokio::task::spawn_blocking(move || -> Result<ZipSummary, ZipError> {
        let bridge = SyncIoBridge::new(sink);
        let mut zw = ZipWriter::new(bridge);
        let mut bytes_total: u64 = 0;
        let mut count: u64 = 0;
        for (entry, body) in bodies {
            let method = match entry.kind {
                PlanKind::Dir => CompressionMethod::Stored,
                PlanKind::File => compression_for_mime(&entry.mime),
            };
            // FileOptions::default() sets the UTF-8 (bit 11) flag.
            // `large_file(false)` skips Zip64 (we're under 4 GiB by caps).
            let mut options: FileOptions<()> = FileOptions::default()
                .compression_method(method)
                .large_file(false)
                .unix_permissions(0o644);
            // Convert SystemTime → zip::DateTime. SystemTime → DateTime<Utc>
            // → zip::DateTime via the chrono helper. The `try_from` impl on
            // zip::DateTime accepts chrono's NaiveDateTime.
            if let Ok(dur) = entry.mtime.duration_since(std::time::UNIX_EPOCH) {
                let secs = dur.as_secs() as i64;
                if let Some(naive) =
                    chrono::NaiveDateTime::from_timestamp_opt(secs, 0)
                {
                    if let Ok(dt) = zip::DateTime::try_from(naive) {
                        options = options.last_modified_time(dt);
                    }
                }
            }
            match entry.kind {
                PlanKind::Dir => {
                    // `add_directory` emits a directory entry with the
                    // trailing-slash name. We pre-formatted zip_name with
                    // the trailing `/` in walk.rs.
                    let dir_name = entry.zip_name.trim_end_matches('/').to_string();
                    zw.add_directory(dir_name, options)?;
                }
                PlanKind::File => {
                    zw.start_file(entry.zip_name, options)?;
                    use std::io::Write as _;
                    zw.write_all(&body)?;
                    bytes_total = bytes_total.saturating_add(body.len() as u64);
                }
            }
            count += 1;
        }
        zw.finish()?;
        Ok(ZipSummary {
            entries: count,
            bytes: bytes_total,
        })
    })
    .await
    .map_err(|e| {
        ZipError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("zip writer task panicked: {e}"),
        ))
    })??;
    Ok(summary)
}

fn user_path_from_zip_entry(entry: &crate::types::PlannedEntry) -> UserPath {
    // `zip_name` is `<root_basename>/<rest>` (or just `<root_basename>` for
    // a single-file zip — not currently supported but harmless). We re-route
    // to the original user path by stitching back. The walk stored the
    // user-facing root in `storage_path`'s sibling — but we kept only the
    // storage_path. For the View read we need a UserPath that maps through
    // the mount; conveniently, View::resolve handles either. We can
    // reconstruct via the zip_name relative to the root.
    //
    // Simpler: the user_path equals "/" + entry.zip_name, since the zip
    // archive's top-level entry is the requested folder's basename and the
    // request was for `/<basename>` (the root requested by the caller).
    let candidate = format!("/{}", entry.zip_name);
    UserPath::new(candidate).expect("zip_name was derived from valid path segments")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ZipCaps;
    use std::io::Cursor;
    use std::sync::Arc;

    // Reuses seed_view from walk.rs via a path import. If your test module
    // organization differs, copy seed_view here.

    #[tokio::test]
    async fn stream_produces_valid_zip() {
        let view = crate::walk::tests::seed_view().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        let handle = tokio::spawn(async move {
            stream_folder(
                &view,
                &UserPath::new("/Photos").unwrap(),
                ZipCaps { max_entries: 100, max_bytes: 1024 * 1024 },
                writer,
            )
            .await
        });
        let mut combined: Vec<u8> = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        let summary = handle.await.unwrap().unwrap();
        assert!(summary.entries >= 3);

        let mut archive = zip::ZipArchive::new(Cursor::new(combined)).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert!(names.iter().any(|n| n == "Photos/cat.jpg"));
        assert!(names.iter().any(|n| n == "Photos/dog.jpg"));
        assert!(names.iter().any(|n| n == "Photos/vacation/beach.jpg"));
    }

    #[tokio::test]
    async fn stream_returns_too_large_without_writing() {
        let view = crate::walk::tests::seed_view().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        let r = stream_folder(
            &view,
            &UserPath::new("/Photos").unwrap(),
            ZipCaps { max_entries: 1, max_bytes: 1024 },
            writer,
        )
        .await;
        assert!(matches!(r, Err(ZipError::Walk(WalkError::TooLarge { .. }))));
        // No bytes should have been emitted — channel closes on drop with
        // nothing in flight.
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        assert!(combined.is_empty(), "expected zero bytes, got {} bytes", combined.len());
    }

    #[tokio::test]
    async fn stream_preserves_unicode_names() {
        use crabcloud_storage::{NoopEventSink, StoragePath};
        let view = crate::walk::tests::seed_view().await;
        // Seed a non-ASCII file into the same view.
        let storage = view.mounts()[0].storage.clone();
        storage
            .put_file(
                &StoragePath::new("Photos/Vacaciónes.txt").unwrap(),
                Box::pin(Cursor::new(b"ole".to_vec())),
                &NoopEventSink,
            )
            .await
            .unwrap();
        // Re-scan filecache so the new file is visible.
        // (If the test view uses a real filecache + scanner, this happens
        // implicitly. Otherwise call `view.filecache.full_scan(...)` here.)

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        let view_clone = view; // move ownership
        tokio::spawn(async move {
            stream_folder(
                &view_clone,
                &UserPath::new("/Photos").unwrap(),
                ZipCaps { max_entries: 100, max_bytes: 1024 * 1024 },
                writer,
            )
            .await
        });
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(combined)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "Photos/Vacaciónes.txt"),
            "non-ASCII name lost; got {names:?}",
        );
    }
}
```

If `crate::walk::tests::seed_view` isn't reachable from the stream module (private), copy it inline into stream.rs's test module. The plan stands by either layout.

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-zip stream::tests
```

Expected: 3 tests pass. If `stream_preserves_unicode_names` fails because the filecache isn't refreshed after seeding, look at how `seed_view` does its scan and re-scan after the new put. Add a `view.filecache.full_scan(storage.id(), storage.clone()).await.unwrap();` line after the put.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-zip/src/stream.rs
git commit -m "zip: stream_folder writes UTF-8 zip via SyncIoBridge"
```

### Task A7: Pre-PR sweep + PR

- [ ] **Step 1: Sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Fix any drift. Common issues:
- `unused_crate_dependencies` on the new crate. The dev-deps (`tempfile`, `crabcloud-filecache`, etc.) may not all be referenced. Use `use foo as _;` under `#[cfg(test)]` in `lib.rs`, matching the workspace pattern (e.g., `crabcloud-sharing/src/lib.rs:28-32`).

- [ ] **Step 2: Push + open PR**

```bash
git push -u origin sp9/a-zip-crate
gh pr create --title "sp9(a): crabcloud-zip foundation (walk, compression, stream)" --body "$(cat <<'EOF'
## Summary
- New `crabcloud-zip` crate scaffolding.
- `walk_for_caps`: DFS pre-flight that builds a `ZipPlan` or returns `TooLarge`.
- `compression_for_mime`: const dispatch (text/* → DEFLATE, else STORED).
- `MpscBytesWriter`: `AsyncWrite` adapter over `tokio::sync::mpsc::Sender<Bytes>`.
- `stream_folder`: walks the plan, writes a UTF-8 zip via `SyncIoBridge` + sync `ZipWriter`.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-zip` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Merge after green.**

---

# Batch B — Config + authed handler

**Branch:** `sp9/b-authed-handler`

**Goal:** Add the `FileConfig` cap fields and the authed `GET /api/files/zip/{*path}` route.

### Task B1: `FileConfig` cap fields

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`

- [ ] **Step 1: Add fields**

In `crates/crabcloud-config/src/types.rs`, find the `FileConfig` struct (around line 30). After the existing Crabcloud-specific section, add:

```rust
    /// Folder zip download — max entries (files + directories) the
    /// pre-flight walk will accept before rejecting with 413. Tunable via
    /// `config.php`. Default 500.
    #[serde(default = "default_folder_zip_max_entries")]
    pub folder_zip_max_entries: u64,
    /// Folder zip download — max total uncompressed bytes the pre-flight
    /// walk will accept before rejecting with 413. Default 2 GiB.
    #[serde(default = "default_folder_zip_max_bytes")]
    pub folder_zip_max_bytes: u64,
```

And the defaults helpers, alongside the existing `default_*` functions:

```rust
fn default_folder_zip_max_entries() -> u64 {
    500
}

fn default_folder_zip_max_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}
```

- [ ] **Step 2: Update any test-fixture configs**

Search for `FileConfig {` constructions in the crate's tests and add the two fields:

```bash
rg "FileConfig \{" crates/crabcloud-config/src
```

For each match, add `folder_zip_max_entries: 500, folder_zip_max_bytes: 2 * 1024 * 1024 * 1024,` (or call the default helpers).

- [ ] **Step 3: Run tests**

```bash
cargo test -p crabcloud-config
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-config/
git commit -m "config: folder_zip_max_entries + folder_zip_max_bytes (defaults 500 / 2 GiB)"
```

### Task B2: Authed `GET /api/files/zip/{*path}` handler

**Files:**
- Create: `crates/crabcloud-http/src/routes/files_zip.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs`
- Modify: `crates/crabcloud-http/src/router.rs`
- Modify: `crates/crabcloud-http/Cargo.toml`

- [ ] **Step 1: Add deps to crabcloud-http**

In `crates/crabcloud-http/Cargo.toml`, add to `[dependencies]` if not already present:

```toml
crabcloud-zip = { workspace = true }
bytes = { workspace = true }
tokio-stream = { workspace = true }
```

If `tokio-stream` is not in the workspace's `[workspace.dependencies]`, add it there: `tokio-stream = "0.1"`.

- [ ] **Step 2: Register the module**

In `crates/crabcloud-http/src/routes/mod.rs`, add `pub mod files_zip;` next to the existing `pub mod public_link;` / `pub mod public_dav;` entries.

- [ ] **Step 3: Write the handler**

Create `crates/crabcloud-http/src/routes/files_zip.rs`:

```rust
//! `GET /api/files/zip/{*path}` — authenticated folder zip download.

use crate::auth_context::AuthContext;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use bytes::Bytes;
use crabcloud_core::AppState;
use crabcloud_fs::path::UserPath;
use crabcloud_storage::FileKind;
use crabcloud_zip::{stream_folder, MpscBytesWriter, WalkError, ZipCaps, ZipError};
use serde::Serialize;
use tokio_stream::wrappers::ReceiverStream;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/files/zip/", get(handler_root))
        .route("/api/files/zip/{*path}", get(handler))
}

async fn handler_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Response {
    handle_zip(state, ctx, String::new()).await
}

async fn handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(path): Path<String>,
) -> Response {
    handle_zip(state, ctx, path).await
}

async fn handle_zip(state: AppState, ctx: AuthContext, raw_path: String) -> Response {
    let user_path_str = if raw_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", raw_path.trim_start_matches('/'))
    };
    let user_path = match UserPath::new(user_path_str.clone()) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };
    let view = match state.view_for(&ctx.user_id).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    // 400 if path is a regular file rather than a directory.
    match view.stat(&user_path).await {
        Ok(meta) if matches!(meta.kind, FileKind::Directory) => {}
        Ok(_) => return (StatusCode::BAD_REQUEST, "not a directory").into_response(),
        Err(crabcloud_fs::FsError::NotFound) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(crabcloud_fs::FsError::Storage(crabcloud_storage::StorageError::NotFound)) => {
            return (StatusCode::NOT_FOUND, "").into_response()
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }
    let caps = ZipCaps {
        max_entries: state.config.folder_zip_max_entries,
        max_bytes: state.config.folder_zip_max_bytes,
    };
    let archive_basename = basename_for_zip(&user_path, ctx.user_id.as_str());

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
    let writer = MpscBytesWriter::new(tx.clone());

    // Drive the zip stream from a dedicated task. We need to handle the
    // TooLarge case specially: if `walk_for_caps` rejects, we must NOT
    // send a 200 — we want to send 413. So we pre-walk before responding.
    match crabcloud_zip::walk_for_caps(&view, &user_path, &caps).await {
        Ok(_) => {
            // Spawn the actual stream task and respond with the body now.
            tokio::spawn(async move {
                if let Err(e) = stream_folder(&view, &user_path, caps, writer).await {
                    tracing::warn!(error = %e, "authed zip stream failed mid-flight");
                }
            });
            zip_response(archive_basename, rx)
        }
        Err(WalkError::TooLarge { count, bytes }) => too_large_response(count, bytes, caps),
        Err(WalkError::View(_)) => (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }
}

fn basename_for_zip(user_path: &UserPath, fallback: &str) -> String {
    let trimmed = user_path.as_str().trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn zip_response(
    basename: String,
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    let safe_ascii: String = basename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let percent = urlencoding::encode(&basename);
    let disp = format!(
        "attachment; filename=\"{}.zip\"; filename*=UTF-8''{}.zip",
        safe_ascii, percent
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disp).unwrap_or(HeaderValue::from_static("attachment")),
    );
    let stream = ReceiverStream::new(rx);
    let body = Body::from_stream(stream);
    (StatusCode::OK, headers, body).into_response()
}

#[derive(Serialize)]
struct OverCapBody {
    error: &'static str,
    entries: u64,
    bytes: u64,
    limits: OverCapLimits,
}
#[derive(Serialize)]
struct OverCapLimits {
    max_entries: u64,
    max_bytes: u64,
}

fn too_large_response(count: u64, bytes: u64, caps: ZipCaps) -> Response {
    let body = OverCapBody {
        error: "folder too large",
        entries: count,
        bytes,
        limits: OverCapLimits {
            max_entries: caps.max_entries,
            max_bytes: caps.max_bytes,
        },
    };
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(body),
    )
        .into_response()
}
```

- [ ] **Step 4: Wire into `router.rs`**

In `crates/crabcloud-http/src/router.rs`, find where the OCS / DAV / public-link routers are merged and add:

```rust
        .merge(crate::routes::files_zip::router())
```

It must be inside the authed surface — the `AuthLayer` already attaches `AuthContext` for `/api/...` paths in the existing app (verify by reading the router's `merge` calls). If `AuthLayer` is NOT yet applied to `/api/files/zip/*`, attach it via `.route_layer(...)` on the `files_zip::router()` invocation, mirroring how `/api/public_link/list` is currently auth'd (the dx server-fn surface). When in doubt, copy the pattern from the existing `/api/public_link/list` registration.

- [ ] **Step 5: Build**

```bash
cargo build -p crabcloud-http
```

Expected: clean. Fix any clippy issues now.

- [ ] **Step 6: Commit**

```bash
git add crates/crabcloud-http/
git commit -m "http: authed /api/files/zip/{*path} handler"
```

### Task B3: E2E tests — authed surface

**Files:**
- Modify: `crates/crabcloud-http/tests/support/mod.rs`
- Create: `crates/crabcloud-http/tests/files_zip_e2e.rs`

- [ ] **Step 1: Extend the support module**

In `crates/crabcloud-http/tests/support/mod.rs`, add a helper that seeds a small directory tree for zip tests. Find an existing seed helper (e.g. `seed_folder`) and add alongside:

```rust
/// Seed a small fixed tree at `<root>` containing `<root>/cat.txt`,
/// `<root>/dog.txt`, and `<root>/vacation/beach.txt` with byte contents
/// derived from the filename. Returns the absolute storage paths of the
/// files for later assertions.
pub async fn seed_zip_tree(
    state: &crabcloud_core::AppState,
    uid: &crabcloud_users::UserId,
    root: &str,
) {
    seed_folder(state, uid, root).await;
    seed_folder(state, uid, &format!("{root}/vacation")).await;
    seed_file(state, uid, &format!("{root}/cat.txt"), b"cat-text").await;
    seed_file(state, uid, &format!("{root}/dog.txt"), b"dog-text").await;
    seed_file(
        state,
        uid,
        &format!("{root}/vacation/beach.txt"),
        b"beach-text-bytes",
    )
    .await;
}
```

- [ ] **Step 2: Write the e2e tests**

Create `crates/crabcloud-http/tests/files_zip_e2e.rs`:

```rust
//! E2E for the authed folder-zip endpoint.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use std::io::Cursor;
use support::{make_state, seed_user, seed_zip_tree};
use tower::ServiceExt;

#[tokio::test]
async fn authed_zip_returns_200_application_zip() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    seed_zip_tree(&state, &uid, "/Photos").await;
    let router = crate::support::authed_router_for(&state, &uid);
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("filename=\"Photos.zip\""), "got: {cd}");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(Cursor::new(body.to_vec())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert!(names.iter().any(|n| n == "Photos/cat.txt"));
    assert!(names.iter().any(|n| n == "Photos/dog.txt"));
    assert!(names.iter().any(|n| n == "Photos/vacation/beach.txt"));
}

#[tokio::test]
async fn authed_zip_over_cap_returns_413_with_summary() {
    let (mut state, _tmp) = make_state().await;
    // Force the cap to 1 entry — anything with more than one file overflows.
    state.config.folder_zip_max_entries = 1;
    let uid = seed_user(&state, "alice").await;
    seed_zip_tree(&state, &uid, "/Photos").await;
    let router = crate::support::authed_router_for(&state, &uid);
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "folder too large");
    assert!(v["entries"].as_u64().unwrap() >= 2);
    assert_eq!(v["limits"]["max_entries"], 1);
}

#[tokio::test]
async fn authed_zip_of_regular_file_returns_400() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    support::seed_file(&state, &uid, "/note.txt", b"hello").await;
    let router = crate::support::authed_router_for(&state, &uid);
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn authed_zip_unknown_path_returns_404() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let router = crate::support::authed_router_for(&state, &uid);
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/does_not_exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn authed_zip_root_uses_uid_basename() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    seed_zip_tree(&state, &uid, "/Photos").await;
    let router = crate::support::authed_router_for(&state, &uid);
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("filename=\"alice.zip\""), "got: {cd}");
}
```

Note the helper `support::authed_router_for(&state, &uid)` is new. Add it to `tests/support/mod.rs`:

```rust
/// Build the full router and pre-attach `AuthContext` for `uid` so authed
/// handlers run without an actual session cookie.
pub fn authed_router_for(
    state: &crabcloud_core::AppState,
    uid: &crabcloud_users::UserId,
) -> axum::Router {
    use axum::Extension;
    let ctx = crate::support::auth_context_for(uid);
    crabcloud_http::build_router(state.clone()).layer(Extension(ctx))
}
```

If `auth_context_for` doesn't exist, write it: it constructs an `AuthContext` with the given user_id and the minimum other fields required (find the existing test pattern in `crates/crabcloud-http/tests/dav_basic.rs` for the canonical construction).

- [ ] **Step 3: Run tests**

```bash
cargo test -p crabcloud-http --test files_zip_e2e
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-http/
git commit -m "http(tests): e2e for authed folder zip (5 cases)"
```

### Task B4: Pre-PR sweep + PR

- [ ] **Step 1: Sweep + push + PR**

Standard sweep, then:

```bash
git push -u origin sp9/b-authed-handler
gh pr create --title "sp9(b): config caps + authed /api/files/zip/{*path}" --body "$(cat <<'EOF'
## Summary
- `FileConfig::folder_zip_max_entries` + `folder_zip_max_bytes` (defaults 500 / 2 GiB).
- New authed handler `GET /api/files/zip/{*path}`, mounted under the existing `AuthLayer`.
- 5 e2e tests: happy path, 413 over-cap, 400 on non-directory, 404 on unknown, root uses uid basename.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-http --test files_zip_e2e` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Merge after green.**

---

# Batch C — Public-link handler

**Branch:** `sp9/c-public-handler`

**Goal:** Ship `GET /s/{token}/zip/{*path}` on the public-link router. Reuses the public-link handler patterns established by SP8 Batch E-Public.

### Task C1: Public `GET /s/{token}/zip/{*path}` handler

**Files:**
- Modify: `crates/crabcloud-http/src/routes/public_link/mod.rs`

- [ ] **Step 1: Register the route**

In `public_link/mod.rs`, find `pub fn router() -> Router<AppState>` (around line 55) and add to its chain:

```rust
        .route("/s/{token}/zip/", axum::routing::get(zip_handler_root))
        .route("/s/{token}/zip/{*path}", axum::routing::get(zip_handler))
```

- [ ] **Step 2: Add the handler functions**

At the end of `public_link/mod.rs` (after the existing `upload_handler`), add:

```rust
async fn zip_handler_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path(token): Path<String>,
) -> Response {
    handle_public_zip(state, ctx, token, String::new()).await
}

async fn zip_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((token, path)): Path<(String, String)>,
) -> Response {
    handle_public_zip(state, ctx, token, path).await
}

async fn handle_public_zip(
    state: AppState,
    ctx: PublicLinkAuthContext,
    token: String,
    raw_path: String,
) -> Response {
    use bytes::Bytes;
    use crabcloud_fs::path::UserPath;
    use crabcloud_sharing::SharePermissions;
    use crabcloud_storage::FileKind;
    use crabcloud_zip::{stream_folder, walk_for_caps, MpscBytesWriter, WalkError, ZipCaps};
    use tokio_stream::wrappers::ReceiverStream;

    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.contains_read() {
        return (StatusCode::FORBIDDEN, "read_not_granted").into_response();
    }

    let user_path_str = if raw_path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", raw_path.trim_start_matches('/'))
    };
    let user_path = match UserPath::new(user_path_str) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };

    // Build the View via PublicLinkMountResolver (SP8 Batch C).
    let resolver = std::sync::Arc::new(crabcloud_fs::PublicLinkMountResolver::new(
        state.storage_factory.clone(),
        ctx.owner_uid.clone(),
        ctx.owner_path.clone(),
        perms,
    ));
    let mounts = match resolver.mounts_for(&ctx.owner_uid).await {
        Ok(m) => m,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    let view = crabcloud_fs::View::new(
        ctx.owner_uid.clone(),
        mounts,
        state.filecache.clone(),
        state.storage_sink.clone(),
    );

    // Reject if path is not a directory (or doesn't exist).
    match view.stat(&user_path).await {
        Ok(meta) if matches!(meta.kind, FileKind::Directory) => {}
        Ok(_) => return (StatusCode::BAD_REQUEST, "not a directory").into_response(),
        Err(crabcloud_fs::FsError::NotFound) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(crabcloud_fs::FsError::Storage(crabcloud_storage::StorageError::NotFound)) => {
            return (StatusCode::NOT_FOUND, "").into_response()
        }
        Err(crabcloud_fs::FsError::Storage(crabcloud_storage::StorageError::PermissionDenied)) => {
            return (StatusCode::FORBIDDEN, "").into_response()
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }

    let caps = ZipCaps {
        max_entries: state.config.folder_zip_max_entries,
        max_bytes: state.config.folder_zip_max_bytes,
    };

    let archive_basename = public_basename_for_zip(&user_path, &token);

    // Pre-walk so we can return 413 cleanly before any body is sent.
    match walk_for_caps(&view, &user_path, &caps).await {
        Ok(_) => {}
        Err(WalkError::TooLarge { count, bytes }) => {
            return public_too_large_response(count, bytes, caps);
        }
        Err(WalkError::View(_)) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
    let writer = MpscBytesWriter::new(tx);
    let view_for_task = view;
    let user_path_for_task = user_path;
    tokio::spawn(async move {
        if let Err(e) = stream_folder(&view_for_task, &user_path_for_task, caps, writer).await {
            tracing::warn!(error = %e, "public-link zip stream failed mid-flight");
        }
    });

    let stream = ReceiverStream::new(rx);
    let body = axum::body::Body::from_stream(stream);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zip"),
    );
    let safe_ascii: String = archive_basename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let percent = urlencoding::encode(&archive_basename);
    let disp = format!(
        "attachment; filename=\"{}.zip\"; filename*=UTF-8''{}.zip",
        safe_ascii, percent
    );
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&disp)
            .unwrap_or(axum::http::HeaderValue::from_static("attachment")),
    );
    (StatusCode::OK, headers, body).into_response()
}

fn public_basename_for_zip(user_path: &crabcloud_fs::path::UserPath, token: &str) -> String {
    let trimmed = user_path.as_str().trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return token.to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| token.to_string())
}

#[derive(serde::Serialize)]
struct PublicOverCapBody {
    error: &'static str,
    entries: u64,
    bytes: u64,
    limits: PublicOverCapLimits,
}
#[derive(serde::Serialize)]
struct PublicOverCapLimits {
    max_entries: u64,
    max_bytes: u64,
}

fn public_too_large_response(
    count: u64,
    bytes: u64,
    caps: crabcloud_zip::ZipCaps,
) -> Response {
    let body = PublicOverCapBody {
        error: "folder too large",
        entries: count,
        bytes,
        limits: PublicOverCapLimits {
            max_entries: caps.max_entries,
            max_bytes: caps.max_bytes,
        },
    };
    (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response()
}
```

- [ ] **Step 3: Build**

```bash
cargo build -p crabcloud-http
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-http/src/routes/public_link/mod.rs
git commit -m "http(public_link): GET /s/{token}/zip/{*path} folder-zip handler"
```

### Task C2: E2E tests — public surface

**Files:**
- Modify: `crates/crabcloud-http/tests/public_link_e2e.rs`

- [ ] **Step 1: Add the tests**

Append to `tests/public_link_e2e.rs`:

```rust
#[tokio::test]
async fn public_zip_read_link_returns_200() {
    use std::io::Cursor;

    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    support::seed_zip_tree(&state, &uid, "/Photos").await;
    let token = support::create_link(&state, &uid, "/Photos", 1, None, None).await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/zip/"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/zip"
    );
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let mut archive = zip::ZipArchive::new(Cursor::new(body.to_vec())).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.iter().any(|n| n == "Photos/cat.txt"));
}

#[tokio::test]
async fn public_zip_create_only_link_returns_403() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    support::seed_zip_tree(&state, &uid, "/Drop").await;
    // Create-only link (permissions = 4).
    let token = support::create_link(&state, &uid, "/Drop", 4, None, None).await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/zip/"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_zip_expired_token_returns_404() {
    use chrono::NaiveDate;
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    support::seed_zip_tree(&state, &uid, "/Photos").await;
    let past = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let token = support::create_link(&state, &uid, "/Photos", 1, None, Some(past)).await;
    let router = crabcloud_http::build_router(state.clone());
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/zip/"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_zip_password_gated_no_cookie_returns_403() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    support::seed_zip_tree(&state, &uid, "/Photos").await;
    let token = support::create_link(
        &state,
        &uid,
        "/Photos",
        1,
        Some("hunter2".to_string()),
        None,
    )
    .await;
    let router = crabcloud_http::build_router(state.clone());
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/zip/"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert!(
        std::str::from_utf8(&body).unwrap().contains("password_required"),
        "expected password_required marker, got {:?}",
        body
    );
}

#[tokio::test]
async fn public_zip_root_uses_basename() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    support::seed_zip_tree(&state, &uid, "/Photos").await;
    let token = support::create_link(&state, &uid, "/Photos", 1, None, None).await;
    let router = crabcloud_http::build_router(state.clone());
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/zip/"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // The link's `file_target` is `/Photos`, so the archive basename should
    // be `Photos`.
    assert!(cd.contains("filename=\"Photos.zip\""), "got: {cd}");
}
```

If `support::seed_zip_tree` was added in Batch B (under `crates/crabcloud-http/tests/support/mod.rs`), it's already available to this file too (both consume the shared support module).

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-http --test public_link_e2e
```

Expected: pre-existing 12+ tests still pass, plus 5 new zip tests.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-http/tests/public_link_e2e.rs
git commit -m "http(tests): e2e for public-link folder zip (5 cases)"
```

### Task C3: Pre-PR sweep + PR

- [ ] **Step 1: Sweep + push + PR**

Standard sweep, then:

```bash
git push -u origin sp9/c-public-handler
gh pr create --title "sp9(c): public-link folder zip GET /s/{token}/zip/{*path}" --body "$(cat <<'EOF'
## Summary
- New public-link route `GET /s/{token}/zip/{*path}` (and root variant).
- Reuses `crabcloud_zip::stream_folder` + the `PublicLinkMountResolver` path established in SP8 Batch E-Public.
- Enforces read bit, password-gate state, and the operator caps.
- 5 e2e tests: read-link happy path, create-only 403, expired 404, password-gated 403, root uses linked-folder basename.
- Closes SP8 carryforward E7.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-http --test public_link_e2e` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Merge after green.**

---

## Acceptance criteria (spec → coverage map)

| Spec section | Test / artifact |
|---|---|
| §2 Decision 1 (new crate) | Batch A creates `crabcloud-zip`. |
| §2 Decision 2 (symmetric handlers) | Batch B authed handler + Batch C public handler both delegate to `stream_folder`. |
| §2 Decision 3 (config caps) | Batch B Task B1 adds fields; e2e test `authed_zip_over_cap_returns_413_with_summary` exercises them. |
| §2 Decision 4 (compression dispatch) | Batch A Task A4 `compression_for_mime` unit tests cover 9 compressible + 7 stored mimes. |
| §2 Decision 5 (UTF-8 filenames) | Batch A Task A6 `stream_preserves_unicode_names` unit test. |
| §2 Decision 6 (pre-flight) | Batch A Task A3 unit tests; Batch B Task B3 over-cap e2e; Batch C Task C2 over-cap test (optional — covered by Batch B). |
| §2 Decision 7 (Content-Disposition) | Batch B Task B3 `authed_zip_root_uses_uid_basename` + Batch C Task C2 `public_zip_root_uses_basename`. |
| §2 Decision 8 (GET) | All e2e tests use GET. |
| §2 Decision 9 (no Range) | Handlers don't set `Accept-Ranges`. No test required for absence; verified by reading the handler. |
| §2 Decision 10 (permission check) | Batch C Task C2 `public_zip_create_only_link_returns_403` + `public_zip_password_gated_no_cookie_returns_403`. |
| §3.1/3.2 data flows | All e2e tests collectively. |
| §3.4 streaming integration | Batch A Task A5 `MpscBytesWriter` tests + Batch A Task A6 stream test. |
| §4 testing strategy | Mapped above. |
| §5 risks | Mitigations baked into implementation; no separate task. |
