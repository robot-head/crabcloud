//! `oc_filelocks` — exclusive WebDAV locks. SP5 ships exclusive scope only.
//!
//! Keyed by `"files/{uid}/{path}"`. TTL is unix-ts; expired rows persist until
//! a future `crabcloud locks:gc` reaps them. `acquire` upserts, overwriting any
//! stale row.

use crabcloud_db::DbPool;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{FileCacheError, FileCacheResult};

pub struct LockStore {
    pool: DbPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRow {
    pub key: String,
    pub ttl: i64,
    pub token: String,
    pub scope: String,
    pub depth: String,
    pub owner: Option<String>,
}

/// SELECT shape for `oc_filelocks.current`: `(key, ttl, token, scope, depth, owner)`.
type LockRowTuple = (
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl LockStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Return the current lock for `key` if it exists AND is unexpired.
    /// Returns `None` if no row or if `ttl <= now`.
    pub async fn current(&self, key: &str) -> FileCacheResult<Option<LockRow>> {
        let n = now_unix();
        let row: Option<LockRowTuple> = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query_as(
                "SELECT key, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE key = ? AND (ttl = 0 OR ttl > ?)",
            )
            .bind(key)
            .bind(n)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::MySql(p) => sqlx::query_as(
                "SELECT `key`, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE `key` = ? AND (ttl = 0 OR ttl > ?)",
            )
            .bind(key)
            .bind(n)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::Postgres(p) => sqlx::query_as(
                "SELECT key, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE key = $1 AND (ttl = 0 OR ttl > $2)",
            )
            .bind(key)
            .bind(n as i32)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
        };
        Ok(row.map(|(key, ttl, token, scope, depth, owner)| LockRow {
            key,
            ttl,
            token: token.unwrap_or_default(),
            scope: scope.unwrap_or_else(|| "exclusive".into()),
            depth: depth.unwrap_or_else(|| "0".into()),
            owner,
        }))
    }

    /// Acquire a lock. Upserts: stale rows for the same key get overwritten.
    pub async fn acquire(
        &self,
        key: &str,
        token: &str,
        scope: &str,
        depth: &str,
        owner: Option<&str>,
        ttl: i64,
    ) -> FileCacheResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (key, ttl, lock, token, scope, depth, owner) \
                     VALUES (?, ?, -1, ?, ?, ?, ?) \
                     ON CONFLICT(key) DO UPDATE SET \
                       ttl = excluded.ttl, lock = -1, token = excluded.token, \
                       scope = excluded.scope, depth = excluded.depth, owner = excluded.owner",
                )
                .bind(key)
                .bind(ttl)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (`key`, ttl, `lock`, token, scope, depth, owner) \
                     VALUES (?, ?, -1, ?, ?, ?, ?) \
                     ON DUPLICATE KEY UPDATE \
                       ttl = VALUES(ttl), `lock` = -1, token = VALUES(token), \
                       scope = VALUES(scope), depth = VALUES(depth), owner = VALUES(owner)",
                )
                .bind(key)
                .bind(ttl)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (key, ttl, lock, token, scope, depth, owner) \
                     VALUES ($1, $2, -1, $3, $4, $5, $6) \
                     ON CONFLICT (key) DO UPDATE SET \
                       ttl = EXCLUDED.ttl, lock = -1, token = EXCLUDED.token, \
                       scope = EXCLUDED.scope, depth = EXCLUDED.depth, owner = EXCLUDED.owner",
                )
                .bind(key)
                .bind(ttl as i32)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
        }
        Ok(())
    }

    /// Release a lock by `(key, token)`. Returns `true` if a row was deleted,
    /// `false` if no such row.
    pub async fn release(&self, key: &str, token: &str) -> FileCacheResult<bool> {
        let rows_affected = match &self.pool {
            DbPool::Sqlite(p) => {
                sqlx::query("DELETE FROM oc_filelocks WHERE key = ? AND token = ?")
                    .bind(key)
                    .bind(token)
                    .execute(p)
                    .await
                    .map_err(FileCacheError::Db)?
                    .rows_affected()
            }
            DbPool::MySql(p) => {
                sqlx::query("DELETE FROM oc_filelocks WHERE `key` = ? AND token = ?")
                    .bind(key)
                    .bind(token)
                    .execute(p)
                    .await
                    .map_err(FileCacheError::Db)?
                    .rows_affected()
            }
            DbPool::Postgres(p) => {
                sqlx::query("DELETE FROM oc_filelocks WHERE key = $1 AND token = $2")
                    .bind(key)
                    .bind(token)
                    .execute(p)
                    .await
                    .map_err(FileCacheError::Db)?
                    .rows_affected()
            }
        };
        Ok(rows_affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("l.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn acquire_then_current_returns_lock() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire(
                "files/alice/a.txt",
                "urn:uuid:t1",
                "exclusive",
                "0",
                Some("alice"),
                ttl,
            )
            .await
            .unwrap();
        let lock = store.current("files/alice/a.txt").await.unwrap().unwrap();
        assert_eq!(lock.token, "urn:uuid:t1");
        assert_eq!(lock.scope, "exclusive");
        assert_eq!(lock.depth, "0");
    }

    #[tokio::test]
    async fn current_returns_none_for_absent() {
        let store = LockStore::new(fresh_pool().await);
        assert!(store.current("files/ghost/x").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn current_returns_none_for_expired() {
        let store = LockStore::new(fresh_pool().await);
        let ttl_past = now_unix() - 10;
        store
            .acquire(
                "files/alice/a",
                "urn:uuid:t",
                "exclusive",
                "0",
                None,
                ttl_past,
            )
            .await
            .unwrap();
        assert!(store.current("files/alice/a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_correct_token_succeeds() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire("files/alice/a", "urn:uuid:t", "exclusive", "0", None, ttl)
            .await
            .unwrap();
        let ok = store.release("files/alice/a", "urn:uuid:t").await.unwrap();
        assert!(ok);
        assert!(store.current("files/alice/a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_wrong_token_fails() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire("files/alice/a", "urn:uuid:t1", "exclusive", "0", None, ttl)
            .await
            .unwrap();
        let ok = store
            .release("files/alice/a", "urn:uuid:other")
            .await
            .unwrap();
        assert!(!ok);
        // Lock still present.
        assert!(store.current("files/alice/a").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn acquire_overwrites_expired_row() {
        let store = LockStore::new(fresh_pool().await);
        let past = now_unix() - 10;
        store
            .acquire("files/a", "urn:uuid:old", "exclusive", "0", None, past)
            .await
            .unwrap();
        // New lock with the same key but a different token.
        let future = now_unix() + 1800;
        store
            .acquire("files/a", "urn:uuid:new", "exclusive", "0", None, future)
            .await
            .unwrap();
        let lock = store.current("files/a").await.unwrap().unwrap();
        assert_eq!(lock.token, "urn:uuid:new");
    }
}
