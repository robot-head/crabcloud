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
use std::sync::Arc;
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

impl StorageEvent {
    /// Returns the `storage_id` of the event.
    pub fn storage_id(&self) -> &str {
        match self {
            StorageEvent::Written { storage_id, .. }
            | StorageEvent::DirCreated { storage_id, .. }
            | StorageEvent::Deleted { storage_id, .. }
            | StorageEvent::Moved { storage_id, .. }
            | StorageEvent::Copied { storage_id, .. } => storage_id,
        }
    }
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

/// Broadcast-channel-backed `EventSink`. Wraps `tokio::sync::broadcast`.
/// `emit` is non-blocking and best-effort (a send with zero receivers is
/// dropped silently). Consumers subscribe via [`ChannelEventSink::subscribe`].
pub struct ChannelEventSink {
    tx: tokio::sync::broadcast::Sender<StorageEvent>,
}

impl ChannelEventSink {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StorageEvent> {
        self.tx.subscribe()
    }
}

#[async_trait]
impl EventSink for ChannelEventSink {
    async fn emit(&self, event: StorageEvent) {
        let _ = self.tx.send(event);
    }
}

/// The storage trait. All mutating methods take `&dyn EventSink` so callers
/// can subscribe to the resulting events.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Stable identifier for this storage. Used as `storage_id` in events
    /// and (in 4b) as the foreign-key value for `oc_filecache.storage`.
    fn id(&self) -> &str;

    /// The user that owns this storage, when one can be attributed.
    /// Returns `Some(uid)` for per-user home backends (e.g. `LocalStorage`
    /// minted by `LocalStorageFactory::home_storage`) and `None` for
    /// shared or unattributed backends. Used by the search indexer to
    /// always include the owner in the per-write recipient set; backends
    /// that return `None` skip owner-injection rather than fall back to
    /// path-shape heuristics.
    fn owner_uid(&self) -> Option<&str> {
        None
    }

    /// For wrappers that delegate to an inner storage at a sub-path:
    /// returns the inner storage and the owner-side path prefix.
    ///
    /// Callers that key caches by `(storage.id(), path)` should consult
    /// this and translate to `(inner.id(), prefix.join(path))` before
    /// lookup; otherwise the cache row keyed by the wrapper's
    /// (recipient-relative) path will collide with the owner's actual
    /// rows in the same storage namespace.
    ///
    /// Default: `None` — this storage is not a wrapper.
    fn inner_storage(&self) -> Option<(&Arc<dyn Storage>, &StoragePath)> {
        None
    }

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

#[cfg(test)]
mod storage_event_accessor_tests {
    use super::*;
    use crate::meta::{ETag, FileKind, FileMetadata, Mimetype, Permissions};
    use std::time::SystemTime;

    fn dir_meta() -> FileMetadata {
        FileMetadata {
            path: StoragePath::root(),
            kind: FileKind::Directory,
            size: 0,
            mtime: SystemTime::UNIX_EPOCH,
            etag: ETag::new(),
            mimetype: Mimetype::octet_stream(),
            permissions: Permissions::full(),
        }
    }

    #[test]
    fn storage_id_returns_field_for_each_variant() {
        let ev = StorageEvent::Written {
            storage_id: "a".into(),
            path: StoragePath::root(),
            metadata: dir_meta(),
        };
        assert_eq!(ev.storage_id(), "a");

        let ev = StorageEvent::DirCreated {
            storage_id: "b".into(),
            path: StoragePath::root(),
            metadata: dir_meta(),
        };
        assert_eq!(ev.storage_id(), "b");

        let ev = StorageEvent::Deleted {
            storage_id: "c".into(),
            path: StoragePath::root(),
        };
        assert_eq!(ev.storage_id(), "c");

        let ev = StorageEvent::Moved {
            storage_id: "d".into(),
            from: StoragePath::root(),
            to: StoragePath::root(),
        };
        assert_eq!(ev.storage_id(), "d");

        let ev = StorageEvent::Copied {
            storage_id: "e".into(),
            from: StoragePath::root(),
            to: StoragePath::root(),
        };
        assert_eq!(ev.storage_id(), "e");
    }
}

#[cfg(test)]
mod channel_sink_tests {
    use super::*;

    #[tokio::test]
    async fn emit_with_subscriber_delivers() {
        let sink = ChannelEventSink::new(4);
        let mut rx = sink.subscribe();
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
        let got = rx.recv().await.unwrap();
        assert!(matches!(got, StorageEvent::Deleted { .. }));
    }

    #[tokio::test]
    async fn emit_without_subscriber_does_not_panic() {
        let sink = ChannelEventSink::new(4);
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let sink = ChannelEventSink::new(4);
        let mut rx1 = sink.subscribe();
        let mut rx2 = sink.subscribe();
        sink.emit(StorageEvent::Deleted {
            storage_id: "y".into(),
            path: StoragePath::root(),
        })
        .await;
        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.storage_id(), "y");
        assert_eq!(e2.storage_id(), "y");
    }
}
