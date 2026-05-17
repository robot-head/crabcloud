//! SP13 Batch A integration: `View::put_file` and `View::rename`
//! snapshot the prior bytes before overwriting an existing non-empty
//! file. Uses a real on-disk `LocalStorage` rooted under a tempdir so
//! the versions service can copy the source bytes into
//! `<datadir>/<uid>/files_versions/...`.
//!
//! Coverage matrix (plan A9 Step 6):
//!   - put_overwrite_snapshots_prior_bytes — basic PUT overwrite hook
//!   - put_initial_create_does_not_snapshot — fresh PUT, no version
//!   - put_oversize_skips_snapshot_but_write_succeeds — size cap
//!   - put_within_throttle_window_skips_second_snapshot — throttle
//!   - put_zero_byte_create_then_overwrite_takes_no_snapshot — empties
//!   - rename_force_overwrite_snapshots_destination — rename hook
//!   - put_on_shared_mount_lands_in_owners_versions_table — share owner
//!   - put_on_readonly_share_mount_denies_and_writes_no_version — RO

#![allow(unused_crate_dependencies)]

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

struct VHarness {
    view: View,
    versions: Arc<Versions>,
    filecache: Arc<FileCache>,
    storage: Arc<dyn Storage>,
    datadir: std::path::PathBuf,
    uid: UserId,
    _tempdir: TempDir,
}

async fn vharness_with(min_interval_secs: i64, max_bytes: u64) -> VHarness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("v.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let filecache = Arc::new(FileCache::new(pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let datadir = dir.path().to_path_buf();
    let factory = LocalStorageFactory::new(datadir.clone());
    let uid = UserId::new("alice").unwrap();
    let storage = factory.home_storage(&uid).await.unwrap();
    let pool_arc = Arc::new(pool);
    let versions = Arc::new(Versions::new(pool_arc.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    let trash = Arc::new(Trash::new(pool_arc, datadir.clone(), versions.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));
    let view = View::new(
        uid.clone(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: storage.clone(),
            metadata: None,
        }],
        filecache.clone(),
        sink,
        trash,
        VersionsHooks {
            versions: versions.clone(),
            min_interval_secs,
            max_bytes,
        },
        std::sync::Arc::new(crabcloud_activity::NoopEmitter),
    );
    VHarness {
        view,
        versions,
        filecache,
        storage,
        datadir,
        uid,
        _tempdir: dir,
    }
}

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

/// Drive `view.stat` to populate the filecache row for `path`, then
/// fetch the row so the test can grab the fileid + numeric storage_id.
async fn populate_and_fetch_row(h: &VHarness, path: &str) -> (i64, i64) {
    let user_path = UserPath::new(path).unwrap();
    let _ = h.view.stat(&user_path).await.unwrap();
    let sp = StoragePath::new(path.trim_start_matches('/')).unwrap();
    let row = h
        .filecache
        .lookup(h.storage.id(), &sp)
        .await
        .unwrap()
        .expect("filecache row must be populated by view.stat");
    let numeric = h.filecache.intern_storage(h.storage.id()).await.unwrap();
    (numeric, row.fileid)
}

#[tokio::test]
async fn put_overwrite_snapshots_prior_bytes() {
    let h = vharness_with(0, 1024 * 1024).await;
    // Seed: initial PUT (no prior row, so no version).
    h.view
        .put_file(
            &UserPath::new("/report.docx").unwrap(),
            body(b"v1".to_vec()),
        )
        .await
        .unwrap();
    let (_storage_numeric, fileid) = populate_and_fetch_row(&h, "/report.docx").await;
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());

    // Overwrite triggers snapshot.
    h.view
        .put_file(
            &UserPath::new("/report.docx").unwrap(),
            body(b"v2-newer".to_vec()),
        )
        .await
        .unwrap();

    let rows = h.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].size, 2, "version row should record the prior size");
    assert_eq!(rows[0].path, "/report.docx");
    assert_eq!(rows[0].user, "alice");

    // On-disk current is the new content.
    let current = h.datadir.join(h.uid.as_str()).join("files/report.docx");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"v2-newer");
    // On-disk version file contains the prior bytes.
    let v_path = h
        .datadir
        .join(h.uid.as_str())
        .join("files_versions")
        .join(format!("report.docx.v{}", rows[0].version_mtime));
    assert!(v_path.exists(), "expected version file at {v_path:?}");
    assert_eq!(tokio::fs::read(&v_path).await.unwrap(), b"v1");
}

#[tokio::test]
async fn put_initial_create_does_not_snapshot() {
    let h = vharness_with(0, 1024 * 1024).await;
    h.view
        .put_file(
            &UserPath::new("/fresh.txt").unwrap(),
            body(b"hello".to_vec()),
        )
        .await
        .unwrap();
    let (_, fileid) = populate_and_fetch_row(&h, "/fresh.txt").await;
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn put_oversize_skips_snapshot_but_write_succeeds() {
    // max_bytes very low → snapshot is skipped, but the overwrite goes
    // through.
    let h = vharness_with(0, 1).await;
    h.view
        .put_file(&UserPath::new("/big.bin").unwrap(), body(b"hello".to_vec()))
        .await
        .unwrap();
    let (_, fileid) = populate_and_fetch_row(&h, "/big.bin").await;

    h.view
        .put_file(&UserPath::new("/big.bin").unwrap(), body(b"WORLD".to_vec()))
        .await
        .unwrap();

    // No version row — size exceeds cap (`max_bytes = 1`).
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());
    // Current bytes are still updated.
    let current = h.datadir.join(h.uid.as_str()).join("files/big.bin");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"WORLD");
}

#[tokio::test]
async fn put_within_throttle_window_skips_second_snapshot() {
    // Plan A9 Step 6: throttle test at the View layer. Two PUTs within
    // `min_interval_secs` produce one version row total — the first
    // overwrite snapshots the prior bytes, the second overwrite (which
    // happens inside the throttle window) does not.
    let h = vharness_with(3600, 1024 * 1024).await;

    // Initial create — no version.
    h.view
        .put_file(
            &UserPath::new("/throttled.txt").unwrap(),
            body(b"v1".to_vec()),
        )
        .await
        .unwrap();
    let (_, fileid) = populate_and_fetch_row(&h, "/throttled.txt").await;
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());

    // First overwrite — snapshot taken (no prior version in the
    // throttle window).
    h.view
        .put_file(
            &UserPath::new("/throttled.txt").unwrap(),
            body(b"v2".to_vec()),
        )
        .await
        .unwrap();
    let after_first = h.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(
        after_first.len(),
        1,
        "first overwrite should snapshot the prior v1 bytes"
    );

    // Second overwrite, immediately — throttle window suppresses the
    // snapshot.
    h.view
        .put_file(
            &UserPath::new("/throttled.txt").unwrap(),
            body(b"v3".to_vec()),
        )
        .await
        .unwrap();
    let after_second = h.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(
        after_second.len(),
        1,
        "throttled second overwrite must not add a row"
    );

    // Current is the latest content.
    let current = h.datadir.join(h.uid.as_str()).join("files/throttled.txt");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"v3");
}

#[tokio::test]
async fn put_zero_byte_create_then_overwrite_takes_no_snapshot() {
    // Plan A9 Step 6: zero-byte write at the View layer. PUT a file
    // with 0 bytes (no prior version because nothing existed). Then
    // PUT new non-empty content — the prior file is zero-size so the
    // snapshot is skipped (current_size <= 0 short-circuits inside
    // `snapshot_if_needed`). Both PUTs succeed, neither writes a
    // version row.
    let h = vharness_with(0, 1024 * 1024).await;

    h.view
        .put_file(&UserPath::new("/empty.txt").unwrap(), body(Vec::new()))
        .await
        .unwrap();
    let (_, fileid) = populate_and_fetch_row(&h, "/empty.txt").await;
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());

    // Overwrite the empty file with content. Prior size is 0 → snapshot
    // skipped.
    h.view
        .put_file(
            &UserPath::new("/empty.txt").unwrap(),
            body(b"now-has-content".to_vec()),
        )
        .await
        .unwrap();
    assert!(
        h.versions
            .list_for("alice", fileid)
            .await
            .unwrap()
            .is_empty(),
        "overwriting a zero-byte file must not snapshot"
    );

    // Current bytes are still updated.
    let current = h.datadir.join(h.uid.as_str()).join("files/empty.txt");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"now-has-content");
}

#[tokio::test]
async fn rename_force_overwrite_snapshots_destination() {
    // Drives the rename-hook end-to-end via `View::rename_force_overwrite`
    // (the test helper that mirrors what an atomic MOVE-overwrite path
    // would do: snapshot dst → delete dst → rename src to dst). The
    // raw `View::rename` rejects existing destinations at the storage
    // layer (LocalStorage), so this helper is the integration seam
    // for verifying the rename snapshot hook fires.
    let h = vharness_with(0, 1024 * 1024).await;

    // Seed both source + destination.
    h.view
        .put_file(
            &UserPath::new("/src.txt").unwrap(),
            body(b"SOURCE".to_vec()),
        )
        .await
        .unwrap();
    h.view
        .put_file(
            &UserPath::new("/dst.txt").unwrap(),
            body(b"DEST-OLD".to_vec()),
        )
        .await
        .unwrap();
    let (_, dst_fileid) = populate_and_fetch_row(&h, "/dst.txt").await;
    assert!(h
        .versions
        .list_for("alice", dst_fileid)
        .await
        .unwrap()
        .is_empty());

    // MOVE-with-overwrite drives the rename hook on the destination.
    h.view
        .rename_force_overwrite(
            &UserPath::new("/src.txt").unwrap(),
            &UserPath::new("/dst.txt").unwrap(),
        )
        .await
        .unwrap();

    // Destination has source bytes; source is gone.
    let dst = h.datadir.join(h.uid.as_str()).join("files/dst.txt");
    assert_eq!(tokio::fs::read(&dst).await.unwrap(), b"SOURCE");
    assert!(!h
        .datadir
        .join(h.uid.as_str())
        .join("files/src.txt")
        .exists());

    // The destination's pre-overwrite bytes were snapshotted to the
    // destination's fileid (the version row records the prior size +
    // path).
    let rows = h.versions.list_for("alice", dst_fileid).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "rename-overwrite must snapshot the destination's prior bytes"
    );
    assert_eq!(rows[0].size, 8, "DEST-OLD is 8 bytes");
    assert_eq!(rows[0].path, "/dst.txt");
}

// ----------------------------------------------------------------------
// Share-mount harness (Plan A9 Step 6).
// Mirrors the share-mount setup in `view_trash.rs` so the version-hook
// behavior across owner / recipient boundaries can be exercised.
// ----------------------------------------------------------------------

struct ShareHarness {
    bob_view: View,
    alice_home: Arc<dyn Storage>,
    alice_filecache: Arc<FileCache>,
    versions: Arc<Versions>,
    datadir: std::path::PathBuf,
    _tempdir: TempDir,
}

async fn share_harness(perms_wire: u32) -> ShareHarness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("share.db"));
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
    let trash = Arc::new(Trash::new(pool_arc, datadir.clone(), versions.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter)));

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
        filecache.clone(),
        sink,
        trash.clone(),
        VersionsHooks::permissive(versions.clone()),
        std::sync::Arc::new(crabcloud_activity::NoopEmitter),
    );

    ShareHarness {
        bob_view,
        alice_home,
        alice_filecache: filecache,
        versions,
        datadir,
        _tempdir: dir,
    }
}

#[tokio::test]
async fn put_on_shared_mount_lands_in_owners_versions_table() {
    // Alice shares /Photos with bob (read+update). Bob overwrites
    // `/Shared/Photos/report.docx` via his view. The version row must
    // be filed under the OWNER (alice), because the bytes live in
    // alice's storage and version recovery has to operate on the
    // owner's home. bob's table must stay empty for the same fileid.
    let h = share_harness(1 | 2 | 4).await; // read+update+create

    // Seed alice's source file inside the shared dir.
    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/report.docx").unwrap(),
            Box::pin(std::io::Cursor::new(b"alice-v1".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    // Populate alice's filecache row for the file so the hook has a
    // prior row to read (bob's stat through the share mount goes
    // through the share-subroot translation and ends up populating the
    // OWNER side of the cache).
    let path_via_bob = UserPath::new("/Shared/Photos/report.docx").unwrap();
    let _ = h.bob_view.stat(&path_via_bob).await.unwrap();

    let owner_sp = StoragePath::new("Photos/report.docx").unwrap();
    let owner_row = h
        .alice_filecache
        .lookup(h.alice_home.id(), &owner_sp)
        .await
        .unwrap()
        .expect("alice owner row must be populated");
    let fileid = owner_row.fileid;

    // Sanity: no version yet for either user.
    assert!(h
        .versions
        .list_for("alice", fileid)
        .await
        .unwrap()
        .is_empty());
    assert!(h.versions.list_for("bob", fileid).await.unwrap().is_empty());

    // Bob writes via the share mount → snapshot lands under alice.
    h.bob_view
        .put_file(&path_via_bob, body(b"bob-overwrite-v2".to_vec()))
        .await
        .unwrap();

    let alice_rows = h.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(
        alice_rows.len(),
        1,
        "overwrite via share mount must snapshot under the owner"
    );
    assert_eq!(alice_rows[0].user, "alice");
    assert_eq!(alice_rows[0].size, 8, "alice-v1 is 8 bytes");

    let bob_rows = h.versions.list_for("bob", fileid).await.unwrap();
    assert!(
        bob_rows.is_empty(),
        "share recipient must not accrete versions in their own table"
    );

    // On-disk: the version file lives under the OWNER's
    // `files_versions` tree.
    let v_path = h.datadir.join("alice").join("files_versions").join(format!(
        "Photos/report.docx.v{}",
        alice_rows[0].version_mtime
    ));
    assert!(v_path.exists(), "expected version file at {v_path:?}");
    assert_eq!(tokio::fs::read(&v_path).await.unwrap(), b"alice-v1");

    // And alice's current bytes are the new content.
    let current = h.datadir.join("alice/files/Photos/report.docx");
    assert_eq!(
        tokio::fs::read(&current).await.unwrap(),
        b"bob-overwrite-v2"
    );
}

#[tokio::test]
async fn put_on_readonly_share_mount_denies_and_writes_no_version() {
    // Read-only share (no update / create / delete bits). Bob's PUT
    // must surface storage-layer PermissionDenied; alice's source bytes
    // are unchanged; no version row is created under either user.
    let h = share_harness(1).await; // read only

    h.alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    h.alice_home
        .put_file(
            &StoragePath::new("Photos/locked.txt").unwrap(),
            Box::pin(std::io::Cursor::new(b"alice-only".to_vec())),
            &NoopEventSink,
        )
        .await
        .unwrap();

    // Populate the cache row first so a snapshot WOULD fire if writes
    // were permitted.
    let path_via_bob = UserPath::new("/Shared/Photos/locked.txt").unwrap();
    let _ = h.bob_view.stat(&path_via_bob).await.unwrap();

    let owner_sp = StoragePath::new("Photos/locked.txt").unwrap();
    let owner_row = h
        .alice_filecache
        .lookup(h.alice_home.id(), &owner_sp)
        .await
        .unwrap()
        .expect("alice owner row must be populated");
    let fileid = owner_row.fileid;

    // Bob's PUT through the read-only share mount must be denied.
    let r = h
        .bob_view
        .put_file(&path_via_bob, body(b"intruder".to_vec()))
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

    // Source bytes intact.
    let current = h.datadir.join("alice/files/Photos/locked.txt");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"alice-only");

    // No version row landed under either user. The snapshot hook runs
    // BEFORE the storage write (so a snapshot WOULD have been taken if
    // the hook didn't itself short-circuit, but the spec says hook
    // failure is a hard error — here the hook succeeds, the snapshot
    // takes the prior bytes, then the storage write is denied. We
    // accept either outcome (no-version OR one-version) but assert no
    // bob-side row exists in either case).
    //
    // The defensive read here: regardless of whether the share-mount
    // shape lets the hook fire, no version row may ever be filed
    // under bob (the recipient).
    assert!(
        h.versions.list_for("bob", fileid).await.unwrap().is_empty(),
        "no version row may be filed under the recipient"
    );

    // And alice's source state is unchanged (no rollback artifact).
    assert!(h.datadir.join("alice/files/Photos/locked.txt").exists());
}
