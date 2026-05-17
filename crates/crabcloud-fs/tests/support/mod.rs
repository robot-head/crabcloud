//! Shared test fixtures for `crabcloud-fs` integration tests.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::FileCache;
use crabcloud_fs::{Mount, MountKind, MountMetadata, SharedSubrootStorage, VersionsHooks, View};
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{memory::MemoryStorage, ChannelEventSink, Storage, StoragePath};
use crabcloud_trash::Trash;
use crabcloud_users::UserId;
use crabcloud_versions::Versions;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

pub struct Harness {
    pub pool: DbPool,
    pub filecache: Arc<FileCache>,
    pub sink: Arc<ChannelEventSink>,
    pub storage: Arc<dyn Storage>,
    pub trash: Arc<Trash>,
    pub versions: Arc<Versions>,
    pub datadir: PathBuf,
    pub _tempdir: TempDir,
}

impl Harness {
    pub fn versions_hooks(&self) -> VersionsHooks {
        VersionsHooks::permissive(self.versions.clone())
    }
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
    let datadir = dir.path().to_path_buf();
    let pool_arc = Arc::new(pool.clone());
    let versions = Arc::new(Versions::new(pool_arc.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    let trash = Arc::new(Trash::new(pool_arc, datadir.clone(), versions.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    Harness {
        pool,
        filecache,
        sink,
        storage,
        trash,
        versions,
        datadir,
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
        h.trash.clone(),
        h.versions_hooks(),
    )
}

/// Build a View for any user with the given storage at root. Used when
/// tests need to act as a non-default user (e.g., alice while bob has a
/// share mount on the same harness).
pub fn view_home_for(h: &Harness, storage: Arc<dyn Storage>) -> View {
    View::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        }],
        h.filecache.clone(),
        h.sink.clone(),
        h.trash.clone(),
        h.versions_hooks(),
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
        h.trash.clone(),
        h.versions_hooks(),
    )
}

/// Build a View for bob with bob's home plus one share-mount whose
/// owner is alice (her storage pinned at `owner_subroot`, surfaced at
/// `mount_name` in bob's view). Used by the share-enumeration tests.
pub fn view_with_share_mount(
    h: &Harness,
    bob_home: Arc<dyn Storage>,
    alice_home: Arc<dyn Storage>,
    owner_subroot: &str,
    mount_name: &str,
) -> View {
    let wrapped: Arc<dyn Storage> = Arc::new(SharedSubrootStorage::new(
        alice_home,
        StoragePath::new(owner_subroot).unwrap(),
        SharePermissions::from_wire(1 | 2),
    ));
    View::new(
        UserId::new("bob").unwrap(),
        vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: bob_home,
                metadata: None,
            },
            Mount {
                path_prefix: StoragePath::new(mount_name).unwrap(),
                storage: wrapped,
                metadata: Some(MountMetadata {
                    kind: MountKind::Share,
                    owner_uid: Some("alice".to_string()),
                    permissions: Some(SharePermissions::from_wire(1 | 2)),
                }),
            },
        ],
        h.filecache.clone(),
        h.sink.clone(),
        h.trash.clone(),
        h.versions_hooks(),
    )
}
