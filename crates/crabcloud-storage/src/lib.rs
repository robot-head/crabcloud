//! `crabcloud-storage` — async storage primitives.
//!
//! This crate ships the [`Storage`] trait and supporting types. Two backends
//! live in this crate: [`local::LocalStorage`] (production) and
//! [`memory::MemoryStorage`] (tests + dev).
//!
//! Mutating operations take a [`EventSink`] reference. Sub-project 4a ships
//! [`NoopEventSink`]; sub-project 4b will add a real channel-backed sink that
//! drives the filecache scanner.
//!
//! Future backends (S3 in 4b; SMB/external-storage later) implement
//! [`Storage`] and slot into the same call sites.

pub mod error;
pub mod meta;
pub mod path;

pub mod local;
pub mod memory;

pub use error::{StorageError, StorageResult};
pub use meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
pub use path::StoragePath;

use async_trait::async_trait;
use std::ops::Range;
use std::pin::Pin;
use tokio::io::AsyncRead;

/// Events emitted by [`Storage`] operations. Subscribers in sub-project 4b
/// will use these to keep `oc_filecache` in sync with storage state.
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

/// Receiver for [`StorageEvent`]s. Emissions are fire-and-forget — a failing
/// emit must NOT roll back the storage operation. Failures are logged.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: StorageEvent);
}

/// No-op sink used in sub-project 4a tests and as the default. 4b adds a
/// channel-backed implementation that fans out to subscribed consumers.
pub struct NoopEventSink;

#[async_trait]
impl EventSink for NoopEventSink {
    async fn emit(&self, _event: StorageEvent) {}
}

/// The storage trait. All mutating methods take `&dyn EventSink` so callers
/// can subscribe to the resulting events.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Stable identifier for this storage. Used as `storage_id` in events
    /// and (in 4b) as the foreign-key value for `oc_filecache.storage`.
    fn id(&self) -> &str;

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata>;
    async fn exists(&self, path: &StoragePath) -> StorageResult<bool>;
    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>>;

    async fn read(&self, path: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn mkdir(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<FileMetadata>;

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()>;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // `tempfile` is a dev-dep used by integration tests under `tests/`, and
    // `tracing` is only invoked from xattr_io's Unix-only debug paths. Both
    // appear unused to the `lib test` target on Windows; anchor them here to
    // keep `unused_crate_dependencies` quiet.
    use tempfile as _;
    use tracing as _;

    #[test]
    fn storage_trait_is_object_safe() {
        // Compile-only assertion. If this fails to compile, someone added a
        // non-object-safe method (generic on the trait method, Self in a
        // non-receiver position, etc.).
        fn _accepts(_s: Arc<dyn Storage>) {}
    }

    #[test]
    fn event_sink_is_object_safe() {
        fn _accepts(_s: Arc<dyn EventSink>) {}
    }

    #[tokio::test]
    async fn noop_sink_swallows_events() {
        let sink = NoopEventSink;
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
    }
}
