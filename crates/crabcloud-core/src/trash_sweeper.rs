//! Background task: daily sweep of `oc_files_trash` that purges rows
//! older than `retention_days`. Mirrors the
//! `PreviewCacheCleanup` / `MailQueueCleanup` shape: cooperative
//! shutdown via `Arc<Notify>`, `sweep_once()` for sync test drive.

use crabcloud_trash::Trash;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// 24-hour sleep between sweeps.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// Cap on rows per pass so a giant backlog can't starve other tasks.
const SWEEP_BATCH: i64 = 500;

#[derive(Clone)]
pub struct TrashSweeper {
    trash: Arc<Trash>,
    retention: chrono::Duration,
    shutdown: Arc<Notify>,
}

impl TrashSweeper {
    pub fn new(trash: Arc<Trash>, retention_days: u32) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                trash,
                retention: chrono::Duration::seconds(retention_days as i64 * 86400),
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop. Cancels cooperatively when shutdown notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.sweep_once().await {
                tracing::warn!(error = %e, "trash sweeper: sweep_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single sweep. Returns count of rows purged. Retention 0
    /// disables sweeping (returns Ok(0) without scanning).
    pub async fn sweep_once(&self) -> Result<u64, crabcloud_trash::TrashError> {
        let secs = self.retention.num_seconds();
        if secs <= 0 {
            return Ok(0);
        }
        let cutoff = chrono::Utc::now().timestamp() - secs;
        self.trash.sweep_expired(cutoff, SWEEP_BATCH).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use crabcloud_trash::TrashType;
    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn setup() -> (Arc<DbPool>, PathBuf, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let cfg = minimal_sqlite_config(db_dir.path().join("sweeper.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let datadir = data_dir.path().to_path_buf();
        (Arc::new(pool), datadir, db_dir, data_dir)
    }

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

    fn make_trash(pool: Arc<crabcloud_db::DbPool>, datadir: PathBuf) -> Arc<Trash> {
        let versions = Arc::new(crabcloud_versions::Versions::new(
            pool.clone(),
            datadir.clone(),
        ));
        Arc::new(Trash::new(pool, datadir, versions))
    }

    #[tokio::test]
    async fn sweep_once_with_retention_zero_returns_zero() {
        let (pool, datadir, _d, _dd) = setup().await;
        let trash = make_trash(pool.clone(), datadir.clone());
        let (sweeper, _shutdown) = TrashSweeper::new(trash, 0);
        assert_eq!(sweeper.sweep_once().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweep_once_purges_old_rows() {
        let (pool, datadir, _d, _dd) = setup().await;
        write_user_file(&datadir, "alice", "/old.txt", b"o").await;
        write_user_file(&datadir, "alice", "/new.txt", b"n").await;
        let trash = make_trash(pool.clone(), datadir.clone());
        let old_id = trash
            .soft_delete("alice", "/old.txt", TrashType::File, None)
            .await
            .unwrap();
        let new_id = trash
            .soft_delete("alice", "/new.txt", TrashType::File, None)
            .await
            .unwrap();
        // Backdate the "old" row to 31 days ago.
        let stamp = chrono::Utc::now().timestamp() - 31 * 86400;
        sqlx::query("UPDATE oc_files_trash SET deleted_at = ? WHERE id = ?")
            .bind(stamp)
            .bind(old_id)
            .execute(match pool.as_ref() {
                DbPool::Sqlite(p) => p,
                _ => unreachable!(),
            })
            .await
            .unwrap();

        let (sweeper, _shutdown) = TrashSweeper::new(trash.clone(), 30);
        let n = sweeper.sweep_once().await.unwrap();
        assert_eq!(n, 1);
        let rows = trash.list("alice").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, new_id);
    }
}
