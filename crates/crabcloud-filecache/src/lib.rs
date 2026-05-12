//! `crabcloud-filecache` — DB-backed cache for storage state.
//!
//! Mirrors 4a's storage events in `oc_filecache`/`oc_storages`/`oc_mimetypes`
//! so consumers (sub-project 5's WebDAV, future indexes) can serve `stat`/
//! `list` in O(1). Cache-miss populate happens through real-backend stats
//! under a per-path lock. Ancestor `size` + `etag` propagation runs in one
//! DB transaction per event — matches upstream Nextcloud behavior so desktop
//! sync clients see byte-identical ETags at every level.

pub mod error;
pub mod mimetypes;
pub mod propagate;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

use crabcloud_db::DbPool;
use crabcloud_storage::{StorageEvent, StoragePath};
use dashmap::DashMap;

// Anchors for crates whose first real call site lands in later batches
// (`async-trait` traits in Batch D; `tokio::sync` locks in Batch C;
// `crabcloud-cache` integrations in Batch E). Keeps the workspace-wide
// `unused_crate_dependencies` lint quiet without losing the manifest entries.
use async_trait as _;
use crabcloud_cache as _;
use crabcloud_config as _;
use tokio as _;
use tracing as _;

/// The cache façade. Constructed via [`FileCache::new`]; subsequent reads
/// (`lookup`/`lookup_by_id`) and writes (`apply`) all dispatch through the
/// shared `DbPool`. Per-process intern caches for storages + mimetypes
/// keep round-trip cost down on the hot path.
pub struct FileCache {
    pool: DbPool,
    pub(crate) storage_ids: DashMap<String, i64>,
    pub(crate) mimetypes: DashMap<String, i64>,
}

impl FileCache {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            storage_ids: DashMap::new(),
            mimetypes: DashMap::new(),
        }
    }

    pub(crate) fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Apply a `StorageEvent` to the cache. Each event handler runs its
    /// leaf mutation + ancestor propagation in one transaction.
    pub async fn apply(&self, event: &StorageEvent) -> FileCacheResult<()> {
        propagate::apply_event(self, event).await
    }

    /// Lookup a row by `(storage_id, path)` without populating on miss.
    pub async fn lookup(
        &self,
        storage_id: &str,
        path: &StoragePath,
    ) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row(self, storage_id, path).await
    }

    /// Lookup a row by `fileid`.
    pub async fn lookup_by_id(&self, fileid: i64) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row_by_id(self, fileid).await
    }
}
