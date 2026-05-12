//! Cache-miss populate integration tests.

mod support;

use async_trait::async_trait;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{
    memory::MemoryStorage, DirEntry, EventSink, FileMetadata, MultipartHandle, NoopEventSink,
    PartTag, Storage, StoragePath, StorageResult,
};
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use support::{harness, harness_concurrent};
use tokio::io::AsyncRead;

// Anchors for crates referenced only by other targets in this crate. Without
// these, `-D warnings` on `cargo clippy --all-targets` flags the integration
// test as missing dependencies that the library + the `tests/support` module
// pull in transitively.
use crabcloud_cache as _;
use crabcloud_users as _;
use dashmap as _;
use hex as _;
use md5 as _;
use sqlx as _;
use thiserror as _;
use tracing as _;

/// Storage wrapper that counts `stat()` calls. Delegates every other
/// method to the inner storage so we can wrap a `MemoryStorage` and
/// observe how many backend stats `FileCache::stat` triggers.
struct CountingStorage {
    inner: Arc<dyn Storage>,
    stat_count: Arc<AtomicU32>,
}

#[async_trait]
impl Storage for CountingStorage {
    fn id(&self) -> &str {
        self.inner.id()
    }
    async fn stat(&self, p: &StoragePath) -> StorageResult<FileMetadata> {
        self.stat_count.fetch_add(1, Ordering::SeqCst);
        self.inner.stat(p).await
    }
    async fn exists(&self, p: &StoragePath) -> StorageResult<bool> {
        self.inner.exists(p).await
    }
    async fn list(&self, p: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        self.inner.list(p).await
    }
    async fn read(&self, p: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        self.inner.read(p).await
    }
    async fn read_range(
        &self,
        p: &StoragePath,
        r: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        self.inner.read_range(p, r).await
    }
    async fn put_file(
        &self,
        p: &StoragePath,
        b: Pin<Box<dyn AsyncRead + Send>>,
        s: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        self.inner.put_file(p, b, s).await
    }
    async fn mkdir(&self, p: &StoragePath, s: &dyn EventSink) -> StorageResult<FileMetadata> {
        self.inner.mkdir(p, s).await
    }
    async fn delete(&self, p: &StoragePath, s: &dyn EventSink) -> StorageResult<()> {
        self.inner.delete(p, s).await
    }
    async fn rename(
        &self,
        f: &StoragePath,
        t: &StoragePath,
        s: &dyn EventSink,
    ) -> StorageResult<()> {
        self.inner.rename(f, t, s).await
    }
    async fn copy(&self, f: &StoragePath, t: &StoragePath, s: &dyn EventSink) -> StorageResult<()> {
        self.inner.copy(f, t, s).await
    }
    async fn begin_multipart(
        &self,
        t: &StoragePath,
        s: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        self.inner.begin_multipart(t, s).await
    }
    async fn put_part(
        &self,
        h: &MultipartHandle,
        n: u32,
        b: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        self.inner.put_part(h, n, b).await
    }
    async fn commit_multipart(
        &self,
        h: MultipartHandle,
        p: Vec<PartTag>,
        s: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        self.inner.commit_multipart(h, p, s).await
    }
    async fn abort_multipart(&self, h: MultipartHandle) -> StorageResult<()> {
        self.inner.abort_multipart(h).await
    }
}

fn body(bytes: Vec<u8>) -> Pin<Box<dyn AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

async fn seed_one_file(storage: &Arc<dyn Storage>, path: &str, bytes: &[u8]) {
    storage
        .put_file(
            &StoragePath::new(path).unwrap(),
            body(bytes.to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn stat_cache_miss_populates_then_uses_cache() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate1"));
    seed_one_file(&inner, "hello.txt", b"hi").await;

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("hello.txt").unwrap();
    let _meta1 = cache.stat(&counting, &p).await.unwrap();
    let after_first = count.load(Ordering::SeqCst);
    let _meta2 = cache.stat(&counting, &p).await.unwrap();
    let after_second = count.load(Ordering::SeqCst);

    // First call may stat the leaf + the root; subsequent call should add 0.
    assert!(after_first >= 1);
    assert_eq!(after_first, after_second, "second stat should be cached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stat_cache_miss_concurrent_populates_once() {
    let h = harness_concurrent().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate2"));
    seed_one_file(&inner, "f.txt", b"x").await;

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("f.txt").unwrap();
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let cache = cache.clone();
        let counting = counting.clone();
        let p = p.clone();
        tasks.push(tokio::spawn(async move {
            cache.stat(&counting, &p).await.unwrap();
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // Backend stat hit at most: 1 leaf + 1 root = 2 total. NOT 100.
    let n = count.load(Ordering::SeqCst);
    assert!(n <= 2, "expected <=2 backend stats (leaf + root), got {n}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stat_cache_miss_distinct_paths_run_in_parallel() {
    // We can use a higher N here because `harness_concurrent` pins the
    // pool to `max_connections = 1` (see its docs for why), so sqlx
    // serializes write transactions naturally and there's no SQLITE_BUSY
    // race. The property under test is "the per-path lock doesn't
    // serialize distinct paths" — each task acquires its own lock,
    // backend-stats, then queues for the connection to commit.
    const N: usize = 16;
    let h = harness_concurrent().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate3"));
    // Seed the root directory + one warmup file. The warmup serves two
    // purposes: (a) it interns oc_storages + oc_mimetypes rows so the 16
    // parallel populates below don't race on those upserts, and (b) it
    // populates the root oc_filecache row so each parallel populate only
    // contends on its own leaf INSERT + a UPDATE of the shared root row
    // (rather than ALSO racing to insert root). Without this pre-warm,
    // Linux CI runners under workspace-test load hit SQLITE_BUSY past the
    // 10s busy_timeout.
    seed_one_file(&inner, "_warmup.txt", b"w").await;
    for i in 0..N {
        seed_one_file(&inner, &format!("f-{i:03}.txt"), b"x").await;
    }

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    // Warmup stat (NOT counted against backend-stats budget; the warmup is
    // through `inner` directly above). This populates intern caches + root
    // row before the parallel race begins.
    cache
        .stat(&counting, &StoragePath::new("_warmup.txt").unwrap())
        .await
        .unwrap();
    let warmup_count = count.load(Ordering::SeqCst);

    let mut tasks = Vec::new();
    for i in 0..N {
        let cache = cache.clone();
        let counting = counting.clone();
        tasks.push(tokio::spawn(async move {
            let p = StoragePath::new(format!("f-{i:03}.txt")).unwrap();
            cache.stat(&counting, &p).await.unwrap();
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // N leaf stats — distinct paths must each hit the backend (the
    // populate lock is per-path, so distinct paths never serialize).
    // Add the warmup count to the budget so the bounds describe just the
    // parallel phase.
    let n = count.load(Ordering::SeqCst);
    let parallel = n.saturating_sub(warmup_count);
    assert!(
        parallel as usize >= N,
        "expected at least {N} distinct backend stats in the parallel phase, got {parallel} (warmup={warmup_count}, total={n})"
    );
    assert!(
        parallel as usize <= N + 4,
        "expected at most ~{N} + a few root re-stats in the parallel phase, got {parallel}"
    );
}

#[tokio::test]
async fn stat_cache_miss_not_found_propagates_without_negative_caching() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate4"));

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("ghost.txt").unwrap();
    let r = cache.stat(&counting, &p).await;
    assert!(matches!(
        r,
        Err(crabcloud_filecache::FileCacheError::NotFound)
    ));

    let r2 = cache.stat(&counting, &p).await;
    assert!(matches!(
        r2,
        Err(crabcloud_filecache::FileCacheError::NotFound)
    ));

    // Both calls hit the backend (no negative caching).
    let n = count.load(Ordering::SeqCst);
    assert!(
        n >= 2,
        "expected at least 2 backend stats on repeat NotFound, got {n}"
    );
}
