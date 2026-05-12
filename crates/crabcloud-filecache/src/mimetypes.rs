//! `oc_mimetypes` interning. Each mimetype string (full mimetype AND its
//! type-half) becomes a row; subsequent inserts re-use the existing id.
//! Per-process intern cache (`DashMap`) avoids redundant DB hits.
//!
//! Batch A omits the in-flight-transaction handle that Batch B will
//! introduce — see the parent design doc. The non-transactional path here
//! is sufficient for Batch A's correctness (id stability + cache reuse).

use crabcloud_db::DbPool;
use dashmap::DashMap;

use crate::error::{FileCacheError, FileCacheResult};

/// Look up or insert a mimetype row, returning its `id`. Consults the
/// per-process intern cache first; falls back to a per-dialect upsert.
pub async fn intern_mimetype(
    pool: &DbPool,
    cache: &DashMap<String, i64>,
    mimetype: &str,
) -> FileCacheResult<i64> {
    if let Some(id) = cache.get(mimetype) {
        return Ok(*id);
    }
    let id = upsert_mimetype(pool, mimetype).await?;
    cache.insert(mimetype.to_string(), id);
    Ok(id)
}

async fn upsert_mimetype(pool: &DbPool, mimetype: &str) -> FileCacheResult<i64> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("INSERT OR IGNORE INTO oc_mimetypes (mimetype) VALUES (?)")
                .bind(mimetype)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: i64 = sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = ?")
                .bind(mimetype)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id)
        }
        DbPool::MySql(p) => {
            sqlx::query("INSERT IGNORE INTO oc_mimetypes (mimetype) VALUES (?)")
                .bind(mimetype)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: u64 = sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = ?")
                .bind(mimetype)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
        DbPool::Postgres(p) => {
            sqlx::query(
                "INSERT INTO oc_mimetypes (mimetype) VALUES ($1) ON CONFLICT (mimetype) DO NOTHING",
            )
            .bind(mimetype)
            .execute(p)
            .await
            .map_err(FileCacheError::Db)?;
            let id: i32 = sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = $1")
                .bind(mimetype)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
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
        let cfg = minimal_sqlite_config(dir.path().join("m.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn intern_mimetype_returns_stable_id() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let a = intern_mimetype(&pool, &cache, "image/png").await.unwrap();
        let b = intern_mimetype(&pool, &cache, "image/png").await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn intern_mimetype_uses_cache_on_repeat() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        intern_mimetype(&pool, &cache, "image/png").await.unwrap();
        assert_eq!(cache.len(), 1);
        intern_mimetype(&pool, &cache, "image/png").await.unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn intern_distinct_mimetypes_get_distinct_ids() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let png = intern_mimetype(&pool, &cache, "image/png").await.unwrap();
        let txt = intern_mimetype(&pool, &cache, "text/plain").await.unwrap();
        assert_ne!(png, txt);
    }
}
