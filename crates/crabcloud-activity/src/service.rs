//! `Activity` — emit/list/sweep + impl ActivityEmitter.
//!
//! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
//! Coalesce: same `(recipient, actor, event_type, object_id)` within
//! `coalesce_window_secs` UPDATEs `count + last_seen_at`; otherwise
//! INSERTs a fresh row. The coalesce race (two concurrent emits both
//! INSERTing) is intentionally not guarded by a UNIQUE constraint per
//! decision §6 of the spec — accepted as a rare self-resolving anomaly.

use crate::emitter::ActivityEmitter;
use crate::error::{ActivityEmitError, ActivityError};
use crate::settings::ActivitySettings;
use crate::sql;
use crate::types::{ActivityEvent, ActivityRow};
use async_trait::async_trait;
use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::sync::Arc;

#[derive(Clone)]
pub struct Activity {
    pool: Arc<DbPool>,
    settings: ActivitySettings,
    coalesce_window_secs: i64,
}

impl Activity {
    pub fn new(pool: Arc<DbPool>, settings: ActivitySettings, coalesce_window_secs: i64) -> Self {
        Self {
            pool,
            settings,
            coalesce_window_secs,
        }
    }

    /// Read the activity feed for one user, descending by id. `since`
    /// is the exclusive upper cursor; pass `None` for the freshest page.
    pub async fn list(
        &self,
        affected_user: &str,
        since: Option<i64>,
        limit: i64,
    ) -> Result<Vec<ActivityRow>, ActivityError> {
        let since_v = since.unwrap_or(0);
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_QM)
                    .bind(affected_user)
                    .bind(since_v)
                    .bind(since_v)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_QM)
                    .bind(affected_user)
                    .bind(since_v)
                    .bind(since_v)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_PG)
                    .bind(affected_user)
                    .bind(since_v)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    /// Delete every row with `occurred_at < cutoff`. Returns rows removed.
    pub async fn sweep_expired(&self, cutoff: i64) -> Result<u64, ActivityError> {
        let n = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(sql::DELETE_EXPIRED_QM)
                .bind(cutoff)
                .execute(p)
                .await?
                .rows_affected(),
            DbPool::MySql(p) => sqlx::query(sql::DELETE_EXPIRED_QM)
                .bind(cutoff)
                .execute(p)
                .await?
                .rows_affected(),
            DbPool::Postgres(p) => sqlx::query(sql::DELETE_EXPIRED_PG)
                .bind(cutoff)
                .execute(p)
                .await?
                .rows_affected(),
        };
        Ok(n)
    }

    async fn coalesce_probe(
        &self,
        affected_user: &str,
        actor: &str,
        event_type: &str,
        object_id: Option<i64>,
        cutoff_ts: i64,
    ) -> Result<Option<i64>, ActivityError> {
        let id = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(sql::COALESCE_PROBE_QM)
                .bind(affected_user)
                .bind(actor)
                .bind(event_type)
                .bind(object_id)
                .bind(object_id)
                .bind(cutoff_ts)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("id"))
                .transpose()?,
            DbPool::MySql(p) => sqlx::query(sql::COALESCE_PROBE_QM)
                .bind(affected_user)
                .bind(actor)
                .bind(event_type)
                .bind(object_id)
                .bind(object_id)
                .bind(cutoff_ts)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("id"))
                .transpose()?,
            DbPool::Postgres(p) => sqlx::query(sql::COALESCE_PROBE_PG)
                .bind(affected_user)
                .bind(actor)
                .bind(event_type)
                .bind(object_id)
                .bind(cutoff_ts)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("id"))
                .transpose()?,
        };
        Ok(id)
    }

    /// **subject_id staleness note:** This UPDATEs `subject_params` but
    /// preserves the stored `subject_id`. If a future caller emits a
    /// different subject_id for the same (recipient, actor, event_type,
    /// object_id) within the window, the stored subject_id won't reflect
    /// the new event. Current callers always produce stable subject_ids per
    /// (recipient, actor, event_type), so this is fine — but worth knowing
    /// if a future caller diverges.
    async fn coalesce_update(
        &self,
        id: i64,
        last_seen_at: i64,
        subject_params_json: &str,
    ) -> Result<(), ActivityError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::COALESCE_UPDATE_QM)
                    .bind(last_seen_at)
                    .bind(subject_params_json)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::COALESCE_UPDATE_QM)
                    .bind(last_seen_at)
                    .bind(subject_params_json)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::COALESCE_UPDATE_PG)
                    .bind(last_seen_at)
                    .bind(subject_params_json)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_row(
        &self,
        affected_user: &str,
        actor: &str,
        event_type: &str,
        subject_id: &str,
        subject_params_json: &str,
        object_type: &str,
        object_id: Option<i64>,
        occurred_at: i64,
    ) -> Result<i64, ActivityError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(affected_user)
                    .bind(actor)
                    .bind(event_type)
                    .bind(subject_id)
                    .bind(subject_params_json)
                    .bind(object_type)
                    .bind(object_id)
                    .bind(occurred_at)
                    .bind(occurred_at)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(affected_user)
                    .bind(actor)
                    .bind(event_type)
                    .bind(subject_id)
                    .bind(subject_params_json)
                    .bind(object_type)
                    .bind(object_id)
                    .bind(occurred_at)
                    .bind(occurred_at)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(affected_user)
                    .bind(actor)
                    .bind(event_type)
                    .bind(subject_id)
                    .bind(subject_params_json)
                    .bind(object_type)
                    .bind(object_id)
                    .bind(occurred_at)
                    .bind(occurred_at)
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }
}

#[async_trait]
impl ActivityEmitter for Activity {
    /// **Atomicity caveat:** the per-recipient loop is not transactional —
    /// each recipient gets its own SELECT+INSERT/UPDATE pair. A panic or
    /// connection loss mid-loop can leave some recipients with the row and
    /// others without. Activity is best-effort; emit failures are not
    /// retried. Callers must not wrap `emit` in a transaction expecting
    /// atomicity across recipients.
    async fn emit(&self, event: ActivityEvent) -> Result<(), ActivityEmitError> {
        // De-dupe recipients in case a caller composes the list naively.
        let mut seen = std::collections::HashSet::new();
        let unique_recipients: Vec<_> = event
            .recipients
            .into_iter()
            .filter(|u| seen.insert(u.as_str().to_string()))
            .collect();

        let subject_params_json = serde_json::to_string(&event.subject_params)
            .map_err(|e| ActivityEmitError(format!("subject_params serialize: {e}")))?;
        let event_type_str = event.event_type.as_str();
        let object_type_str = event.object_type.as_str();
        let cutoff_ts = event.occurred_at - self.coalesce_window_secs;

        for recipient in unique_recipients {
            let is_actor = recipient.as_str() == event.actor;
            if !is_actor {
                // Stream opt-out check (actor row exempt per spec §6).
                let stream = self
                    .settings
                    .stream_enabled(recipient.as_str(), event_type_str)
                    .await
                    .map_err(ActivityEmitError::from)?;
                if !stream {
                    continue;
                }
            }

            let subject_id = if is_actor {
                &event.subject_id_actor
            } else {
                &event.subject_id_recipient
            };

            if self.coalesce_window_secs > 0 {
                if let Some(id) = self
                    .coalesce_probe(
                        recipient.as_str(),
                        &event.actor,
                        event_type_str,
                        event.object_id,
                        cutoff_ts,
                    )
                    .await
                    .map_err(ActivityEmitError::from)?
                {
                    self.coalesce_update(id, event.occurred_at, &subject_params_json)
                        .await
                        .map_err(ActivityEmitError::from)?;
                    continue;
                }
            }

            self.insert_row(
                recipient.as_str(),
                &event.actor,
                event_type_str,
                subject_id,
                &subject_params_json,
                object_type_str,
                event.object_id,
                event.occurred_at,
            )
            .await
            .map_err(ActivityEmitError::from)?;
        }

        Ok(())
    }
}

// -- Per-dialect row decoders (mirrors `crabcloud-versions::service`).

fn row_from_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<ActivityRow, ActivityError> {
    let subject_params_str: String = r.try_get("subject_params")?;
    let subject_params: serde_json::Value =
        serde_json::from_str(&subject_params_str).unwrap_or(serde_json::Value::Null);
    Ok(ActivityRow {
        id: r.try_get("id")?,
        affected_user: r.try_get("affected_user")?,
        actor: r.try_get("actor")?,
        event_type: r.try_get("event_type")?,
        subject_id: r.try_get("subject_id")?,
        subject_params,
        object_type: r.try_get("object_type")?,
        object_id: r.try_get("object_id")?,
        occurred_at: r.try_get("occurred_at")?,
        last_seen_at: r.try_get("last_seen_at")?,
        count: r.try_get::<i32, _>("count")?,
    })
}

fn row_from_mysql(r: sqlx::mysql::MySqlRow) -> Result<ActivityRow, ActivityError> {
    let subject_params_str: String = r.try_get("subject_params")?;
    let subject_params: serde_json::Value =
        serde_json::from_str(&subject_params_str).unwrap_or(serde_json::Value::Null);
    Ok(ActivityRow {
        id: r.try_get("id")?,
        affected_user: r.try_get("affected_user")?,
        actor: r.try_get("actor")?,
        event_type: r.try_get("event_type")?,
        subject_id: r.try_get("subject_id")?,
        subject_params,
        object_type: r.try_get("object_type")?,
        object_id: r.try_get("object_id")?,
        occurred_at: r.try_get("occurred_at")?,
        last_seen_at: r.try_get("last_seen_at")?,
        count: r.try_get::<i32, _>("count")?,
    })
}

fn row_from_postgres(r: sqlx::postgres::PgRow) -> Result<ActivityRow, ActivityError> {
    let subject_params_str: String = r.try_get("subject_params")?;
    let subject_params: serde_json::Value =
        serde_json::from_str(&subject_params_str).unwrap_or(serde_json::Value::Null);
    Ok(ActivityRow {
        id: r.try_get("id")?,
        affected_user: r.try_get("affected_user")?,
        actor: r.try_get("actor")?,
        event_type: r.try_get("event_type")?,
        subject_id: r.try_get("subject_id")?,
        subject_params,
        object_type: r.try_get("object_type")?,
        object_id: r.try_get("object_id")?,
        occurred_at: r.try_get("occurred_at")?,
        last_seen_at: r.try_get("last_seen_at")?,
        count: r.try_get::<i32, _>("count")?,
    })
}
