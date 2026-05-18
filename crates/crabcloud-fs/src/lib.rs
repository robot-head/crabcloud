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
pub mod storage;
pub mod uploads;
pub mod view;

pub use error::{FsError, FsResult};
pub use mount::{Mount, MountKind, MountMetadata, MountResolver, StorageFactory};
pub use path::UserPath;
pub use resolver::local::LocalStorageFactory;
pub use resolver::{
    FileCacheLookup, HomeMountResolver, PublicLinkMountResolver, ShareMountResolver, SharesLookup,
};
pub use storage::SharedSubrootStorage;
pub use uploads::{UploadHandle, Uploads};
pub use view::{ListedEntry, VersionsHooks, View};

// Anchor crates whose real call sites are intentionally test-only or
// reserved for follow-up. `tracing` will be picked up by future warn!/info!
// calls inside Uploads when error logging gets added.
#[cfg(test)]
use crabcloud_config as _;
#[cfg(test)]
use crabcloud_core as _;
#[cfg(test)]
use crabcloud_search as _;
use tracing as _;
