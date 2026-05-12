//! Local-FS-specific tests: atomic durability, xattr persistence, path escape.

// Tests rely on Unix xattr APIs + symlinks. The whole binary is empty on
// non-Unix; suppress the dev-dep "unused_crate_dependencies" lint there.
#![cfg_attr(not(unix), allow(unused_crate_dependencies))]
#![cfg(unix)]

mod support;

// Anchor workspace-level dev/transitive deps so `-D warnings` +
// `unused_crate_dependencies` stays quiet for this integration-test binary.
use bytes as _;
use hex as _;
use infer as _;
use phf as _;
use rand as _;
use serde as _;
use sha2 as _;
use thiserror as _;
use tracing as _;

use crabcloud_storage::local::LocalStorage;
use crabcloud_storage::{NoopEventSink, Storage, StoragePath};
use tempfile::tempdir;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn etag_persists_across_reload() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("p.txt").unwrap();
    storage
        .put_file(&path, body(b"hello".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let first = storage.stat(&path).await.unwrap();

    let reloaded = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let second = reloaded.stat(&path).await.unwrap();
    assert_eq!(first.etag, second.etag);
}

#[tokio::test]
async fn mimetype_persists_across_reload() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("hello.txt").unwrap();
    storage
        .put_file(&path, body(b"hi".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let reloaded = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        reloaded.stat(&path).await.unwrap().mimetype.as_str(),
        "text/plain"
    );
}

#[tokio::test]
async fn xattr_stripped_falls_back_to_mtime_inode_etag() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("fallback.txt").unwrap();
    storage
        .put_file(&path, body(b"hi".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let real_path = dir.path().join("fallback.txt");
    let _ = xattr::remove(&real_path, "user.crabcloud.etag");
    // After xattr strip, ETag should be deterministic-from-mtime/inode and
    // non-empty. Two stats should agree.
    let a = storage.stat(&path).await.unwrap();
    let b = storage.stat(&path).await.unwrap();
    assert_eq!(a.etag, b.etag);
    assert_eq!(a.etag.as_str().len(), 40);
}

#[tokio::test]
async fn atomic_write_temp_cleaned_on_drop() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("clean.txt").unwrap();
    // Successful write — should leave no .tmp-crabcloud-* siblings.
    storage
        .put_file(&path, body(b"x".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let mut leftover = false;
    let mut rd = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = rd.next_entry().await.unwrap() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-crabcloud-")
        {
            leftover = true;
        }
    }
    assert!(!leftover, "found leftover .tmp-crabcloud-* file");
}

#[tokio::test]
async fn path_escape_via_canonicalize_rejected() {
    let outer = tempdir().unwrap();
    let inner = outer.path().join("inner");
    tokio::fs::create_dir(&inner).await.unwrap();
    let storage = LocalStorage::new(inner.clone()).unwrap();

    // Create a real escape target outside `inner` and a symlink inside that
    // points to it. resolve() canonicalize + starts_with(root) check rejects.
    let target_outside = outer.path().join("OUTSIDE");
    tokio::fs::write(&target_outside, b"secret").await.unwrap();
    let link_in = inner.join("escape");
    std::os::unix::fs::symlink(&target_outside, &link_in).unwrap();

    let res = storage.stat(&StoragePath::new("escape").unwrap()).await;
    assert!(
        matches!(res, Err(crabcloud_storage::StorageError::InvalidPath(_))),
        "expected InvalidPath, got {:?}",
        res
    );
}

#[tokio::test]
async fn multipart_abort_drops_upload_dir() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let target = StoragePath::new("aborted.bin").unwrap();
    let handle = storage
        .begin_multipart(&target, &NoopEventSink)
        .await
        .unwrap();
    storage
        .put_part(&handle, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let upload_id = handle.upload_id.clone();
    storage.abort_multipart(handle).await.unwrap();

    // Upload tempdir should be gone.
    let mut found = false;
    let mut rd = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = rd.next_entry().await.unwrap() {
        if entry.file_name().to_string_lossy().contains(&upload_id) {
            found = true;
        }
    }
    assert!(!found, "upload tempdir not cleaned up");
}

#[tokio::test]
async fn multipart_corrupted_part_rejected_at_commit() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let target = StoragePath::new("corrupt.bin").unwrap();
    let handle = storage
        .begin_multipart(&target, &NoopEventSink)
        .await
        .unwrap();
    let t1 = storage
        .put_part(&handle, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    // Tamper with the part file directly.
    let parent = dir.path().to_path_buf();
    let temp_dir = parent.join(format!(".upload-{}", handle.upload_id));
    let part_file = temp_dir.join("part-00000001");
    tokio::fs::write(&part_file, b"BBB").await.unwrap();

    let err = storage
        .commit_multipart(handle, vec![t1], &NoopEventSink)
        .await
        .unwrap_err();
    assert!(matches!(err, crabcloud_storage::StorageError::Multipart(_)));
}
