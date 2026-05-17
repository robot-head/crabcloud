//! Focused regression for SP12 Batch A: `View::delete` reroutes through
//! `Trash::soft_delete` (file moves to `<datadir>/<uid>/files_trashbin/`
//! and a trash row is written) while `View::hard_delete` keeps the
//! pre-SP12 destructive semantics (file is removed, no trash row).
//!
//! Uses a real on-disk `LocalStorage` rooted under a tempdir because the
//! trash service moves bytes under `<datadir>/<uid>/files_trashbin/...`;
//! `MemoryStorage` has no on-disk presence and would error
//! `SourceMissing`.

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::FileCache;
use crabcloud_fs::{LocalStorageFactory, Mount, StorageFactory, UserPath, View};
use crabcloud_storage::{ChannelEventSink, StoragePath};
use crabcloud_trash::Trash;
use crabcloud_users::UserId;
use std::sync::Arc;
use tempfile::TempDir;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_core as _;
use crabcloud_sharing as _;
use thiserror as _;
use tracing as _;

struct LocalHarness {
    view: View,
    datadir: std::path::PathBuf,
    uid: UserId,
    _tempdir: TempDir,
}

async fn local_harness(uid: &str) -> LocalHarness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("trash.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let filecache = Arc::new(FileCache::new(pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let datadir = dir.path().to_path_buf();
    let factory = LocalStorageFactory::new(datadir.clone());
    let uid = UserId::new(uid).unwrap();
    let storage = factory.home_storage(&uid).await.unwrap();
    let trash = Arc::new(Trash::new(Arc::new(pool), datadir.clone()));
    let view = View::new(
        uid.clone(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        }],
        filecache,
        sink,
        trash.clone(),
    );
    LocalHarness {
        view,
        datadir,
        uid,
        _tempdir: dir,
    }
}

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn view_delete_creates_trash_row_and_keeps_bytes_in_trashbin() {
    let h = local_harness("alice").await;
    h.view
        .mkdir(&UserPath::new("/notes").unwrap())
        .await
        .unwrap();
    h.view
        .put_file(
            &UserPath::new("/notes/x.txt").unwrap(),
            body(b"hello".to_vec()),
        )
        .await
        .unwrap();

    h.view
        .delete(&UserPath::new("/notes/x.txt").unwrap())
        .await
        .unwrap();

    // Original gone from `<datadir>/<uid>/files/notes/x.txt`.
    assert!(!h
        .datadir
        .join(h.uid.as_str())
        .join("files/notes/x.txt")
        .exists());

    // Bytes now live under `<datadir>/<uid>/files_trashbin/files/x.txt.dN`.
    let trashbin = h
        .datadir
        .join(h.uid.as_str())
        .join("files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&trashbin)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().into_string().unwrap();
    assert!(name.starts_with("x.txt.d"), "got {name}");
}

#[tokio::test]
async fn view_hard_delete_does_not_create_trash_row_or_trashbin_dir() {
    let h = local_harness("bob").await;
    h.view
        .put_file(
            &UserPath::new("/single.txt").unwrap(),
            body(b"z".to_vec()),
        )
        .await
        .unwrap();

    h.view
        .hard_delete(&UserPath::new("/single.txt").unwrap())
        .await
        .unwrap();

    // Original gone.
    assert!(!h
        .datadir
        .join(h.uid.as_str())
        .join("files/single.txt")
        .exists());
    // Trashbin directory was never created.
    assert!(!h
        .datadir
        .join(h.uid.as_str())
        .join("files_trashbin")
        .exists());
}
