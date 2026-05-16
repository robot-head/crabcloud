//! Sweeper-support queries on `Shares`. Extracted from `service.rs` so
//! the core CRUD file isn't carrying scheduler-specific code paths.
//!
//! `find_expiring_links` + `stamp_last_warned` are the two operations
//! `crabcloud-core::ExpirationWarningSweeper` calls each tick to drive
//! T-1 day link-expiration warnings. SP11/C5.
//!
//! The `ExpiringLink` projection type itself lives in `crate::types`
//! because it's re-exported from the crate root (consumed by
//! `crabcloud-core`); keeping it there preserves the public API.

use chrono::NaiveDateTime;
use crabcloud_db::DbPool;
use sqlx::Row as _;

use super::Shares;
use crate::error::ShareError;
use crate::sql;
use crate::types::ExpiringLink;

impl Shares {
    /// Select link / email-link rows whose `expiration` falls inside
    /// the window `(now, now + 24h]` and have not yet been warned.
    /// Used by `ExpirationWarningSweeper` in `crabcloud-core`. Returns
    /// at most `limit` rows ordered by id.
    pub async fn find_expiring_links(
        &self,
        now: NaiveDateTime,
        until: NaiveDateTime,
        limit: i64,
    ) -> Result<Vec<ExpiringLink>, ShareError> {
        let mut out = Vec::new();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::SELECT_EXPIRING_LINKS_QM)
                    .bind(now)
                    .bind(until)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                for row in rows {
                    out.push(ExpiringLink {
                        id: row.try_get("id")?,
                        uid_owner: row.try_get("uid_owner")?,
                        file_target: row.try_get("file_target")?,
                        token: row.try_get("token")?,
                        expiration: row.try_get("expiration")?,
                    });
                }
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::SELECT_EXPIRING_LINKS_QM)
                    .bind(now)
                    .bind(until)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                for row in rows {
                    out.push(ExpiringLink {
                        id: row.try_get("id")?,
                        uid_owner: row.try_get_unchecked("uid_owner")?,
                        file_target: row.try_get_unchecked("file_target")?,
                        token: row.try_get_unchecked("token")?,
                        expiration: row.try_get_unchecked("expiration")?,
                    });
                }
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::SELECT_EXPIRING_LINKS_PG)
                    .bind(now)
                    .bind(until)
                    .bind(limit)
                    .fetch_all(p)
                    .await?;
                for row in rows {
                    out.push(ExpiringLink {
                        id: row.try_get("id")?,
                        uid_owner: row.try_get("uid_owner")?,
                        file_target: row.try_get("file_target")?,
                        token: row.try_get("token")?,
                        expiration: row.try_get("expiration")?,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Stamp `last_warned = now` on a share row regardless of whether
    /// the user opted in. The sweeper does this *after* attempting the
    /// enqueue so an opted-out user is not re-considered next sweep.
    pub async fn stamp_last_warned(&self, id: i64, when: NaiveDateTime) -> Result<(), ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::STAMP_LAST_WARNED_QM)
                    .bind(when)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::STAMP_LAST_WARNED_QM)
                    .bind(when)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::STAMP_LAST_WARNED_PG)
                    .bind(when)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }
}
