mod support;

use crabcloud_fs::{FsError, UserPath};
use support::{harness, view_home, view_with_two_mounts};
use tokio::io::AsyncReadExt;

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

#[tokio::test]
async fn view_rename_within_mount_moves_file() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(&UserPath::new("/from.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();

    view.rename(
        &UserPath::new("/from.txt").unwrap(),
        &UserPath::new("/to.txt").unwrap(),
    )
    .await
    .unwrap();

    let from_stat = view.stat(&UserPath::new("/from.txt").unwrap()).await;
    assert!(matches!(
        from_stat,
        Err(FsError::FileCache(
            crabcloud_filecache::FileCacheError::NotFound
        ))
    ));

    let mut reader = view.read(&UserPath::new("/to.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"x");
}

#[tokio::test]
async fn view_copy_within_mount_preserves_source_and_creates_dest() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(
        &UserPath::new("/src.txt").unwrap(),
        body(b"copy-me".to_vec()),
    )
    .await
    .unwrap();
    let src_meta = view
        .stat(&UserPath::new("/src.txt").unwrap())
        .await
        .unwrap();

    view.copy(
        &UserPath::new("/src.txt").unwrap(),
        &UserPath::new("/dst.txt").unwrap(),
    )
    .await
    .unwrap();

    // Source still exists.
    let mut reader = view
        .read(&UserPath::new("/src.txt").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"copy-me");

    // Dest has the same contents but a fresh ETag.
    let mut reader = view
        .read(&UserPath::new("/dst.txt").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"copy-me");

    let dst_meta = view
        .stat(&UserPath::new("/dst.txt").unwrap())
        .await
        .unwrap();
    assert_ne!(src_meta.etag, dst_meta.etag);
}

#[tokio::test]
async fn view_rename_cross_mount_errors() {
    let h = harness().await;
    let view = view_with_two_mounts(&h);
    // Set up a file in the home mount.
    view.put_file(&UserPath::new("/from.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    // /Shared is a different mount. Attempting to rename across should fail.
    let r = view
        .rename(
            &UserPath::new("/from.txt").unwrap(),
            &UserPath::new("/Shared/to.txt").unwrap(),
        )
        .await;
    assert!(matches!(r, Err(FsError::CrossMount)));
}

#[tokio::test]
async fn view_copy_cross_mount_errors() {
    let h = harness().await;
    let view = view_with_two_mounts(&h);
    view.put_file(&UserPath::new("/src.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    let r = view
        .copy(
            &UserPath::new("/src.txt").unwrap(),
            &UserPath::new("/Shared/dst.txt").unwrap(),
        )
        .await;
    assert!(matches!(r, Err(FsError::CrossMount)));
}
