//! `oc_storages` interning. Each `Storage::id()` string becomes a row;
//! subsequent inserts re-use the existing `numeric_id`. Per-process intern
//! cache (`DashMap`) avoids redundant DB hits.

use crabcloud_db::DbPool;
use dashmap::DashMap;

use crate::error::{FileCacheError, FileCacheResult};

pub async fn intern_storage(
    pool: &DbPool,
    cache: &DashMap<String, i64>,
    storage_id: &str,
) -> FileCacheResult<i64> {
    if let Some(id) = cache.get(storage_id) {
        return Ok(*id);
    }
    let id = upsert_storage(pool, storage_id).await?;
    cache.insert(storage_id.to_string(), id);
    Ok(id)
}

async fn upsert_storage(pool: &DbPool, storage_id: &str) -> FileCacheResult<i64> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("INSERT OR IGNORE INTO oc_storages (id) VALUES (?)")
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: i64 = sqlx::query_scalar("SELECT numeric_id FROM oc_storages WHERE id = ?")
                .bind(storage_id)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id)
        }
        DbPool::MySql(p) => {
            sqlx::query("INSERT IGNORE INTO oc_storages (id) VALUES (?)")
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: u32 = sqlx::query_scalar("SELECT numeric_id FROM oc_storages WHERE id = ?")
                .bind(storage_id)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
        DbPool::Postgres(p) => {
            sqlx::query("INSERT INTO oc_storages (id) VALUES ($1) ON CONFLICT (id) DO NOTHING")
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: i32 = sqlx::query_scalar("SELECT numeric_id FROM oc_storages WHERE id = $1")
                .bind(storage_id)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
    }
}

/// Update `oc_storages.last_checked` to the current unix timestamp.
/// Idempotent; called at the end of `Scanner::full_scan` (Batch D).
pub async fn stamp_last_checked(pool: &DbPool, storage_id: &str) -> FileCacheResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = ? WHERE id = ?")
                .bind(now)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = ? WHERE id = ?")
                .bind(now)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = $1 WHERE id = $2")
                .bind(now as i32)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("s.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn intern_storage_returns_stable_id() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let a = intern_storage(&pool, &cache, "local::/srv/data/alice")
            .await
            .unwrap();
        let b = intern_storage(&pool, &cache, "local::/srv/data/alice")
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn intern_distinct_storages_get_distinct_ids() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let alice = intern_storage(&pool, &cache, "local::/srv/data/alice")
            .await
            .unwrap();
        let bob = intern_storage(&pool, &cache, "local::/srv/data/bob")
            .await
            .unwrap();
        assert_ne!(alice, bob);
    }

    #[tokio::test]
    async fn stamp_last_checked_updates_row() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        intern_storage(&pool, &cache, "local::/x").await.unwrap();
        stamp_last_checked(&pool, "local::/x").await.unwrap();
        let DbPool::Sqlite(p) = &pool else { panic!() };
        let lc: Option<i64> =
            sqlx::query_scalar("SELECT last_checked FROM oc_storages WHERE id = ?")
                .bind("local::/x")
                .fetch_one(p)
                .await
                .unwrap();
        assert!(lc.is_some());
    }
}
