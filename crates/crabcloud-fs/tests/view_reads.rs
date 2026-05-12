mod support;

use crabcloud_fs::{FsError, UserPath};
use crabcloud_storage::{FileKind, NoopEventSink, StoragePath};
use support::{harness, view_home};
use tokio::io::AsyncReadExt;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use thiserror as _;
use tracing as _;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn view_stat_returns_metadata_for_existing_file() {
    let h = harness().await;
    // Seed via the storage directly (NoopEventSink — the View's stat goes
    // through cache populate on miss, hitting the backend stat).
    h.storage
        .put_file(
            &StoragePath::new("hello.txt").unwrap(),
            body(b"hi".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let view = view_home(&h);
    let meta = view
        .stat(&UserPath::new("/hello.txt").unwrap())
        .await
        .unwrap();
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.size, 2);
}

#[tokio::test]
async fn view_put_then_read_roundtrip() {
    let h = harness().await;
    let view = view_home(&h);

    let meta = view
        .put_file(&UserPath::new("/hi.txt").unwrap(), body(b"hello".to_vec()))
        .await
        .unwrap();
    assert_eq!(meta.size, 5);
    let fresh_etag = meta.etag.clone();

    // Read back via the View.
    let mut reader = view.read(&UserPath::new("/hi.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hello");

    // The fresh etag is returned by put_file directly (no scanner lag).
    assert_eq!(fresh_etag.as_str().len(), 40);
}

#[tokio::test]
async fn view_list_returns_children() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/d").unwrap()).await.unwrap();
    view.put_file(&UserPath::new("/d/a.txt").unwrap(), body(b"a".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/b.txt").unwrap(), body(b"b".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/c.txt").unwrap(), body(b"c".to_vec()))
        .await
        .unwrap();

    let entries = view.list(&UserPath::new("/d").unwrap()).await.unwrap();
    assert_eq!(entries.len(), 3);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"c.txt"));
}

#[tokio::test]
async fn view_mkdir_creates_directory() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/newdir").unwrap())
        .await
        .unwrap();
    let meta = view.stat(&UserPath::new("/newdir").unwrap()).await.unwrap();
    assert_eq!(meta.kind, FileKind::Directory);
}

#[tokio::test]
async fn view_delete_removes_file() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(&UserPath::new("/del.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    view.delete(&UserPath::new("/del.txt").unwrap())
        .await
        .unwrap();
    let r = view.stat(&UserPath::new("/del.txt").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(
            crabcloud_filecache::FileCacheError::NotFound
        ))
    ));
}

#[tokio::test]
async fn view_delete_removes_empty_directory() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/empty").unwrap()).await.unwrap();
    view.delete(&UserPath::new("/empty").unwrap())
        .await
        .unwrap();
    let r = view.stat(&UserPath::new("/empty").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(
            crabcloud_filecache::FileCacheError::NotFound
        ))
    ));
}

#[tokio::test]
async fn view_invalid_user_path_rejected() {
    // No leading slash.
    assert!(matches!(
        UserPath::new("photos/cat.jpg"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_path_escape_via_dotdot_rejected() {
    // Path escape via .. is caught at UserPath construction.
    assert!(matches!(
        UserPath::new("/photos/../../etc/passwd"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_read_range_returns_slice() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(
        &UserPath::new("/range.txt").unwrap(),
        body(b"abcdefghij".to_vec()),
    )
    .await
    .unwrap();
    let mut reader = view
        .read_range(&UserPath::new("/range.txt").unwrap(), 2..5)
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"cde");
}
