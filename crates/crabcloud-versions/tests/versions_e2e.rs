//! sqlite e2e for the Versions service.

#![allow(unused_crate_dependencies)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_versions::{Versions, VersionsError};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

async fn setup() -> (Arc<DbPool>, PathBuf, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let data_dir = TempDir::new().unwrap();
    let cfg = minimal_sqlite_config(db_dir.path().join("versions.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let datadir = data_dir.path().to_path_buf();
    (Arc::new(pool), datadir, db_dir, data_dir)
}

/// Write a file under `<datadir>/<uid>/files/<rel>`.
async fn write_user_file(datadir: &std::path::Path, uid: &str, rel: &str, contents: &[u8]) {
    let p = datadir
        .join(uid)
        .join("files")
        .join(rel.trim_start_matches('/'));
    tokio::fs::create_dir_all(p.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&p, contents).await.unwrap();
}

#[tokio::test]
async fn snapshot_writes_row_and_copies_bytes() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/report.docx", b"v1").await;

    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/report.docx", 2, 1_716_000_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot id");
    assert!(id > 0);

    // On-disk version file exists with the original bytes.
    let v_path = datadir.join("alice/files_versions/report.docx.v1716000000");
    assert!(v_path.exists(), "version file should exist at {v_path:?}");
    assert_eq!(tokio::fs::read(&v_path).await.unwrap(), b"v1");

    // Original is untouched (snapshot is a copy, not a move).
    let original = datadir.join("alice/files/report.docx");
    assert_eq!(tokio::fs::read(&original).await.unwrap(), b"v1");

    // List returns the entry.
    let rows = versions.list_for("alice", 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].size, 2);
    assert_eq!(rows[0].version_mtime, 1_716_000_000);
    assert_eq!(rows[0].path, "/report.docx");
    assert_eq!(rows[0].user, "alice");
    assert_eq!(rows[0].storage_id, 1);
    assert_eq!(rows[0].fileid, 100);
}

#[tokio::test]
async fn snapshot_skips_when_throttled() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/a.txt", b"v1").await;

    versions
        .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("first snapshot");
    // Second snapshot at now=1001 (within throttle window of 2s) → None.
    let r = versions
        .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_001, 2, 1024)
        .await
        .unwrap();
    assert!(r.is_none(), "throttled snapshot should return None");
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 1);

    // Past throttle (now=1003) → snapshot.
    let r = versions
        .snapshot_if_needed("alice", 1, 100, "/a.txt", 2, 1_003, 2, 1024)
        .await
        .unwrap();
    assert!(r.is_some(), "post-throttle snapshot should succeed");
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 2);
}

#[tokio::test]
async fn snapshot_skips_when_oversize() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/big.bin", b"hello").await;

    let r = versions
        .snapshot_if_needed("alice", 1, 100, "/big.bin", 999_999_999, 1_000, 2, 1024)
        .await
        .unwrap();
    assert!(r.is_none());
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn snapshot_skips_on_zero_byte() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/empty.txt", b"").await;

    let r = versions
        .snapshot_if_needed("alice", 1, 100, "/empty.txt", 0, 1_000, 2, 1024)
        .await
        .unwrap();
    assert!(r.is_none());
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn snapshot_into_nested_directory_creates_parents() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/projects/q1/report.docx", b"hello").await;

    let id = versions
        .snapshot_if_needed(
            "alice",
            1,
            100,
            "/projects/q1/report.docx",
            5,
            1_716_000_000,
            2,
            1024,
        )
        .await
        .unwrap()
        .expect("snapshot id");
    assert!(id > 0);
    let v_path = datadir.join("alice/files_versions/projects/q1/report.docx.v1716000000");
    assert!(v_path.exists(), "nested version should exist at {v_path:?}");
}

#[tokio::test]
async fn snapshot_source_missing_returns_err() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    // No file at <datadir>/alice/files/missing.txt
    let r = versions
        .snapshot_if_needed("alice", 1, 100, "/missing.txt", 5, 1_000, 2, 1024)
        .await;
    assert!(matches!(r, Err(VersionsError::SourceMissing)));
}

#[tokio::test]
async fn restore_snapshots_current_then_replaces() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/report.docx", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/report.docx", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");

    // Current file changes to v2.
    write_user_file(&datadir, "alice", "/report.docx", b"v2-newer").await;

    // Restore v1. now is 2_000 — well outside throttle, so the auto-
    // snapshot of the current state fires.
    versions
        .restore("alice", id, 8, 2_000, 2, 1024)
        .await
        .unwrap();

    // Current is now v1 again.
    let current = datadir.join("alice/files/report.docx");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"v1");

    // Two versions exist: the original v1 + a snapshot of v2 (taken
    // before the restore overwrote current).
    let rows = versions.list_for("alice", 100).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Newest-first: the new snapshot is first.
    assert_eq!(rows[0].size, 8);
    assert_eq!(rows[1].id, id);
}

#[tokio::test]
async fn restore_wrong_user_errors() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/report.docx", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/report.docx", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");

    let r = versions.restore("bob", id, 2, 2_000, 2, 1024).await;
    assert!(matches!(r, Err(VersionsError::WrongUser)));
}

#[tokio::test]
async fn delete_removes_row_and_file() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");

    versions.delete("alice", id).await.unwrap();
    assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
    assert!(!datadir.join("alice/files_versions/x.txt.v1000").exists());
}

#[tokio::test]
async fn delete_wrong_user_returns_error() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");

    let r = versions.delete("bob", id).await;
    assert!(matches!(r, Err(VersionsError::WrongUser)));
}

#[tokio::test]
async fn delete_not_found_returns_error() {
    let (pool, _datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), _datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    let r = versions.delete("alice", 99_999).await;
    assert!(matches!(r, Err(VersionsError::NotFound)));
}

#[tokio::test]
async fn purge_for_fileid_removes_all_versions() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
    versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 0, 1024)
        .await
        .unwrap();
    write_user_file(&datadir, "alice", "/x.txt", b"v2").await;
    versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_003, 0, 1024)
        .await
        .unwrap();
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 2);

    let n = versions.purge_for_fileid(1, 100).await.unwrap();
    assert_eq!(n, 2);
    assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
    assert!(!datadir.join("alice/files_versions/x.txt.v1000").exists());
    assert!(!datadir.join("alice/files_versions/x.txt.v1003").exists());
}

#[tokio::test]
async fn purge_for_user_fileid_removes_all_versions() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
    versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 0, 1024)
        .await
        .unwrap();
    write_user_file(&datadir, "alice", "/x.txt", b"v2").await;
    versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_003, 0, 1024)
        .await
        .unwrap();
    assert_eq!(versions.list_for("alice", 100).await.unwrap().len(), 2);

    let n = versions.purge_for_user_fileid("alice", 100).await.unwrap();
    assert_eq!(n, 2);
    assert!(versions.list_for("alice", 100).await.unwrap().is_empty());
}

#[tokio::test]
async fn sweep_tiered_keeps_at_least_one_per_bucket() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));

    // Seed 6 versions of the same file spanning several buckets.
    let now: i64 = 1_000_000_000;
    let day = 86_400i64;
    let hour = 3_600i64;
    for offset in [-1i64, -hour - 1, -2 * day, -10 * day, -45 * day, -200 * day] {
        // The on-disk path matters per snapshot — write the current
        // file (with a fresh content marker), then snapshot.
        write_user_file(&datadir, "alice", "/y.txt", format!("v{offset}").as_bytes()).await;
        versions
            .snapshot_if_needed("alice", 1, 200, "/y.txt", 4, now + offset, 0, 1024)
            .await
            .unwrap();
    }
    assert_eq!(versions.list_for("alice", 200).await.unwrap().len(), 6);

    let _purged = versions.sweep_tiered(now).await.unwrap();
    let post = versions.list_for("alice", 200).await.unwrap();

    // All six versions land in distinct bucket slots, so none should be
    // dropped:
    //  -1s            → tag 0 (keep every)
    //  -hour-1        → tag 0 (still <24h ago)
    //  -2d            → tag 1 (24h-30d, hour slot)
    //  -10d           → tag 1 (different hour slot)
    //  -45d           → tag 2 (30d-180d, day slot)
    //  -200d          → tag 3 (180d+, week slot)
    assert_eq!(post.len(), 6);
}

#[tokio::test]
async fn sweep_tiered_drops_duplicates_in_same_hour_bucket() {
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));

    let now: i64 = 1_000_000_000;
    let day = 86_400i64;
    // Two versions 7 days old, 30 seconds apart → same hour slot, tag
    // 1. The sweeper must drop the older one.
    for offset in [-7 * day, -7 * day - 30] {
        write_user_file(&datadir, "alice", "/y.txt", format!("v{offset}").as_bytes()).await;
        versions
            .snapshot_if_needed("alice", 1, 200, "/y.txt", 4, now + offset, 0, 1024)
            .await
            .unwrap();
    }
    assert_eq!(versions.list_for("alice", 200).await.unwrap().len(), 2);
    let purged = versions.sweep_tiered(now).await.unwrap();
    assert_eq!(purged, 1);
    let post = versions.list_for("alice", 200).await.unwrap();
    assert_eq!(post.len(), 1);
    // Newest-first ordering: the surviving row is the newer of the two.
    assert_eq!(post[0].version_mtime, now - 7 * day);
}

#[tokio::test]
async fn restore_succeeds_when_current_source_missing() {
    // Regression: restore is the recovery lever. The pre-restore
    // snapshot of CURRENT must not abort restore when current is gone
    // from disk (the very situation where the user needs to recover an
    // older version).
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/report.docx", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/report.docx", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");

    // Simulate current being gone on disk (no row removal — the
    // filecache still thinks the file exists with size 2).
    tokio::fs::remove_file(datadir.join("alice/files/report.docx"))
        .await
        .unwrap();

    // Restore must succeed; the pre-snapshot of the missing current is
    // a soft skip.
    versions
        .restore("alice", id, 2, 2_000, 0, 1024)
        .await
        .expect("restore should succeed even with current missing");

    // Restored bytes are present.
    let current = datadir.join("alice/files/report.docx");
    assert_eq!(tokio::fs::read(&current).await.unwrap(), b"v1");

    // Only the original version row remains (no pre-restore snapshot
    // was taken because current was gone).
    let rows = versions.list_for("alice", 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
}

#[tokio::test]
async fn snapshot_duplicate_in_same_second_is_soft_skip() {
    // The UNIQUE index on (storage_id, fileid, version_mtime) rejects
    // a second insert with the same triple. `snapshot_if_needed` must
    // surface that as `Ok(None)` (not Err) so concurrent writers in
    // the same second don't break either caller's write path.
    let (pool, datadir, _d, _dd) = setup().await;
    let versions = Versions::new(pool.clone(), datadir.clone(), std::sync::Arc::new(crabcloud_activity::NoopEmitter));
    write_user_file(&datadir, "alice", "/race.txt", b"v1").await;

    let id = versions
        .snapshot_if_needed("alice", 7, 100, "/race.txt", 2, 1_000, 0, 1024)
        .await
        .unwrap()
        .expect("first snapshot wins");
    assert!(id > 0);

    // Second snapshot at identical `now_secs` against the same
    // (storage_id, fileid). Throttle is 0 so we don't short-circuit on
    // the throttle path — we want the INSERT to actually be attempted.
    let r = versions
        .snapshot_if_needed("alice", 7, 100, "/race.txt", 2, 1_000, 0, 1024)
        .await
        .expect("duplicate must not error");
    assert!(r.is_none(), "duplicate in same second is a soft skip");

    // The on-disk version file is preserved (the winner's bytes — and
    // since both racers race identical source bytes, it's lossless).
    let v_path = datadir.join("alice/files_versions/race.txt.v1000");
    assert!(v_path.exists());
    assert_eq!(tokio::fs::read(&v_path).await.unwrap(), b"v1");

    // Exactly one row in the table.
    let rows = versions.list_for("alice", 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
}

#[tokio::test]
async fn restore_emits_activity_for_owner() {
    // Build a real Activity (not Noop) so the emit is observable.
    let (pool, datadir, _d, _dd) = setup().await;
    let settings = crabcloud_activity::ActivitySettings::new(pool.clone());
    let activity = std::sync::Arc::new(crabcloud_activity::Activity::new(
        pool.clone(),
        settings,
        0,
    ));
    let versions = Versions::new(
        pool.clone(),
        datadir.clone(),
        activity.clone() as std::sync::Arc<dyn crabcloud_activity::ActivityEmitter>,
    );
    write_user_file(&datadir, "alice", "/x.txt", b"v1").await;
    let id = versions
        .snapshot_if_needed("alice", 1, 100, "/x.txt", 2, 1_000, 2, 1024)
        .await
        .unwrap()
        .expect("snapshot");
    write_user_file(&datadir, "alice", "/x.txt", b"v2").await;
    versions.restore("alice", id, 8, 2_000, 2, 1024).await.unwrap();

    let rows = activity.list("alice", None, 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_type, "version_restored");
    assert_eq!(rows[0].actor, "alice");
    assert_eq!(rows[0].subject_id, "version_restored_you");
}
