//! Integration tests for `FileCache::apply`. Each test builds a fresh
//! SQLite-backed `Harness`, constructs a `FileCache`, applies events and
//! asserts cache state — covering all five `StorageEvent` variants plus
//! the ancestor-missing error path.

mod support;

use crabcloud_filecache::{FileCache, FileCacheError};
use crabcloud_storage::{StorageEvent, StoragePath};
use support::{harness, make_dir_metadata, make_metadata};

// Anchors for crates referenced only by other targets in this crate. Without
// these, `-D warnings` on `cargo clippy --all-targets` flags the integration
// test as missing dependencies that the library + the `tests/support` module
// pull in transitively (`hex` + `md5` for path_hash, `sqlx` for the pool,
// etc.).
use async_trait as _;
use crabcloud_cache as _;
use crabcloud_config as _;
use dashmap as _;
use hex as _;
use md5 as _;
use sqlx as _;
use thiserror as _;
use tokio as _;
use tracing as _;

const SID: &str = "local::/test";

#[tokio::test]
async fn apply_written_event_inserts_leaf_with_metadata() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    // Seed the root directory so a leaf at `hello.txt` has a resolvable parent.
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();

    let md = make_metadata("hello.txt", 5, "text/plain");
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("hello.txt").unwrap(),
            metadata: md.clone(),
        })
        .await
        .unwrap();

    let row = cache
        .lookup(SID, &StoragePath::new("hello.txt").unwrap())
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.name, "hello.txt");
    assert_eq!(row.size, 5);
    assert_eq!(row.mimetype.as_str(), "text/plain");
    assert_eq!(row.etag, md.etag);
}

#[tokio::test]
async fn apply_propagates_size_and_etag_up_chain() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("a").unwrap(),
            metadata: make_dir_metadata("a"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("a/b").unwrap(),
            metadata: make_dir_metadata("a/b"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("a/b/c").unwrap(),
            metadata: make_dir_metadata("a/b/c"),
        })
        .await
        .unwrap();

    let root_before = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();
    let a_before = cache
        .lookup(SID, &StoragePath::new("a").unwrap())
        .await
        .unwrap()
        .unwrap();
    let ab_before = cache
        .lookup(SID, &StoragePath::new("a/b").unwrap())
        .await
        .unwrap()
        .unwrap();
    let abc_before = cache
        .lookup(SID, &StoragePath::new("a/b/c").unwrap())
        .await
        .unwrap()
        .unwrap();

    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("a/b/c/file.txt").unwrap(),
            metadata: make_metadata("a/b/c/file.txt", 100, "text/plain"),
        })
        .await
        .unwrap();

    let root_after = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();
    let a_after = cache
        .lookup(SID, &StoragePath::new("a").unwrap())
        .await
        .unwrap()
        .unwrap();
    let ab_after = cache
        .lookup(SID, &StoragePath::new("a/b").unwrap())
        .await
        .unwrap()
        .unwrap();
    let abc_after = cache
        .lookup(SID, &StoragePath::new("a/b/c").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(root_after.size, root_before.size + 100);
    assert_eq!(a_after.size, a_before.size + 100);
    assert_eq!(ab_after.size, ab_before.size + 100);
    assert_eq!(abc_after.size, abc_before.size + 100);

    assert_ne!(root_after.etag, root_before.etag);
    assert_ne!(a_after.etag, a_before.etag);
    assert_ne!(ab_after.etag, ab_before.etag);
    assert_ne!(abc_after.etag, abc_before.etag);
}

#[tokio::test]
async fn apply_dir_created_inserts_directory_with_zero_size() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("d").unwrap(),
            metadata: make_dir_metadata("d"),
        })
        .await
        .unwrap();

    let row = cache
        .lookup(SID, &StoragePath::new("d").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.size, 0);
    assert_eq!(row.mimetype.as_str(), "httpd/unix-directory");
}

#[tokio::test]
async fn apply_deleted_cascades_descendants_and_decrements_size() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("d").unwrap(),
            metadata: make_dir_metadata("d"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("d/inner.txt").unwrap(),
            metadata: make_metadata("d/inner.txt", 50, "text/plain"),
        })
        .await
        .unwrap();

    let root_pre = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();

    cache
        .apply(&StorageEvent::Deleted {
            storage_id: SID.into(),
            path: StoragePath::new("d").unwrap(),
        })
        .await
        .unwrap();

    assert!(cache
        .lookup(SID, &StoragePath::new("d").unwrap())
        .await
        .unwrap()
        .is_none());
    assert!(cache
        .lookup(SID, &StoragePath::new("d/inner.txt").unwrap())
        .await
        .unwrap()
        .is_none());

    let root_post = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();
    // Root size dropped by `d`'s size (which was 50 after the inner write).
    assert_eq!(root_post.size, root_pre.size - 50);
}

#[tokio::test]
async fn apply_moved_within_same_parent_bumps_etag_only() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("from.txt").unwrap(),
            metadata: make_metadata("from.txt", 10, "text/plain"),
        })
        .await
        .unwrap();

    let root_pre = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();

    cache
        .apply(&StorageEvent::Moved {
            storage_id: SID.into(),
            from: StoragePath::new("from.txt").unwrap(),
            to: StoragePath::new("to.txt").unwrap(),
        })
        .await
        .unwrap();

    assert!(cache
        .lookup(SID, &StoragePath::new("from.txt").unwrap())
        .await
        .unwrap()
        .is_none());
    let to_row = cache
        .lookup(SID, &StoragePath::new("to.txt").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(to_row.size, 10);

    let root_post = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root_post.size, root_pre.size); // same parent → no size change
    assert_ne!(root_post.etag, root_pre.etag); // but etag bumped
}

#[tokio::test]
async fn apply_moved_across_parents_shifts_size_and_bumps_both_etags() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("a").unwrap(),
            metadata: make_dir_metadata("a"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("b").unwrap(),
            metadata: make_dir_metadata("b"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("a/file.txt").unwrap(),
            metadata: make_metadata("a/file.txt", 25, "text/plain"),
        })
        .await
        .unwrap();

    let a_pre = cache
        .lookup(SID, &StoragePath::new("a").unwrap())
        .await
        .unwrap()
        .unwrap();
    let b_pre = cache
        .lookup(SID, &StoragePath::new("b").unwrap())
        .await
        .unwrap()
        .unwrap();

    cache
        .apply(&StorageEvent::Moved {
            storage_id: SID.into(),
            from: StoragePath::new("a/file.txt").unwrap(),
            to: StoragePath::new("b/file.txt").unwrap(),
        })
        .await
        .unwrap();

    let a_post = cache
        .lookup(SID, &StoragePath::new("a").unwrap())
        .await
        .unwrap()
        .unwrap();
    let b_post = cache
        .lookup(SID, &StoragePath::new("b").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a_post.size, a_pre.size - 25);
    assert_eq!(b_post.size, b_pre.size + 25);
    assert_ne!(a_post.etag, a_pre.etag);
    assert_ne!(b_post.etag, b_pre.etag);
}

#[tokio::test]
async fn apply_moved_directory_rewrites_descendant_paths() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::new("a").unwrap(),
            metadata: make_dir_metadata("a"),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("a/inner.txt").unwrap(),
            metadata: make_metadata("a/inner.txt", 7, "text/plain"),
        })
        .await
        .unwrap();

    // Move directory "a" -> "b". Descendant "a/inner.txt" should become
    // "b/inner.txt".
    cache
        .apply(&StorageEvent::Moved {
            storage_id: SID.into(),
            from: StoragePath::new("a").unwrap(),
            to: StoragePath::new("b").unwrap(),
        })
        .await
        .unwrap();

    assert!(cache
        .lookup(SID, &StoragePath::new("a/inner.txt").unwrap())
        .await
        .unwrap()
        .is_none());
    let inner = cache
        .lookup(SID, &StoragePath::new("b/inner.txt").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inner.name, "inner.txt");
    assert_eq!(inner.size, 7);
}

#[tokio::test]
async fn apply_copied_inserts_dest_with_fresh_etag() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("src.txt").unwrap(),
            metadata: make_metadata("src.txt", 12, "text/plain"),
        })
        .await
        .unwrap();

    let src_pre = cache
        .lookup(SID, &StoragePath::new("src.txt").unwrap())
        .await
        .unwrap()
        .unwrap();
    let root_pre = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();

    cache
        .apply(&StorageEvent::Copied {
            storage_id: SID.into(),
            from: StoragePath::new("src.txt").unwrap(),
            to: StoragePath::new("dst.txt").unwrap(),
        })
        .await
        .unwrap();

    let dst = cache
        .lookup(SID, &StoragePath::new("dst.txt").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dst.size, 12);
    assert_ne!(dst.etag, src_pre.etag);

    let root_post = cache
        .lookup(SID, &StoragePath::root())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root_post.size, root_pre.size + 12);
    assert_ne!(root_post.etag, root_pre.etag);
}

#[tokio::test]
async fn apply_copied_preserves_source_permissions() {
    use crabcloud_storage::Permissions;
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: make_dir_metadata(""),
        })
        .await
        .unwrap();

    // Build source metadata with non-default permissions (read+update only).
    let mut src_md = make_metadata("src.txt", 12, "text/plain");
    src_md.permissions = Permissions::new(Permissions::READ | Permissions::UPDATE);
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("src.txt").unwrap(),
            metadata: src_md,
        })
        .await
        .unwrap();

    cache
        .apply(&StorageEvent::Copied {
            storage_id: SID.into(),
            from: StoragePath::new("src.txt").unwrap(),
            to: StoragePath::new("dst.txt").unwrap(),
        })
        .await
        .unwrap();

    let dst = cache
        .lookup(SID, &StoragePath::new("dst.txt").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        dst.permissions.bits(),
        Permissions::READ | Permissions::UPDATE,
        "copy must inherit source permissions"
    );
}

#[tokio::test]
async fn apply_missing_ancestor_errors() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    // No root row + no "a" row — direct write to "a/file" must fail.
    let res = cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("a/file.txt").unwrap(),
            metadata: make_metadata("a/file.txt", 5, "text/plain"),
        })
        .await;
    assert!(matches!(res, Err(FileCacheError::AncestorMissing(_))));
}
