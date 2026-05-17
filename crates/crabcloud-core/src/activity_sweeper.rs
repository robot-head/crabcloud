//! Background task: daily age-based sweep of `oc_activity`. Mirrors the
//! `VersionsSweeper` / `TrashSweeper` shape: cooperative shutdown via
//! `Arc<Notify>`, `sweep_once()` for sync test drive.
//!
//! `retention_days = 0` short-circuits to `Ok(0)` (compliance
//! retain-forever escape hatch). The activity log accepts the rare
//! coalesce-race duplicate row without a UNIQUE constraint; future
//! work could add upsert plumbing if the duplicate count grows.

use crabcloud_activity::Activity;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct ActivitySweeper {
    activity: Arc<Activity>,
    retention: chrono::Duration,
    shutdown: Arc<Notify>,
}

impl ActivitySweeper {
    pub fn new(activity: Arc<Activity>, retention_days: u32) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                activity,
                retention: chrono::Duration::seconds(retention_days as i64 * 86_400),
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop. Cancels cooperatively when shutdown notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.sweep_once().await {
                tracing::warn!(error = %e, "activity sweeper: sweep_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single sweep. Returns count of rows purged. Retention
    /// `<= 0` short-circuits to `Ok(0)`.
    pub async fn sweep_once(&self) -> Result<u64, crabcloud_activity::ActivityError> {
        let secs = self.retention.num_seconds();
        if secs <= 0 {
            return Ok(0);
        }
        let cutoff = chrono::Utc::now().timestamp() - secs;
        self.activity.sweep_expired(cutoff).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_activity::{Activity, ActivitySettings};
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::TempDir;

    async fn setup_activity() -> (Arc<Activity>, TempDir) {
        let db = TempDir::new().unwrap();
        let cfg = minimal_sqlite_config(db.path().join("t.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let pool = Arc::new(pool);
        let settings = ActivitySettings::new(pool.clone());
        (Arc::new(Activity::new(pool, settings, 0)), db)
    }

    #[tokio::test]
    async fn sweep_once_disabled_returns_zero() {
        let (activity, _d) = setup_activity().await;
        let (sw, _) = ActivitySweeper::new(activity, /*retention_days*/ 0);
        assert_eq!(sw.sweep_once().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweep_once_enabled_returns_zero_with_no_data() {
        let (activity, _d) = setup_activity().await;
        let (sw, _) = ActivitySweeper::new(activity, 365);
        assert_eq!(sw.sweep_once().await.unwrap(), 0);
    }
}
