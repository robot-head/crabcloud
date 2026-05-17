//! sqlite e2e for the Trash service. Round-trips every public method
//! plus the edge cases the spec calls out (collision suffixing,
//! sweeper aging, sub-second collision).

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_trash::{Trash, TrashType};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

// Workspace deps `serde`, `thiserror`, and `tracing` are first-class
// dependencies of `crabcloud-trash` itself; the integration-test target
// links them too but doesn't use them directly. Keep `unused_crate_dependencies`
// quiet without losing the manifest entries.
use serde as _;
use thiserror as _;
use tracing as _;

/// Spins a fresh sqlite pool + datadir tempdir and runs all migrations.
/// Returns the pool, the datadir, and held-onto `TempDir`s so callers
/// keep the tempdirs alive for the test's lifetime.
async fn setup() -> (Arc<DbPool>, PathBuf, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let data_dir = TempDir::new().unwrap();
    let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let datadir = data_dir.path().to_path_buf();
    (Arc::new(pool), datadir, db_dir, data_dir)
}

/// Seed: write a file inside a user's "files" dir so we can soft-delete it.
async fn write_user_file(datadir: &Path, uid: &str, rel: &str, contents: &[u8]) {
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
async fn soft_delete_writes_row_and_moves_file() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "alice", "/notes/todo.txt", b"hello").await;
    let trash = Trash::new(pool.clone(), datadir.clone());

    let id = trash
        .soft_delete("alice", "/notes/todo.txt", TrashType::File, None)
        .await
        .unwrap();
    assert!(id > 0);

    // Original gone.
    let original = datadir.join("alice/files/notes/todo.txt");
    assert!(
        !original.exists(),
        "original should be removed after soft-delete"
    );

    // Trashbin entry present on disk under the suffix-encoded name.
    let entries: Vec<_> = std::fs::read_dir(datadir.join("alice/files_trashbin/files"))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let name = entries[0].file_name().into_string().unwrap();
    assert!(name.starts_with("todo.txt.d"), "got name {name}");

    // List returns it.
    let listed = trash.list("alice").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].basename, "todo.txt");
    assert_eq!(listed[0].location, "/notes");
    assert_eq!(listed[0].r#type, TrashType::File);
}

#[tokio::test]
async fn restore_moves_file_back_and_deletes_row() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "bob", "/photos/cat.jpg", b"jpeg-bytes").await;
    let trash = Trash::new(pool.clone(), datadir.clone());

    let id = trash
        .soft_delete("bob", "/photos/cat.jpg", TrashType::File, None)
        .await
        .unwrap();
    let restored = trash.restore("bob", id, None).await.unwrap();
    assert_eq!(restored.path, "/photos/cat.jpg");

    // File back at original location.
    let back = datadir.join("bob/files/photos/cat.jpg");
    assert!(back.exists());
    assert_eq!(tokio::fs::read(&back).await.unwrap(), b"jpeg-bytes");

    // Trash row gone.
    assert!(trash.list("bob").await.unwrap().is_empty());
}

#[tokio::test]
async fn restore_auto_creates_missing_parents() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "carol", "/a/b/c/file.txt", b"x").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let id = trash
        .soft_delete("carol", "/a/b/c/file.txt", TrashType::File, None)
        .await
        .unwrap();
    // Remove the parent chain so restore must recreate it.
    tokio::fs::remove_dir_all(datadir.join("carol/files/a"))
        .await
        .unwrap();
    let restored = trash.restore("carol", id, None).await.unwrap();
    assert_eq!(restored.path, "/a/b/c/file.txt");
    assert!(datadir.join("carol/files/a/b/c/file.txt").exists());
}

#[tokio::test]
async fn restore_collision_suffixes_with_restored() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "dave", "/doc.txt", b"v1").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let id = trash
        .soft_delete("dave", "/doc.txt", TrashType::File, None)
        .await
        .unwrap();
    // User created a new file at the same path before restoring.
    write_user_file(&datadir, "dave", "/doc.txt", b"v2").await;

    let restored = trash.restore("dave", id, None).await.unwrap();
    assert_eq!(restored.path, "/doc.txt (restored)");
    assert!(datadir.join("dave/files/doc.txt").exists());
    assert!(datadir.join("dave/files/doc.txt (restored)").exists());
}

#[tokio::test]
async fn purge_deletes_row_and_file() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "eve", "/x.txt", b"z").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let id = trash
        .soft_delete("eve", "/x.txt", TrashType::File, None)
        .await
        .unwrap();
    trash.purge("eve", id).await.unwrap();
    assert!(trash.list("eve").await.unwrap().is_empty());
    let entries: Vec<_> = std::fs::read_dir(datadir.join("eve/files_trashbin/files"))
        .map(|d| d.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn sweep_expired_deletes_old_rows_only() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "fay", "/old.txt", b"o").await;
    write_user_file(&datadir, "fay", "/new.txt", b"n").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let old_id = trash
        .soft_delete("fay", "/old.txt", TrashType::File, None)
        .await
        .unwrap();
    let new_id = trash
        .soft_delete("fay", "/new.txt", TrashType::File, None)
        .await
        .unwrap();
    // Backdate the "old" row by 31 days.
    let cutoff = chrono::Utc::now().timestamp() - 30 * 86400;
    sqlx::query("UPDATE oc_files_trash SET deleted_at = ? WHERE id = ?")
        .bind(cutoff - 86400)
        .bind(old_id)
        .execute(match pool.as_ref() {
            DbPool::Sqlite(p) => p,
            _ => unreachable!(),
        })
        .await
        .unwrap();

    let n = trash.sweep_expired(cutoff, 100).await.unwrap();
    assert_eq!(n, 1);
    let rows = trash.list("fay").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, new_id);
}

#[tokio::test]
async fn sub_second_collision_suffix_increments() {
    let (pool, datadir, _d, _dd) = setup().await;
    write_user_file(&datadir, "gail", "/a.txt", b"1").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    // Two soft-deletes of the same basename within one second.
    let id1 = trash
        .soft_delete("gail", "/a.txt", TrashType::File, None)
        .await
        .unwrap();
    // Recreate the source.
    write_user_file(&datadir, "gail", "/a.txt", b"2").await;
    let id2 = trash
        .soft_delete("gail", "/a.txt", TrashType::File, None)
        .await
        .unwrap();

    let rows = trash.list("gail").await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(
        rows[0].suffix, rows[1].suffix,
        "suffixes must differ across the two deletes"
    );
    // Both rows refer to distinct on-disk files.
    let mut names: Vec<_> = std::fs::read_dir(datadir.join("gail/files_trashbin/files"))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2);
    let _ = (id1, id2);
}
