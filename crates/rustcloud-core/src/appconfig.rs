//! Runtime app-config service backed by `oc_appconfig` + a write-through cache.
//!
//! Schema-compatible with Nextcloud (spec §5.2). Reads check cache first;
//! misses fall through to DB and prime the cache. Writes go to DB then
//! invalidate the cache key.

use crate::error::{CoreResult, Error};
use rustcloud_cache::Cache;
use rustcloud_db::DbPool;
use std::sync::Arc;

/// Per-key TTL for the `oc_appconfig` write-through cache. Short enough that
/// admin-UI changes propagate within a minute; long enough that hot reads are
/// amortized.
const APPCONFIG_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone)]
pub struct AppConfigService {
    pool: DbPool,
    cache: Arc<dyn Cache>,
    table: String,
    instance_id: String,
}

impl std::fmt::Debug for AppConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfigService")
            .field("table", &self.table)
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

impl AppConfigService {
    pub fn new(pool: DbPool, cache: Arc<dyn Cache>, prefix: &str, instance_id: &str) -> Self {
        Self {
            pool,
            cache,
            table: format!("{prefix}appconfig"),
            instance_id: instance_id.to_string(),
        }
    }

    fn cache_key(&self, appid: &str, key: &str) -> String {
        format!("{}:appconfig:{appid}:{key}", self.instance_id)
    }

    pub async fn get(&self, appid: &str, key: &str) -> CoreResult<Option<String>> {
        let ck = self.cache_key(appid, key);
        if let Some(bytes) = self.cache.get(&ck).await? {
            // Empty bytes = sentinel for "known missing"
            if bytes.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
        let v = self.fetch_db(appid, key).await?;
        let sentinel: &[u8] = match &v {
            Some(s) => s.as_bytes(),
            None => &[],
        };
        if let Err(e) = self
            .cache
            .set(&ck, sentinel, Some(APPCONFIG_CACHE_TTL))
            .await
        {
            tracing::warn!(error = %e, appid, key, "failed to write appconfig cache");
        }
        Ok(v)
    }

    pub async fn set(&self, appid: &str, key: &str, value: &str) -> CoreResult<()> {
        self.write_db(appid, key, value).await?;
        // Invalidate; next read will repopulate.
        if let Err(e) = self.cache.del(&self.cache_key(appid, key)).await {
            tracing::warn!(error = %e, appid, key, "failed to invalidate appconfig cache");
        }
        Ok(())
    }

    async fn fetch_db(&self, appid: &str, key: &str) -> CoreResult<Option<String>> {
        let select_q = match &self.pool {
            DbPool::Postgres(_) => format!(
                "SELECT configvalue FROM {} WHERE appid = $1 AND configkey = $2",
                self.table
            ),
            _ => format!(
                "SELECT configvalue FROM {} WHERE appid = ? AND configkey = ?",
                self.table
            ),
        };
        let row: Option<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
            DbPool::MySql(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
            DbPool::Postgres(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
        };
        Ok(row.map(|(v,)| v))
    }

    async fn write_db(&self, appid: &str, key: &str, value: &str) -> CoreResult<()> {
        // UPSERT — dialect-specific.
        match &self.pool {
            DbPool::Sqlite(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES (?, ?, ?) \
                     ON CONFLICT(appid, configkey) DO UPDATE SET configvalue = excluded.configvalue",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
            DbPool::MySql(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES (?, ?, ?) \
                     ON DUPLICATE KEY UPDATE configvalue = VALUES(configvalue)",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
            DbPool::Postgres(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES ($1, $2, $3) \
                     ON CONFLICT (appid, configkey) DO UPDATE SET configvalue = EXCLUDED.configvalue",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;
    use rustcloud_config::test_support::minimal_sqlite_config;
    use rustcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh() -> AppConfigService {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("ac.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        // Keep dir alive in test by leaking — small leak in tests is fine.
        std::mem::forget(dir);
        AppConfigService::new(pool, cache, &cfg.dbtableprefix, &cfg.instanceid)
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let ac = fresh().await;
        assert_eq!(ac.get("files", "no-such-key").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let ac = fresh().await;
        ac.set("files", "max_upload", "1024").await.unwrap();
        assert_eq!(
            ac.get("files", "max_upload").await.unwrap(),
            Some("1024".to_string())
        );
    }

    #[tokio::test]
    async fn set_upserts_existing_key() {
        let ac = fresh().await;
        ac.set("files", "max_upload", "1024").await.unwrap();
        ac.set("files", "max_upload", "2048").await.unwrap();
        assert_eq!(
            ac.get("files", "max_upload").await.unwrap(),
            Some("2048".to_string())
        );
    }

    #[tokio::test]
    async fn cache_is_used_on_second_read() {
        let ac = fresh().await;
        ac.set("files", "k", "v").await.unwrap();
        let _ = ac.get("files", "k").await.unwrap(); // populates cache
                                                     // Mutate DB directly to verify next read hits cache, not DB.
        let direct_q = "UPDATE oc_appconfig SET configvalue = 'BYPASSED' WHERE appid = 'files' AND configkey = 'k'";
        if let DbPool::Sqlite(p) = &ac.pool {
            sqlx::query(direct_q).execute(p).await.unwrap();
        }
        // Cache still returns the original.
        assert_eq!(ac.get("files", "k").await.unwrap(), Some("v".to_string()));
    }

    #[tokio::test]
    async fn missing_key_is_cached_as_sentinel() {
        let ac = fresh().await;
        // First miss: hits DB.
        assert_eq!(ac.get("files", "absent").await.unwrap(), None);
        // Insert directly into DB; the cached miss-sentinel should still hide it.
        let direct_q = "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES ('files', 'absent', 'sneaky')";
        if let DbPool::Sqlite(p) = &ac.pool {
            sqlx::query(direct_q).execute(p).await.unwrap();
        }
        assert_eq!(ac.get("files", "absent").await.unwrap(), None);
    }
}
