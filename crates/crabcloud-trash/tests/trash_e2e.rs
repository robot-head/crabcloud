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
    assert_ne!(
        id1, id2,
        "concurrent same-second deletes must produce distinct ids"
    );
}

/// Seed a directory tree under a dummy "owner home" outside the deleter's
/// own files directory, so the new directory soft-delete actually has to
/// cross-storage copy rather than rename.
async fn seed_owner_tree(datadir: &Path, owner: &str, rel: &str) {
    let base = datadir
        .join(owner)
        .join("files")
        .join(rel.trim_start_matches('/'));
    tokio::fs::create_dir_all(&base).await.unwrap();
    tokio::fs::write(base.join("a.txt"), b"alpha")
        .await
        .unwrap();
    tokio::fs::write(base.join("b.txt"), b"beta").await.unwrap();
    let sub = base.join("sub");
    tokio::fs::create_dir(&sub).await.unwrap();
    tokio::fs::write(sub.join("c.txt"), b"gamma").await.unwrap();
}

#[tokio::test]
async fn soft_delete_directory_from_path_copies_tree_and_writes_dir_row() {
    let (pool, datadir, _d, _dd) = setup().await;
    // Owner "alice" has /Photos/D with the seeded subtree. Deleter "bob"
    // owns the trash row.
    seed_owner_tree(&datadir, "alice", "/Photos/D").await;
    let trash = Trash::new(pool.clone(), datadir.clone());

    let src = datadir.join("alice/files/Photos/D");
    let id = trash
        .soft_delete_directory_from_path("bob", "/Shared/Photos", "D", &src, None)
        .await
        .unwrap();
    assert!(id > 0);

    // Single Dir row under bob with location reflecting the deleter view.
    let rows = trash.list("bob").await.unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.user, "bob");
    assert_eq!(row.basename, "D");
    assert_eq!(row.location, "/Shared/Photos");
    assert_eq!(row.r#type, TrashType::Dir);

    // Full subtree present under bob's trashbin.
    let bob_bin = datadir.join("bob/files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_bin)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let dir_name = entries[0].file_name().into_string().unwrap();
    assert!(dir_name.starts_with("D.d"), "got {dir_name}");
    let dir = bob_bin.join(&dir_name);
    assert_eq!(tokio::fs::read(dir.join("a.txt")).await.unwrap(), b"alpha");
    assert_eq!(tokio::fs::read(dir.join("b.txt")).await.unwrap(), b"beta");
    assert_eq!(
        tokio::fs::read(dir.join("sub/c.txt")).await.unwrap(),
        b"gamma"
    );

    // No row under alice (the deleter is bob).
    assert!(trash.list("alice").await.unwrap().is_empty());

    // The source tree was NOT touched by the trash service — removal of
    // the source is the caller's responsibility per the doc comment.
    assert!(src.join("a.txt").exists());
}

#[tokio::test]
async fn soft_delete_directory_from_path_rejects_bad_basename() {
    let (pool, datadir, _d, _dd) = setup().await;
    seed_owner_tree(&datadir, "alice", "/Photos/D").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let src = datadir.join("alice/files/Photos/D");
    for bad in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
        let r = trash
            .soft_delete_directory_from_path("bob", "/", bad, &src, None)
            .await;
        assert!(r.is_err(), "expected error for basename {bad:?}");
    }
}

#[tokio::test]
async fn soft_delete_directory_from_path_missing_source_returns_source_missing() {
    let (pool, datadir, _d, _dd) = setup().await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let r = trash
        .soft_delete_directory_from_path(
            "bob",
            "/Shared/Photos",
            "Ghost",
            &datadir.join("alice/files/Photos/Ghost"),
            None,
        )
        .await;
    assert!(matches!(r, Err(crabcloud_trash::TrashError::SourceMissing)));
    assert!(trash.list("bob").await.unwrap().is_empty());
}

#[tokio::test]
async fn restore_directory_round_trips_full_subtree() {
    let (pool, datadir, _d, _dd) = setup().await;
    seed_owner_tree(&datadir, "alice", "/Photos/D").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let src = datadir.join("alice/files/Photos/D");
    let id = trash
        .soft_delete_directory_from_path("bob", "/Shared/Photos", "D", &src, None)
        .await
        .unwrap();

    // Restore back to the row's recorded location under bob's HOME
    // (bob's own files dir — the trash service is uid-scoped).
    let restored = trash.restore("bob", id, None).await.unwrap();
    assert_eq!(restored.path, "/Shared/Photos/D");
    let restored_root = datadir.join("bob/files/Shared/Photos/D");
    assert!(restored_root.exists());
    assert_eq!(
        tokio::fs::read(restored_root.join("a.txt")).await.unwrap(),
        b"alpha"
    );
    assert_eq!(
        tokio::fs::read(restored_root.join("b.txt")).await.unwrap(),
        b"beta"
    );
    assert_eq!(
        tokio::fs::read(restored_root.join("sub/c.txt"))
            .await
            .unwrap(),
        b"gamma"
    );
    // Row gone.
    assert!(trash.list("bob").await.unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn soft_delete_directory_from_path_rolls_back_partial_destination_on_failure() {
    // Construct a source tree that contains an unsupported file type
    // (a symlink) so the recursive walker errors mid-traversal. The
    // failure must trigger a full rollback: bob's bin ends up empty,
    // no row was written, and (since the trash service doesn't touch
    // the source) the source remains intact.
    let (pool, datadir, _d, _dd) = setup().await;
    let owner_files = datadir.join("alice/files/Photos/D");
    tokio::fs::create_dir_all(&owner_files).await.unwrap();
    tokio::fs::write(owner_files.join("good.txt"), b"ok")
        .await
        .unwrap();
    // Symlink (unsupported file type) — the walker hits the else branch
    // and returns InvalidData partway through.
    std::os::unix::fs::symlink("/dev/null", owner_files.join("weird_link")).unwrap();
    tokio::fs::write(owner_files.join("also_good.txt"), b"ok2")
        .await
        .unwrap();

    let trash = Trash::new(pool.clone(), datadir.clone());
    let r = trash
        .soft_delete_directory_from_path("bob", "/Shared/Photos", "D", &owner_files, None)
        .await;
    assert!(r.is_err(), "expected error from symlink in source");

    // No trash row for bob.
    assert!(trash.list("bob").await.unwrap().is_empty());
    // Bob's bin contains no leftover bytes from the rolled-back tree.
    let bob_bin = datadir.join("bob/files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_bin)
        .map(|d| d.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    assert!(entries.is_empty(), "bin should be empty after rollback");
    // Source intact — the trash service never touches it; the caller
    // owns source removal.
    assert!(owner_files.join("good.txt").exists());
    assert!(owner_files.join("also_good.txt").exists());
}

#[tokio::test]
async fn purge_directory_removes_subtree_and_row() {
    let (pool, datadir, _d, _dd) = setup().await;
    seed_owner_tree(&datadir, "alice", "/Photos/D").await;
    let trash = Trash::new(pool.clone(), datadir.clone());
    let src = datadir.join("alice/files/Photos/D");
    let id = trash
        .soft_delete_directory_from_path("bob", "/Shared/Photos", "D", &src, None)
        .await
        .unwrap();
    trash.purge("bob", id).await.unwrap();
    assert!(trash.list("bob").await.unwrap().is_empty());
    let bob_bin = datadir.join("bob/files_trashbin/files");
    let entries: Vec<_> = std::fs::read_dir(&bob_bin)
        .map(|d| d.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    assert!(entries.is_empty(), "bin should be empty after purge");
}
