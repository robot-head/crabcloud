//! Background task: hourly sweep of `oc_mail_queue` deleting terminal
//! (`Sent` / `Failed`) rows older than `mail_queue_retention_days`.
//!
//! Without this, the queue grows monotonically: the worker only flips
//! state, it never deletes. The retention window is on `created_at`
//! (not `sent_at`) so the predicate works uniformly for both terminal
//! states without a `COALESCE` — the cleanup is loose by design and the
//! extra "Sent rows that took a while to send" tail is negligible.
//!
//! Multidialect dispatch follows [`crate::mail_queue::MailQueue`]:
//! explicit `match self.pool.as_ref()` arms with per-dialect placeholder
//! syntax (`?` for sqlite + mysql, `$1` for postgres). The query is
//! identical otherwise.
//!
//! Like [`crate::ExpirationWarningSweeper`], the long-running [`run`]
//! loop sleeps on a `tokio::sync::Notify` so test teardown (and
//! eventual graceful-shutdown wiring) can cancel cooperatively, and
//! [`cleanup_once`] is exposed `pub` so e2e tests can drive a sweep
//! synchronously without waiting for the hourly timer.
//!
//! [`run`]: MailQueueCleanup::run
//! [`cleanup_once`]: MailQueueCleanup::cleanup_once

use chrono::Utc;
use crabcloud_db::DbPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// How long to sleep between sweeps in `run()`.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

/// Periodic deleter for terminal rows in `oc_mail_queue`. See module
/// docs for the rationale on the `created_at` predicate.
#[derive(Clone)]
pub struct MailQueueCleanup {
    queue_pool: Arc<DbPool>,
    retention: chrono::Duration,
    shutdown: Arc<Notify>,
}

impl MailQueueCleanup {
    /// Construct a cleanup task + paired shutdown handle. `notify_one()`
    /// on the returned `Arc<Notify>` cancels the `run()` loop after the
    /// current sweep completes.
    pub fn new(pool: Arc<DbPool>, retention_days: u32) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                queue_pool: pool,
                retention: chrono::Duration::days(retention_days as i64),
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop: cleanup, sleep, repeat. Cancels cooperatively
    /// when the paired shutdown `Notify` is notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.cleanup_once().await {
                tracing::warn!(error = %e, "mail_queue_cleanup.cleanup_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single cleanup pass. Returns the number of rows deleted.
    /// Exposed `pub` so integration tests can invoke it directly without
    /// waiting for the hourly timer.
    pub async fn cleanup_once(&self) -> Result<u64, sqlx::Error> {
        let cutoff = (Utc::now() - self.retention).naive_utc();
        let n = match self.queue_pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(
                "DELETE FROM oc_mail_queue \
                 WHERE state IN ('Sent', 'Failed') AND created_at < ?",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
            DbPool::MySql(p) => sqlx::query(
                "DELETE FROM oc_mail_queue \
                 WHERE state IN ('Sent', 'Failed') AND created_at < ?",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
            DbPool::Postgres(p) => sqlx::query(
                "DELETE FROM oc_mail_queue \
                 WHERE state IN ('Sent', 'Failed') AND created_at < $1",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
        };
        Ok(n)
    }
}
