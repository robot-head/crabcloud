//! Shared test fixtures: SQLite pool with migrations applied, helpers for
//! constructing `FileMetadata` and `Harness`.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_storage::{ETag, FileKind, FileMetadata, Mimetype, Permissions, StoragePath};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

pub fn make_metadata(path: &str, size: u64, mimetype: &str) -> FileMetadata {
    FileMetadata {
        path: StoragePath::new(path).unwrap(),
        kind: FileKind::File,
        size,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(mimetype).unwrap(),
        permissions: Permissions::full(),
    }
}

pub fn make_dir_metadata(path: &str) -> FileMetadata {
    FileMetadata {
        path: StoragePath::new(path).unwrap(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::octet_stream(),
        permissions: Permissions::full(),
    }
}

pub struct Harness {
    pub pool: DbPool,
    pub _tempdir: TempDir,
}

pub async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("h.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    Harness {
        pool,
        _tempdir: dir,
    }
}

/// Like `harness()`, but pins the SQLite pool to `max_connections = 1`.
///
/// **Why one connection:** SQLite's busy_timeout only retries when waiting
/// for the OS-level file lock. Pool-internal contention (two connections
/// in the same process both holding write transactions on the same file)
/// is NOT routed through busy_handler, so the second tx sees `SQLITE_BUSY`
/// immediately even with a 10-second pragma. Pinning to one connection
/// makes sqlx's own pool queue serialize transactions — no BUSY race.
///
/// This still exercises the populate path's per-path lock map: N parallel
/// `cache.stat` calls all wait on the same connection at the apply step,
/// but the lock acquisition + backend stat phase is genuinely parallel,
/// which is the property under test.
pub async fn harness_concurrent() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("h.db");
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
        .unwrap()
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(10));
    let sqlite_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    let pool = DbPool::Sqlite(sqlite_pool);
    let mut runner = MigrationRunner::new(&pool, "oc_");
    runner.register(core_set());
    runner.run().await.unwrap();
    Harness {
        pool,
        _tempdir: dir,
    }
}
