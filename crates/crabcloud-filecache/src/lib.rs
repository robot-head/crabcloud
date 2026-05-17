//! `crabcloud-filecache` — DB-backed cache for storage state.
//!
//! Mirrors 4a's storage events in `oc_filecache`/`oc_storages`/`oc_mimetypes`
//! so consumers (sub-project 5's WebDAV, future indexes) can serve `stat`/
//! `list` in O(1). Cache-miss populate happens through real-backend stats
//! under a per-path lock. Ancestor `size` + `etag` propagation runs in one
//! DB transaction per event — matches upstream Nextcloud behavior so desktop
//! sync clients see byte-identical ETags at every level.

pub mod error;
pub mod locks;
pub mod mimetypes;
pub mod populate;
pub mod propagate;
pub mod properties;
pub mod scanner;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use locks::{LockRow, LockStore};
pub use properties::{PropertyRow, PropertyStore};
pub use scanner::Scanner;
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

use crabcloud_db::DbPool;
use crabcloud_storage::{DirEntry, FileMetadata, Storage, StorageEvent, StoragePath};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// Anchors for crates whose first real call site lands in later batches
// (`async-trait` traits in Batch D; `crabcloud-cache` integrations in
// Batch E). Keeps the workspace-wide `unused_crate_dependencies` lint
// quiet without losing the manifest entries.
use async_trait as _;
use crabcloud_cache as _;
use crabcloud_config as _;
use tracing as _;

/// The cache façade. Constructed via [`FileCache::new`]; subsequent reads
/// (`stat`/`list`/`lookup`/`lookup_by_id`) and writes (`apply`) all dispatch
/// through the shared `DbPool`. Per-process intern caches for storages +
/// mimetypes keep round-trip cost down on the hot path; `populate_locks`
/// serializes concurrent cache-miss populates for the same `(storage, path)`.
pub struct FileCache {
    pool: DbPool,
    pub(crate) storage_ids: DashMap<String, i64>,
    pub(crate) mimetypes: DashMap<String, i64>,
    pub(crate) populate_locks: DashMap<(String, StoragePath), Arc<Mutex<()>>>,
}

impl FileCache {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            storage_ids: DashMap::new(),
            mimetypes: DashMap::new(),
            populate_locks: DashMap::new(),
        }
    }

    /// Access to the shared pool. `pub` so downstream crates can construct
    /// auxiliary stores (`PropertyStore`, `LockStore`) that share this
    /// connection pool without re-opening the DB.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Intern a `Storage::id()` string into the numeric `oc_storages.numeric_id`
    /// that the rest of the schema (filecache, versions, etc.) joins against.
    /// Pass-through to [`storages::intern_storage`] using this cache's own
    /// per-process intern map — downstream crates that need the numeric id
    /// (e.g. `crabcloud-versions`'s storage_id column) can call this without
    /// poking at private fields.
    pub async fn intern_storage(&self, storage_id: &str) -> FileCacheResult<i64> {
        storages::intern_storage(&self.pool, &self.storage_ids, storage_id).await
    }

    /// Pass-through to [`PropertyStore::get_many`] for one named property
    /// across many paths. Used by PROPFIND to fetch `{oc:}favorite` (or
    /// any per-resource custom prop) for an entire directory listing in
    /// a single round-trip.
    pub async fn get_property_many(
        &self,
        userid: &crabcloud_users::UserId,
        propertypaths: &[String],
        propertyname: &str,
    ) -> FileCacheResult<Vec<(String, Option<String>)>> {
        let ps = PropertyStore::new(self.pool.clone());
        ps.get_many(userid, propertypaths, propertyname).await
    }

    /// Cached stat. On miss, calls `storage.stat(path)` under a per-path
    /// lock so concurrent callers for the same path produce one backend
    /// stat. Distinct paths populate in parallel.
    pub async fn stat(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<FileMetadata> {
        populate::stat(self, storage, path).await
    }

    /// Cached directory listing. On miss, populates the directory itself +
    /// every immediate child (one level). Returns the cache rows shaped as
    /// [`DirEntry`].
    pub async fn list(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<Vec<DirEntry>> {
        populate::list(self, storage, path).await
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

    /// Update `oc_storages.last_checked` for `storage_id`. Called by the
    /// scanner at the end of `full_scan` (Batch D).
    pub async fn stamp_last_checked(&self, storage_id: &str) -> FileCacheResult<()> {
        storages::stamp_last_checked(&self.pool, storage_id).await
    }
}
