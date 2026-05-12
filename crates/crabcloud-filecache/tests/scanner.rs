//! Scanner integration tests: continuous consumption, full-scan
//! reconciliation, and `Lagged` recovery.

mod support;

use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_storage::{
    memory::MemoryStorage, ChannelEventSink, ETag, FileKind, FileMetadata, Mimetype, NoopEventSink,
    Permissions, Storage, StorageEvent, StoragePath,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use support::harness;
use tokio::io::AsyncRead;

// Anchors for crates referenced only by other targets in this crate. Without
// these, `-D warnings` on `cargo clippy --all-targets` flags the integration
// test as missing dependencies that the library + the `tests/support` module
// pull in transitively.
use async_trait as _;
use crabcloud_cache as _;
use crabcloud_users as _;
use dashmap as _;
use hex as _;
use md5 as _;
use sqlx as _;
use thiserror as _;
use tracing as _;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

/// Synthesize a directory `FileMetadata` for the storage root. `MemoryStorage`
/// doesn't materialize a row for the root until something is written under
/// it, so tests that need the root cached up-front build it explicitly.
fn root_dir_metadata() -> FileMetadata {
    FileMetadata {
        path: StoragePath::root(),
        kind: FileKind::Directory,
        size: 0,
        mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::octet_stream(),
        permissions: Permissions::full(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_consumes_written_events_into_cache() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner1"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Seed the root row in the cache so the scanner's apply doesn't fail
    // with AncestorMissing for child writes. We can't get root metadata
    // from `storage.stat(root)` because `MemoryStorage` has no implicit
    // root entry; synthesize one.
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: storage.id().to_string(),
            path: StoragePath::root(),
            metadata: root_dir_metadata(),
        })
        .await
        .unwrap();

    // Spawn the consumer.
    let _handle = scanner.clone().spawn();

    // Give the consumer a brief moment to subscribe to the sink before we
    // emit; otherwise the first event can be dropped (broadcast::Sender
    // discards messages when there are zero receivers).
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Now emit a real write through the sink-bound storage.
    storage
        .put_file(
            &StoragePath::new("scanned.txt").unwrap(),
            body(b"hello".to_vec()),
            &*sink,
        )
        .await
        .unwrap();

    // Wait for the scanner to catch up.
    let target = StoragePath::new("scanned.txt").unwrap();
    let mut attempts = 0;
    loop {
        let row = cache.lookup(storage.id(), &target).await.unwrap();
        if row.is_some() {
            break;
        }
        attempts += 1;
        if attempts > 50 {
            panic!("scanner didn't catch up in time");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_full_scan_reconciles_external_writes() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner2"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Write files DIRECTLY through the storage with a NoopEventSink so the
    // scanner doesn't see live events. Then drive reconciliation via
    // `full_scan`.
    storage
        .put_file(
            &StoragePath::new("a.txt").unwrap(),
            body(b"x".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
    storage
        .put_file(
            &StoragePath::new("b.txt").unwrap(),
            body(b"y".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
    storage
        .put_file(
            &StoragePath::new("c.txt").unwrap(),
            body(b"z".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let count = scanner.full_scan(&storage).await.unwrap();
    assert!(
        count >= 4,
        "expected at least 4 entries (root + 3 files), got {count}"
    );
    assert!(cache
        .lookup(storage.id(), &StoragePath::new("a.txt").unwrap())
        .await
        .unwrap()
        .is_some());
    assert!(cache
        .lookup(storage.id(), &StoragePath::new("b.txt").unwrap())
        .await
        .unwrap()
        .is_some());
    assert!(cache
        .lookup(storage.id(), &StoragePath::new("c.txt").unwrap())
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_lagged_triggers_full_scan_recovery() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    // Tiny capacity so the consumer falls behind quickly.
    let sink = Arc::new(ChannelEventSink::new(4));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner3"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Seed the root row.
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: storage.id().to_string(),
            path: StoragePath::root(),
            metadata: root_dir_metadata(),
        })
        .await
        .unwrap();

    // Spawn the consumer. Give it time to subscribe before flooding.
    let _handle = scanner.clone().spawn();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Emit 20 writes through the sink. With capacity=4, the consumer will
    // either keep up (live apply) or fall behind and recover via full-scan.
    // Either way the cache should reflect all 20 files when we're done.
    for i in 0..20u32 {
        storage
            .put_file(
                &StoragePath::new(format!("f{i:02}.txt")).unwrap(),
                body(vec![b'x'; 1]),
                &*sink,
            )
            .await
            .unwrap();
    }

    // Wait for the scanner to catch up. The last file written is what we
    // poll for; it ends up in the cache either via apply (if the consumer
    // kept up) or via `full_scan` (if it lagged).
    let target = StoragePath::new("f19.txt").unwrap();
    let mut attempts = 0;
    loop {
        let row = cache.lookup(storage.id(), &target).await.unwrap();
        if row.is_some() {
            break;
        }
        attempts += 1;
        if attempts > 100 {
            panic!("scanner didn't recover from lag");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
