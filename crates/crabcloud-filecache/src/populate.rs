//! Cache-miss populate path. Per-`(storage_id, path)` mutex serializes
//! concurrent stat-on-miss for the same path; distinct paths run in
//! parallel.
//!
//! Algorithm for `stat`:
//!   1. Cache hit check via `cache.lookup`.
//!   2. Acquire per-path lock from `populate_locks`.
//!   3. Re-check cache under the lock (another task may have populated
//!      while we were waiting).
//!   4. Backend `storage.stat(path)`. `NotFound` propagates as
//!      `FileCacheError::NotFound` (no negative caching).
//!   5. Recurse parent so its row exists before we INSERT. If parent is
//!      root, ensure the root row exists (lazy populate via DirCreated).
//!   6. Materialize the row through `cache.apply(...)` so it goes through
//!      the same intern + propagation sequence as event-driven writes.
//!   7. Drop the lock guard.
//!   8. Opportunistic cleanup of the lock map entry — if we hold the only
//!      remaining `Arc` (besides the `DashMap`'s), remove the entry to
//!      keep the map from growing unboundedly.

use crabcloud_storage::{
    DirEntry, FileKind, FileMetadata, Storage, StorageError, StorageEvent, StoragePath,
};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{FileCacheError, FileCacheResult};
use crate::schema::FilecacheRow;
use crate::FileCache;

/// Cached stat. On miss, calls `storage.stat(path)` under a per-path
/// lock so concurrent callers for the same path produce one backend
/// stat. Distinct paths populate in parallel.
pub async fn stat(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<FileMetadata> {
    // Box the recursive future — `async fn` recursion needs an explicit
    // boxed return on the recursive call.
    Box::pin(stat_inner(cache, storage, path)).await
}

async fn stat_inner(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<FileMetadata> {
    // 1. Fast path: cache hit.
    if let Some(row) = cache.lookup(storage.id(), path).await? {
        return Ok(row_to_metadata(row));
    }

    // 2. Acquire per-path lock.
    let key = (storage.id().to_string(), path.clone());
    let lock = cache
        .populate_locks
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let guard = lock.lock().await;

    // 3. Re-check cache under the lock — another task may have populated
    // while we were waiting.
    if let Some(row) = cache.lookup(storage.id(), path).await? {
        drop(guard);
        opportunistic_cleanup(cache, &key, &lock);
        return Ok(row_to_metadata(row));
    }

    // 4. Backend stat. NotFound propagates as-is (no negative caching).
    let meta = match storage.stat(path).await {
        Ok(m) => m,
        Err(StorageError::NotFound) => {
            drop(guard);
            opportunistic_cleanup(cache, &key, &lock);
            return Err(FileCacheError::NotFound);
        }
        Err(e) => {
            drop(guard);
            opportunistic_cleanup(cache, &key, &lock);
            return Err(FileCacheError::Storage(e));
        }
    };

    // 5. Ensure parent row exists before we INSERT (avoids
    // `AncestorMissing` from `apply_written`). If parent is root, we
    // intentionally do NOT pre-populate it: `resolve_parent_fileid`
    // returns `Ok(None)` for a missing root parent, and ancestor
    // propagation in `propagate_ancestors_*` already short-circuits when
    // it walks up to a missing root. Some backends (e.g. `MemoryStorage`)
    // don't materialize a root entry until something is written under
    // it, so `storage.stat(root)` would spuriously fail with `NotFound`.
    if let Some(parent) = path.parent() {
        if !parent.is_root() {
            // Recurse — populates `parent` (and its ancestors) into the cache.
            stat(cache, storage, &parent).await?;
        }
    }

    // 6. Materialize the row through `apply` so it goes through the same
    // intern + propagation sequence as event-driven writes.
    let event = if matches!(meta.kind, FileKind::Directory) {
        StorageEvent::DirCreated {
            storage_id: storage.id().to_string(),
            path: path.clone(),
            metadata: meta.clone(),
        }
    } else {
        StorageEvent::Written {
            storage_id: storage.id().to_string(),
            path: path.clone(),
            metadata: meta.clone(),
        }
    };
    cache.apply(&event).await?;

    // 7. Drop guard.
    drop(guard);
    // 8. Opportunistic cleanup.
    opportunistic_cleanup(cache, &key, &lock);
    Ok(meta)
}

/// Cached directory listing. On miss, populates the directory itself +
/// every immediate child (one level deep). Recursion is the scanner's
/// job (Batch D); we don't yet trust the cache as authoritative for
/// "all children present" — sub-project 5 can layer that on.
pub async fn list(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<Vec<DirEntry>> {
    // Ensure the directory itself is populated.
    let _ = stat(cache, storage, path).await?;
    // List from backend.
    let entries = storage.list(path).await?;
    // Populate each child (one level only — no recursion).
    for child in &entries {
        let child_path = if path.is_root() {
            StoragePath::new(child.name.clone())?
        } else {
            path.join(&child.name)?
        };
        stat(cache, storage, &child_path).await?;
    }
    Ok(entries)
}

fn row_to_metadata(row: FilecacheRow) -> FileMetadata {
    use std::time::{Duration, UNIX_EPOCH};
    FileMetadata {
        path: row.path,
        kind: row.kind,
        size: row.size,
        mtime: UNIX_EPOCH + Duration::from_secs(row.mtime),
        etag: row.etag,
        mimetype: row.mimetype,
        permissions: row.permissions,
    }
}

fn opportunistic_cleanup(cache: &FileCache, key: &(String, StoragePath), lock: &Arc<Mutex<()>>) {
    // If we hold the only Arc (besides the DashMap's), remove the entry.
    // Racy but bounded — the next populate just re-creates an Arc.
    // 2 = our local Arc + the one held by the DashMap entry.
    if Arc::strong_count(lock) <= 2 {
        cache.populate_locks.remove(key);
    }
}
