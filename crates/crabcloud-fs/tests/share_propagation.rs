//! Spec §10 carry-forward #1: when alice deletes a file behind a share,
//! bob's view through the share mount must observe the delete.
//!
//! The propagation path is:
//!     View::delete → storage.delete(sink) → StorageEvent::Deleted
//!         → Scanner::spawn loop → FileCache::apply → row removed
//! Bob's `view.list("/Vacation")` resolves into alice's storage (through
//! `SharedSubrootStorage`) so the deletion is visible from his side too.

mod support;

use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_fs::UserPath;
use crabcloud_storage::StoragePath;
use crabcloud_storage::{memory::MemoryStorage, Storage};
use std::sync::Arc;
use std::time::Duration;
use support::{view_home, view_with_share_mount, Harness};

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_core as _;
use crabcloud_sharing as _;
use thiserror as _;
use tracing as _;

use chrono as _;
use serde_json as _;
fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

/// Wait until `cache.lookup(storage_id, path)` returns `Ok(None)` or panic
/// after `max_attempts * step`. The scanner consumer applies events
/// asynchronously, so a busy-wait is the simplest pattern (mirrors the
/// existing approach in `crabcloud-filecache/tests/scanner.rs`).
async fn wait_until_missing(cache: &FileCache, storage_id: &str, path: &StoragePath) {
    for _ in 0..50 {
        let row = cache.lookup(storage_id, path).await.unwrap();
        if row.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    panic!(
        "filecache row for {storage_id}:{} never disappeared",
        path.as_str()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_through_alice_propagates_to_bob_view_via_share_mount() {
    // Build a harness whose `storage` is alice's home. Then construct a
    // bob view that mounts alice's storage at `/Vacation` via the share
    // wrapper. Both views share the same FileCache + ChannelEventSink so
    // the scanner's apply removes the row that bob then can't see.
    let h: Harness = support::harness().await;
    let alice_storage = h.storage.clone();

    // Seed alice's `/Vacation` folder + `/Vacation/x.jpg` directly via
    // her storage (which is `h.storage`). Use the sink so the scanner
    // populates the filecache rows — but we need to spawn the scanner
    // first, and seed the root row by hand because MemoryStorage has no
    // implicit root entry.
    seed_root_row(&h.filecache, alice_storage.id()).await;

    let scanner = Arc::new(Scanner::new(h.filecache.clone(), h.sink.clone()));
    scanner.register_storage(alice_storage.clone());
    let _handle = scanner.clone().spawn();

    // Give the consumer a moment to subscribe before we emit (matches
    // the pattern in crabcloud-filecache/tests/scanner.rs).
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Seed alice's content through her home view so the scanner sees the
    // events and the filecache row for `/Vacation/x.jpg` is written.
    let alice_view = view_home(&h);
    alice_view
        .mkdir(&UserPath::new("/Vacation").unwrap())
        .await
        .unwrap();
    alice_view
        .put_file(
            &UserPath::new("/Vacation/x.jpg").unwrap(),
            body(b"jpeg".to_vec()),
        )
        .await
        .unwrap();

    // Wait until the file's filecache row materializes so the assertion
    // after the delete is meaningful (without this we could race the
    // scanner and "verify" absence that was simply never present).
    let target = StoragePath::new("Vacation/x.jpg").unwrap();
    let alice_sid = alice_storage.id().to_string();
    wait_until_present(&h.filecache, &alice_sid, &target).await;

    // Build bob's view: bob has an empty home, plus a share mount onto
    // alice's `/Vacation` surfaced at bob's `/Vacation`.
    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let bob_view =
        view_with_share_mount(&h, bob_home, alice_storage.clone(), "Vacation", "Vacation");

    // Sanity: pre-delete, bob can see `x.jpg` through the share mount.
    let pre = bob_view
        .list(&UserPath::new("/Vacation").unwrap())
        .await
        .unwrap();
    let pre_names: Vec<&str> = pre.iter().map(|e| e.name.as_str()).collect();
    assert!(
        pre_names.contains(&"x.jpg"),
        "expected x.jpg visible to bob pre-delete, got {pre_names:?}",
    );

    // Now alice deletes /Vacation/x.jpg through her home view. We use
    // `hard_delete` here so the bytes actually leave alice's MemoryStorage
    // and emit a `StorageEvent::Deleted` for the scanner to apply. The
    // SP12 soft-delete path (`View::delete`) moves bytes on disk under
    // `<datadir>/<uid>/files/...`, which doesn't apply to MemoryStorage
    // and isn't what this share-propagation test is verifying anyway.
    alice_view
        .hard_delete(&UserPath::new("/Vacation/x.jpg").unwrap())
        .await
        .unwrap();

    wait_until_missing(&h.filecache, &alice_sid, &target).await;

    // The filecache row for `(alice_sid, /Vacation/x.jpg)` is now gone.
    assert!(h
        .filecache
        .lookup(&alice_sid, &target)
        .await
        .unwrap()
        .is_none());

    // Bob's listing of /Vacation through the share mount no longer
    // returns x.jpg. The View's `list` reads through the share-wrapped
    // storage (which translates root → alice's `/Vacation`), and the
    // underlying MemoryStorage no longer holds the file. We list both
    // through the View and directly through alice's storage as a
    // defense-in-depth check that the storage layer was actually mutated.
    let post = bob_view
        .list(&UserPath::new("/Vacation").unwrap())
        .await
        .unwrap();
    let post_names: Vec<&str> = post.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !post_names.contains(&"x.jpg"),
        "x.jpg still visible to bob post-delete: {post_names:?}",
    );

    // Also verify the underlying alice storage actually doesn't contain
    // it any more (proves the delete went through, not just the cache).
    let alice_listed = alice_storage
        .list(&StoragePath::new("Vacation").unwrap())
        .await
        .unwrap();
    let alice_names: Vec<&str> = alice_listed.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !alice_names.contains(&"x.jpg"),
        "alice storage still holds x.jpg: {alice_names:?}",
    );
}

/// Seed alice's storage-root row directly so the scanner doesn't fail
/// `AncestorMissing` on the first child write. Mirrors the helper used
/// in `crabcloud-filecache/tests/scanner.rs`.
async fn seed_root_row(cache: &FileCache, storage_id: &str) {
    use crabcloud_storage::{ETag, FileKind, FileMetadata, Mimetype, Permissions};
    use std::time::{Duration as StdDuration, SystemTime};

    let md = FileMetadata {
        path: StoragePath::root(),
        kind: FileKind::Directory,
        size: 0,
        mtime: SystemTime::UNIX_EPOCH + StdDuration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::octet_stream(),
        permissions: Permissions::full(),
    };
    cache
        .apply(&crabcloud_storage::StorageEvent::DirCreated {
            storage_id: storage_id.to_string(),
            path: StoragePath::root(),
            metadata: md,
        })
        .await
        .unwrap();
}

async fn wait_until_present(cache: &FileCache, storage_id: &str, path: &StoragePath) {
    for _ in 0..50 {
        let row = cache.lookup(storage_id, path).await.unwrap();
        if row.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    panic!(
        "filecache row for {storage_id}:{} never materialized",
        path.as_str()
    );
}
