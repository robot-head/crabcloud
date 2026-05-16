//! Per-user, per-event-type opt-out for email notifications.
//!
//! Default = enabled (true). Stored in `oc_user_notification_prefs`
//! by [crate migration `0007`]. Rows are inserted lazily — the
//! absence of a row means "default enabled".

use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::sync::Arc;
use thiserror::Error;

/// Errors raised by [`NotificationPrefs`].
#[derive(Debug, Error)]
pub enum NotificationPrefsError {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// CRUD handle for the per-user notification opt-out table.
#[derive(Clone)]
pub struct NotificationPrefs {
    pool: Arc<DbPool>,
}

impl NotificationPrefs {
    /// Construct a new handle. Cloning is cheap (only an `Arc` is bumped).
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    /// Returns the enabled state for `(user_id, event_type)`. Defaults
    /// to `true` when no row exists — the absence of a row means the
    /// user has not opted out, which is the desired behaviour.
    pub async fn get(
        &self,
        user_id: &str,
        event_type: &str,
    ) -> Result<bool, NotificationPrefsError> {
        let row: Option<i16> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(
                "SELECT enabled FROM oc_user_notification_prefs \
                 WHERE user_id = ? AND event_type = ?",
            )
            .bind(user_id)
            .bind(event_type)
            .fetch_optional(p)
            .await?
            .map(|r| r.try_get("enabled"))
            .transpose()?,
            DbPool::MySql(p) => sqlx::query(
                "SELECT enabled FROM oc_user_notification_prefs \
                 WHERE user_id = ? AND event_type = ?",
            )
            .bind(user_id)
            .bind(event_type)
            .fetch_optional(p)
            .await?
            .map(|r| r.try_get("enabled"))
            .transpose()?,
            DbPool::Postgres(p) => sqlx::query(
                "SELECT enabled FROM oc_user_notification_prefs \
                 WHERE user_id = $1 AND event_type = $2",
            )
            .bind(user_id)
            .bind(event_type)
            .fetch_optional(p)
            .await?
            .map(|r| r.try_get("enabled"))
            .transpose()?,
        };
        Ok(row.map(|v| v != 0).unwrap_or(true))
    }

    /// Upsert the enabled state for `(user_id, event_type)`. Idempotent.
    pub async fn set(
        &self,
        user_id: &str,
        event_type: &str,
        enabled: bool,
    ) -> Result<(), NotificationPrefsError> {
        let v: i16 = if enabled { 1 } else { 0 };
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs \
                     (user_id, event_type, enabled) VALUES (?, ?, ?) \
                     ON CONFLICT (user_id, event_type) \
                     DO UPDATE SET enabled = excluded.enabled",
                )
                .bind(user_id)
                .bind(event_type)
                .bind(v)
                .execute(p)
                .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs \
                     (user_id, event_type, enabled) VALUES (?, ?, ?) \
                     ON DUPLICATE KEY UPDATE enabled = VALUES(enabled)",
                )
                .bind(user_id)
                .bind(event_type)
                .bind(v)
                .execute(p)
                .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs \
                     (user_id, event_type, enabled) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id, event_type) \
                     DO UPDATE SET enabled = EXCLUDED.enabled",
                )
                .bind(user_id)
                .bind(event_type)
                .bind(v)
                .execute(p)
                .await?;
            }
        }
        Ok(())
    }
}
