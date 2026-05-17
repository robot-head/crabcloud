//! `ActivitySettings` — per-user-per-event stream toggle storage.
//!
//! Default `stream = true` when no row exists. `stream_enabled` is one row;
//! `get_all_for_user` returns every set toggle. `set` is an upsert.

use crate::error::ActivityError;
use crate::sql;
use crate::types::ActivitySetting;
use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::sync::Arc;

#[derive(Clone)]
pub struct ActivitySettings {
    pool: Arc<DbPool>,
}

impl ActivitySettings {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    /// Returns the stream toggle for `(user_id, event_type)`. Missing
    /// rows default to `true`.
    pub async fn stream_enabled(
        &self,
        user_id: &str,
        event_type: &str,
    ) -> Result<bool, ActivityError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::SETTINGS_GET_QM)
                    .bind(user_id)
                    .bind(event_type)
                    .fetch_optional(p)
                    .await?;
                match row {
                    Some(r) => Ok(r.try_get::<bool, _>("stream")?),
                    None => Ok(true),
                }
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::SETTINGS_GET_QM)
                    .bind(user_id)
                    .bind(event_type)
                    .fetch_optional(p)
                    .await?;
                match row {
                    Some(r) => Ok(r.try_get::<bool, _>("stream")?),
                    None => Ok(true),
                }
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::SETTINGS_GET_PG)
                    .bind(user_id)
                    .bind(event_type)
                    .fetch_optional(p)
                    .await?;
                match row {
                    Some(r) => Ok(r.try_get::<bool, _>("stream")?),
                    None => Ok(true),
                }
            }
        }
    }

    pub async fn get_all_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<ActivitySetting>, ActivityError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::SETTINGS_GET_ALL_QM)
                    .bind(user_id)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(|r| {
                        Ok(ActivitySetting {
                            event_type: r.try_get("event_type")?,
                            stream: r.try_get::<bool, _>("stream")?,
                        })
                    })
                    .collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::SETTINGS_GET_ALL_QM)
                    .bind(user_id)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(|r| {
                        Ok(ActivitySetting {
                            event_type: r.try_get("event_type")?,
                            stream: r.try_get::<bool, _>("stream")?,
                        })
                    })
                    .collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::SETTINGS_GET_ALL_PG)
                    .bind(user_id)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(|r| {
                        Ok(ActivitySetting {
                            event_type: r.try_get("event_type")?,
                            stream: r.try_get::<bool, _>("stream")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub async fn set(
        &self,
        user_id: &str,
        event_type: &str,
        stream: bool,
    ) -> Result<(), ActivityError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::SETTINGS_UPSERT_SQLITE)
                    .bind(user_id)
                    .bind(event_type)
                    .bind(stream)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::SETTINGS_UPSERT_MYSQL)
                    .bind(user_id)
                    .bind(event_type)
                    .bind(stream)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::SETTINGS_UPSERT_PG)
                    .bind(user_id)
                    .bind(event_type)
                    .bind(stream)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }
}
