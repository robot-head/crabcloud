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

// Anchor dev-deps used by integration tests in later batches. Their lib-test
// usage is gated to silence `unused-crate-dependencies` until the
// `tests/` directory adds real call sites in Batches B+.
#[cfg(test)]
use crabcloud_cache as _; // used in Batch B integration tests
#[cfg(test)]
use crabcloud_db as _; // used in Batch B integration tests
