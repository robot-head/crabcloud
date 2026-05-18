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
use crabcloud_activity as _;
use serde_json as _;
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
async fn appstate_view_for_emits_activity_to_state_activity() {
    // SP14 wiring: confirm that a put_file via state.view_for(uid) ends up
    // emitting a row through state.activity (i.e. the View is constructed
    // with the live emitter, not NoopEmitter).
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let view = state.view_for(&uid).await.unwrap();
    view.put_file(
        &UserPath::new("/wired.txt").unwrap(),
        body(b"wired".to_vec()),
    )
    .await
    .unwrap();

    let rows = state.activity.list(uid.as_str(), None, 10).await.unwrap();
    assert!(
        rows.iter().any(|r| r.event_type == "file_created"),
        "expected a file_created row in state.activity after put_file, got: {:?}",
        rows.iter()
            .map(|r| r.event_type.as_str())
            .collect::<Vec<_>>(),
    );
}

/// Poll `state.search.query(uid, "<text>")` until a hit appears or
/// the deadline expires. Deadline is generous (15s) so a heavily
/// loaded workspace-parallel test run doesn't time out behind a
/// scheduling spike — the indexer is event-driven, normal latency is
/// sub-100ms.
async fn wait_for_index<F>(predicate: F)
where
    F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if predicate().await {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("index never reached expected state within 15s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appstate_write_eventually_indexed_then_queryable() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    cfg.search_indexer_enabled = true;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let view = state.view_for(&uid).await.unwrap();
    view.put_file(
        &UserPath::new("/report.docx").unwrap(),
        body(b"contents".to_vec()),
    )
    .await
    .unwrap();

    let search = state.search.clone();
    wait_for_index(move || {
        let search = search.clone();
        Box::pin(async move {
            let hits = search
                .query("alice", &crabcloud_search::parse_query("report"), 10, None)
                .await
                .unwrap_or_default();
            !hits.is_empty()
        })
    })
    .await;

    let hits = state
        .search
        .query("alice", &crabcloud_search::parse_query("report"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].basename, "report.docx");
    assert_eq!(hits[0].path, "/report.docx");

    state.search_indexer_shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appstate_delete_eventually_removes_from_index() {
    let db_dir = tempdir().unwrap();
    let data_dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(db_dir.path().join("state.db"));
    cfg.datadirectory = data_dir.path().to_path_buf();
    cfg.search_indexer_enabled = true;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let uid = UserId::new("alice").unwrap();
    let view = state.view_for(&uid).await.unwrap();
    view.put_file(
        &UserPath::new("/deleteme.txt").unwrap(),
        body(b"x".to_vec()),
    )
    .await
    .unwrap();

    let search = state.search.clone();
    wait_for_index(move || {
        let search = search.clone();
        Box::pin(async move {
            !search
                .query(
                    "alice",
                    &crabcloud_search::parse_query("deleteme"),
                    10,
                    None,
                )
                .await
                .unwrap_or_default()
                .is_empty()
        })
    })
    .await;

    view.delete(&UserPath::new("/deleteme.txt").unwrap())
        .await
        .unwrap();

    let search = state.search.clone();
    wait_for_index(move || {
        let search = search.clone();
        Box::pin(async move {
            search
                .query(
                    "alice",
                    &crabcloud_search::parse_query("deleteme"),
                    10,
                    None,
                )
                .await
                .unwrap_or_default()
                .is_empty()
        })
    })
    .await;

    state.search_indexer_shutdown.notify_one();
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
