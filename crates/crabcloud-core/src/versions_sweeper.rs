//! Background task: daily tiered-retention sweep of `oc_files_versions`.
//! Mirrors the `TrashSweeper` / `MailQueueCleanup` shape: cooperative
//! shutdown via `Arc<Notify>`, `sweep_once()` for sync test drive.

use crabcloud_versions::Versions;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// 24-hour sleep between sweeps.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct VersionsSweeper {
    versions: Arc<Versions>,
    retention_disabled: bool,
    shutdown: Arc<Notify>,
}

impl VersionsSweeper {
    pub fn new(versions: Arc<Versions>, retention_disabled: bool) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                versions,
                retention_disabled,
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop. Cancels cooperatively when shutdown notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.sweep_once().await {
                tracing::warn!(error = %e, "versions sweeper: sweep_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single sweep. Returns count of rows purged.
    /// `retention_disabled` short-circuits to `Ok(0)` without scanning
    /// (compliance retain-forever escape hatch).
    pub async fn sweep_once(&self) -> Result<u64, crabcloud_versions::VersionsError> {
        if self.retention_disabled {
            return Ok(0);
        }
        let now = chrono::Utc::now().timestamp();
        self.versions.sweep_tiered(now).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::TempDir;

    async fn make_versions() -> (Arc<Versions>, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let cfg = minimal_sqlite_config(db_dir.path().join("vs.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let versions = Arc::new(Versions::new(
            Arc::new(pool),
            data_dir.path().to_path_buf(),
        ));
        (versions, db_dir, data_dir)
    }

    #[tokio::test]
    async fn sweep_once_disabled_returns_zero() {
        let (versions, _d, _dd) = make_versions().await;
        let (sw, _shutdown) = VersionsSweeper::new(versions, true);
        assert_eq!(sw.sweep_once().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweep_once_enabled_runs_without_data() {
        let (versions, _d, _dd) = make_versions().await;
        let (sw, _shutdown) = VersionsSweeper::new(versions, false);
        // No data → nothing to purge, returns Ok(0).
        assert_eq!(sw.sweep_once().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweep_once_purges_old_duplicate_versions() {
        let (versions, _d, data_dir) = make_versions().await;
        let datadir = data_dir.path();
        // Seed two versions inside the same hour bucket, 7 days old —
        // tiered sweeper should drop the older.
        let now = chrono::Utc::now().timestamp();
        let week_ago = now - 7 * 86_400;
        for offset in [0i64, 30] {
            let p = datadir.join("alice/files/y.txt");
            tokio::fs::create_dir_all(p.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&p, format!("v{offset}")).await.unwrap();
            versions
                .snapshot_if_needed("alice", 1, 200, "/y.txt", 4, week_ago - offset, 0, 1024)
                .await
                .unwrap();
        }
        assert_eq!(versions.list_for("alice", 200).await.unwrap().len(), 2);
        let (sw, _shutdown) = VersionsSweeper::new(versions.clone(), false);
        let n = sw.sweep_once().await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(versions.list_for("alice", 200).await.unwrap().len(), 1);
    }
}
