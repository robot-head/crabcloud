//! Shared test fixtures for `crabcloud-fs` integration tests.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::FileCache;
use crabcloud_fs::{Mount, View};
use crabcloud_storage::{memory::MemoryStorage, ChannelEventSink, Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;
use tempfile::TempDir;

pub struct Harness {
    pub pool: DbPool,
    pub filecache: Arc<FileCache>,
    pub sink: Arc<ChannelEventSink>,
    pub storage: Arc<dyn Storage>,
    pub _tempdir: TempDir,
}

pub async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("h.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let filecache = Arc::new(FileCache::new(pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    Harness {
        pool,
        filecache,
        sink,
        storage,
        _tempdir: dir,
    }
}

/// Build a single-home-mount `View` against the harness's storage.
pub fn view_home(h: &Harness) -> View {
    View::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: h.storage.clone(),
            metadata: None,
        }],
        h.filecache.clone(),
        h.sink.clone(),
    )
}

/// Build a 2-mount view: home at `/` + a synthetic mount at `/Shared`.
/// Used to exercise the cross-mount error path in Batch C tests.
pub fn view_with_two_mounts(h: &Harness) -> View {
    let shared: Arc<dyn Storage> = Arc::new(MemoryStorage::new("shared"));
    View::new(
        UserId::new("alice").unwrap(),
        vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: h.storage.clone(),
                metadata: None,
            },
            Mount {
                path_prefix: StoragePath::new("Shared").unwrap(),
                storage: shared,
                metadata: None,
            },
        ],
        h.filecache.clone(),
        h.sink.clone(),
    )
}
