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
use crabcloud_fs::{
    LocalStorageFactory, Mount, MountKind, MountMetadata, SharedSubrootStorage, StorageFactory,
    UserPath, VersionsHooks, View,
};
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{ChannelEventSink, NoopEventSink, Storage, StorageError, StoragePath};
use crabcloud_trash::Trash;
use crabcloud_users::UserId;
use crabcloud_versions::Versions;
use std::sync::Arc;
use tempfile::TempDir;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_core as _;
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
    let pool_arc = Arc::new(pool);
    let versions = Arc::new(Versions::new(pool_arc.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    let trash = Arc::new(Trash::new(pool_arc, datadir.clone(), versions.clone()));
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
        VersionsHooks::permissive(versions),
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
    let trashbin = h.datadir.join(h.uid.as_str()).join("files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&trashbin)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().into_string().unwrap();
    assert!(name.starts_with("x.txt.d"), "got {name}");
}

/// Two-user harness: alice's home backs a folder that bob has mounted
/// as a share at `/Shared/Photos`. Both homes share the same datadir +
/// trash service (so the trash row written under bob is visible from
/// the same `Trash` handle the test inspects).
struct ShareHarness {
    bob_view: View,
    alice_home: Arc<dyn Storage>,
    trash: Arc<Trash>,
    datadir: std::path::PathBuf,
    _tempdir: TempDir,
}

async fn share_harness(perms_wire: u32) -> ShareHarness {
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
    let pool_arc = Arc::new(pool);
    let versions = Arc::new(Versions::new(pool_arc.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    let trash = Arc::new(Trash::new(pool_arc, datadir.clone(), versions.clone()));

    let bob_uid = UserId::new("bob").unwrap();
    let alice_uid = UserId::new("alice").unwrap();
    let bob_home = factory.home_storage(&bob_uid).await.unwrap();
    let alice_home = factory.home_storage(&alice_uid).await.unwrap();

    // Wrap alice's home behind a share-mount with the given permissions.
    let perms = SharePermissions::from_wire(perms_wire);
    let share_owner_path = StoragePath::new("Photos").unwrap();
    let wrapped: Arc<dyn Storage> = Arc::new(SharedSubrootStorage::new(
        alice_home.clone(),
        share_owner_path,
        perms,
    ));

    let bob_view = View::new(
        bob_uid,
        vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: bob_home,
                metadata: None,
            },
            Mount {
                path_prefix: StoragePath::new("Shared/Photos").unwrap(),
                storage: wrapped,
                metadata: Some(MountMetadata {
                    kind: MountKind::Share,
                    owner_uid: Some("alice".to_string()),
                    permissions: Some(perms),
                }),
            },
        ],
        filecache,
        sink,
        trash.clone(),
        VersionsHooks::permissive(versions),
    );

    ShareHarness {
        bob_view,
        alice_home,
        trash,
        datadir,
        _tempdir: dir,
    }
}

#[tokio::test]
async fn view_delete_on_share_mount_lands_in_deleters_trashbin() {
    // Alice shares /Photos with bob (with delete permission). Bob deletes
    // /Shared/Photos/cat.jpg from his view. Per spec §2 decision #7, the
    // trash row's `user` must be `bob`, the bytes must land under
    // `<datadir>/bob/files_trashbin/files/...`, and alice's source file
    // must be gone.
    let h = share_harness(1 | 2 | 4 | 8).await; // read+update+create+delete
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/cat.jpg").unwrap(),
            Box::pin(std::io::Cursor::new(b"meow".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    h.bob_view
        .delete(&UserPath::new("/Shared/Photos/cat.jpg").unwrap())
        .await
        .expect("bob's delete should soft-delete into his bin");

    // Alice's source bytes are gone.
    assert!(!h
        .datadir
        .join("alice")
        .join("files/Photos/cat.jpg")
        .exists());

    // Bob's bin has one entry — `cat.jpg`, with `location` reflecting
    // bob's view of the path.
    let rows = h.trash.list("bob").await.unwrap();
    assert_eq!(rows.len(), 1, "exactly one trash row under bob");
    let row = &rows[0];
    assert_eq!(row.user, "bob");
    assert_eq!(row.basename, "cat.jpg");
    assert_eq!(row.location, "/Shared/Photos");
    assert!(h.trash.list("alice").await.unwrap().is_empty());

    // Bytes are under bob's trashbin.
    let bob_trashbin = h.datadir.join("bob").join("files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_trashbin)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().into_string().unwrap();
    assert!(name.starts_with("cat.jpg.d"), "got {name}");
}

#[tokio::test]
async fn view_delete_on_readonly_share_mount_returns_403_no_trash_row() {
    // Read-only share (no delete bit). Bob's delete must surface the
    // storage backend's PermissionDenied as Forbidden; no bytes leave
    // alice's storage; no trash row is created (the rollback path in
    // `delete_via_share_mount`).
    let h = share_harness(1).await; // read only
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/locked.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"x".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let r = h
        .bob_view
        .delete(&UserPath::new("/Shared/Photos/locked.txt").unwrap())
        .await;
    assert!(
        matches!(
            r,
            Err(crabcloud_fs::FsError::Storage(
                StorageError::PermissionDenied
            ))
        ),
        "expected PermissionDenied, got {r:?}"
    );

    // Source still present.
    assert!(h
        .datadir
        .join("alice")
        .join("files/Photos/locked.txt")
        .exists());
    // No trash row under either user.
    assert!(h.trash.list("bob").await.unwrap().is_empty());
    assert!(h.trash.list("alice").await.unwrap().is_empty());
}

#[tokio::test]
async fn view_delete_on_share_mount_directory_recursive_lands_in_deleters_trashbin() {
    // Alice shares /Photos with bob (with delete). Inside it lives a
    // directory D with two files + a subdir + a nested file. Bob
    // deletes /Shared/Photos/D from his view. The full subtree must
    // land in bob's trashbin under a single Dir row, and alice's
    // source tree must be gone.
    let h = share_harness(1 | 2 | 4 | 8).await;
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .mkdir(&StoragePath::new("Photos/D").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/D/a.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"alpha".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/D/b.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"beta".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();
    h.alice_home
        .mkdir(&StoragePath::new("Photos/D/sub").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/D/sub/c.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"gamma".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    h.bob_view
        .delete(&UserPath::new("/Shared/Photos/D").unwrap())
        .await
        .expect("bob's recursive dir delete should soft-delete into his bin");

    // Alice's source tree is gone.
    assert!(!h.datadir.join("alice/files/Photos/D").exists());

    // Bob has one Dir trash row.
    let rows = h.trash.list("bob").await.unwrap();
    assert_eq!(rows.len(), 1, "exactly one trash row under bob");
    let row = &rows[0];
    assert_eq!(row.user, "bob");
    assert_eq!(row.basename, "D");
    assert_eq!(row.location, "/Shared/Photos");
    assert_eq!(row.r#type, crabcloud_trash::TrashType::Dir);
    assert!(h.trash.list("alice").await.unwrap().is_empty());

    // The full subtree lives at bob's bin under D.d<ts>/.
    let bob_bin = h.datadir.join("bob/files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_bin)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let dir_name = entries[0].file_name().into_string().unwrap();
    assert!(dir_name.starts_with("D.d"), "got {dir_name}");
    let trash_dir = bob_bin.join(&dir_name);
    assert_eq!(
        tokio::fs::read(trash_dir.join("a.txt")).await.unwrap(),
        b"alpha"
    );
    assert_eq!(
        tokio::fs::read(trash_dir.join("b.txt")).await.unwrap(),
        b"beta"
    );
    assert_eq!(
        tokio::fs::read(trash_dir.join("sub/c.txt")).await.unwrap(),
        b"gamma"
    );
}

#[tokio::test]
async fn view_delete_on_share_mount_directory_then_restore_reconstructs_subtree_under_bob() {
    // After bob soft-deletes a shared dir into his bin, restoring it
    // through the Trash service places the full subtree under bob's
    // OWN home at the recorded location. The trash service is
    // uid-scoped; bob "owns" the trash row and restores into his own
    // files dir even though the original lived inside alice's storage
    // via a share mount.
    let h = share_harness(1 | 2 | 4 | 8).await;
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .mkdir(&StoragePath::new("Photos/D").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/D/a.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"alpha".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();
    h.alice_home
        .mkdir(&StoragePath::new("Photos/D/sub").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/D/sub/c.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"gamma".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    h.bob_view
        .delete(&UserPath::new("/Shared/Photos/D").unwrap())
        .await
        .unwrap();

    let id = h.trash.list("bob").await.unwrap()[0].id;
    let restored = h.trash.restore("bob", id, None).await.unwrap();
    assert_eq!(restored.path, "/Shared/Photos/D");

    // Bob's HOME now has the reconstructed tree at /Shared/Photos/D.
    let root = h.datadir.join("bob/files/Shared/Photos/D");
    assert!(root.exists());
    assert_eq!(tokio::fs::read(root.join("a.txt")).await.unwrap(), b"alpha");
    assert_eq!(
        tokio::fs::read(root.join("sub/c.txt")).await.unwrap(),
        b"gamma"
    );

    // Row gone.
    assert!(h.trash.list("bob").await.unwrap().is_empty());
}

#[tokio::test]
async fn view_delete_on_readonly_share_mount_directory_returns_403_and_rolls_back_trash() {
    // Read-only share — alice's source dir is intact, bob ends up with
    // no trash row, and bob's bin has no leftover bytes from a half-
    // staged copy.
    let h = share_harness(1).await; // read only
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .mkdir(&StoragePath::new("Photos/L").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/L/secret.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"sssh".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let r = h
        .bob_view
        .delete(&UserPath::new("/Shared/Photos/L").unwrap())
        .await;
    assert!(
        matches!(
            r,
            Err(crabcloud_fs::FsError::Storage(
                StorageError::PermissionDenied
            ))
        ),
        "expected PermissionDenied, got {r:?}"
    );

    // Source still present.
    assert!(h.datadir.join("alice/files/Photos/L/secret.txt").exists());
    // No trash row under either user.
    assert!(h.trash.list("bob").await.unwrap().is_empty());
    assert!(h.trash.list("alice").await.unwrap().is_empty());
    // Bob's bin contains no leftover bytes from the rolled-back copy.
    let bob_bin = h.datadir.join("bob/files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_bin)
        .map(|d| d.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    assert!(
        entries.is_empty(),
        "bob's bin should have no leftovers after rollback"
    );
}

#[tokio::test]
async fn view_hard_delete_does_not_create_trash_row_or_trashbin_dir() {
    let h = local_harness("bob").await;
    h.view
        .put_file(&UserPath::new("/single.txt").unwrap(), body(b"z".to_vec()))
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
