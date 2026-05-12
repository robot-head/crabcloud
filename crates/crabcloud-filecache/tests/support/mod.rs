//! Shared test fixtures: SQLite pool with migrations applied, helpers for
//! constructing `FileMetadata` and `Harness`.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_storage::{ETag, FileKind, FileMetadata, Mimetype, Permissions, StoragePath};
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
