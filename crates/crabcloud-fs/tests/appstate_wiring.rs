//! Integration tests for `AppState::view_for` / `uploads_for`. Verifies the
//! resolver is wired correctly and that two calls for the same uid return
//! views over the same mount.

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use tempfile::tempdir;
use tokio::io::AsyncReadExt;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_db as _;
use crabcloud_filecache as _;
use crabcloud_sharing as _;
use crabcloud_storage as _;
use crabcloud_trash as _;
use crabcloud_versions as _;
use thiserror as _;
use tracing as _;

use chrono as _;
use serde_json as _;
use crabcloud_activity as _;
fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn appstate_view_for_round_trip_through_local_storage() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let view = state.view_for(&uid).await.unwrap();

    // Write a file via the View.
    let meta = view
        .put_file(&UserPath::new("/hello.txt").unwrap(), body(b"hi".to_vec()))
        .await
        .unwrap();
    assert_eq!(meta.size, 2);

    // Read it back via a fresh view_for (different request).
    let view2 = state.view_for(&uid).await.unwrap();
    let mut reader = view2
        .read(&UserPath::new("/hello.txt").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hi");
}

#[tokio::test]
async fn appstate_view_for_distinct_users_get_distinct_storages() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let alice = state
        .view_for(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let bob = state.view_for(&UserId::new("bob").unwrap()).await.unwrap();

    alice
        .put_file(&UserPath::new("/a.txt").unwrap(), body(b"alice".to_vec()))
        .await
        .unwrap();
    bob.put_file(&UserPath::new("/a.txt").unwrap(), body(b"bob".to_vec()))
        .await
        .unwrap();

    // Each user's /a.txt is independent.
    let mut reader = alice.read(&UserPath::new("/a.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"alice");

    let mut reader = bob.read(&UserPath::new("/a.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"bob");
}

#[tokio::test]
async fn appstate_uploads_for_round_trip() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let uploads = state.uploads_for(&uid).await.unwrap();
    let dest = UserPath::new("/upload.bin").unwrap();

    let handle = uploads.begin(&dest).await.unwrap();
    let t1 = uploads
        .put_part(&handle.upload_id, 1, body(b"DATA".to_vec()))
        .await
        .unwrap();
    let meta = uploads
        .commit(&handle.upload_id, &dest, vec![t1])
        .await
        .unwrap();
    assert_eq!(meta.size, 4);
}
