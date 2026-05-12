//! BFS walk of a storage; populates every cache row top-down.
//!
//! Special-cases the storage root: some backends (notably `MemoryStorage`)
//! don't materialize a row for the root path until something is written
//! under it. The scanner treats root as an implicit directory so that
//! `full_scan` works against any backend regardless of whether it stores
//! an explicit root entry.

use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, Permissions, Storage, StorageError, StorageEvent,
    StoragePath,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use crate::error::{FileCacheError, FileCacheResult};
use crate::FileCache;

pub async fn full_scan(cache: &FileCache, storage: &Arc<dyn Storage>) -> FileCacheResult<u64> {
    let mut queue: VecDeque<StoragePath> = VecDeque::new();
    queue.push_back(StoragePath::root());
    let mut count = 0u64;

    while let Some(path) = queue.pop_front() {
        // Populate this row in the cache.
        ensure_cached(cache, storage, &path).await?;
        count += 1;

        // Determine whether to descend. Root is always treated as a
        // directory; non-root paths are stat-checked.
        let is_directory = if path.is_root() {
            true
        } else {
            match storage.stat(&path).await {
                Ok(meta) => matches!(meta.kind, FileKind::Directory),
                Err(StorageError::NotFound) => continue, // race: removed mid-scan
                Err(e) => return Err(FileCacheError::Storage(e)),
            }
        };

        if is_directory {
            let children = match storage.list(&path).await {
                Ok(c) => c,
                Err(StorageError::NotFound) => continue,
                Err(e) => return Err(FileCacheError::Storage(e)),
            };
            for child in children {
                let child_path = if path.is_root() {
                    StoragePath::new(child.name.clone())?
                } else {
                    path.join(&child.name)?
                };
                queue.push_back(child_path);
            }
        }
    }

    cache.stamp_last_checked(storage.id()).await?;
    Ok(count)
}

/// Populate the cache row for `path`. Special-cases the root: if backend
/// `stat(root)` is `NotFound`, the scanner synthesizes a directory row so
/// downstream `Written` apply calls for root-level files don't fail with
/// `AncestorMissing`.
async fn ensure_cached(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<()> {
    if !path.is_root() {
        let _ = cache.stat(storage, path).await?;
        return Ok(());
    }

    // Root path. Cache hit?
    if cache.lookup(storage.id(), path).await?.is_some() {
        return Ok(());
    }

    // Cache miss for root. Try the backend; if it doesn't carry an explicit
    // root entry, synthesize one.
    let meta = match storage.stat(path).await {
        Ok(m) => m,
        Err(StorageError::NotFound) => synthetic_root_metadata(),
        Err(e) => return Err(FileCacheError::Storage(e)),
    };

    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: storage.id().to_string(),
            path: StoragePath::root(),
            metadata: meta,
        })
        .await
}

fn synthetic_root_metadata() -> FileMetadata {
    FileMetadata {
        path: StoragePath::root(),
        kind: FileKind::Directory,
        size: 0,
        mtime: SystemTime::now(),
        etag: ETag::new(),
        mimetype: Mimetype::octet_stream(),
        permissions: Permissions::full(),
    }
}
