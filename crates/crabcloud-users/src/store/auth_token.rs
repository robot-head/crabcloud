//! `TokenStore` — async trait for `oc_authtoken` CRUD + lifecycle.
//! `SqlTokenStore` body lands in Task 4. `TokenAuthCache` lands in Task 5
//! (Batch B).

use crate::auth_token::AuthToken;
use crate::error::UsersResult;
use crate::user::UserId;
use async_trait::async_trait;
use crabcloud_db::DbPool;

#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Insert a fresh row. Returns the new row id.
    async fn create(&self, row: &AuthToken) -> UsersResult<i64>;

    /// Look up by hash. Returns `None` on miss (NOT an error).
    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>>;

    /// Look up by primary key. Returns `None` on miss.
    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>>;

    /// All rows for `uid`, newest-`last_activity`-first.
    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>>;

    /// Set `last_activity = last_check = now`. Best-effort: a missing row
    /// is silently ignored to avoid failing an otherwise-successful auth.
    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()>;

    /// Delete by id. Idempotent (deleting an absent row is fine).
    async fn revoke(&self, id: i64) -> UsersResult<()>;

    /// Delete every row owned by `uid`.
    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()>;

    /// Delete every row owned by `uid` except `except`.
    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()>;

    /// Set `password_invalid = 1` on every row owned by `uid`.
    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()>;
}

#[derive(Clone)]
pub struct SqlTokenStore {
    pool: DbPool,
}

impl SqlTokenStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

use crate::auth_token::AuthTokenType;
use crate::error::UsersError;
use crabcloud_db::DbError;
use sqlx::Row as _;

fn map_sqlx<T>(r: Result<T, sqlx::Error>) -> UsersResult<T> {
    r.map_err(|e| UsersError::Db(DbError::Sqlx(e)))
}

/// Per-dialect row decoders. sqlx tuple `FromRow` only supports up to 16
/// elements, but `oc_authtoken` has 17 columns — so we decode manually
/// against each dialect's `Row` type.
fn decode_sqlite(row: &sqlx::sqlite::SqliteRow) -> UsersResult<AuthToken> {
    Ok(AuthToken {
        id: map_sqlx(row.try_get::<i64, _>("id"))?,
        uid: UserId::new(map_sqlx(row.try_get::<String, _>("uid"))?)?,
        login_name: map_sqlx(row.try_get::<String, _>("login_name"))?,
        password: map_sqlx(row.try_get::<Option<String>, _>("password"))?,
        name: map_sqlx(row.try_get::<String, _>("name"))?,
        token: map_sqlx(row.try_get::<String, _>("token"))?,
        kind: AuthTokenType::from_i32(map_sqlx(row.try_get::<i64, _>("type"))? as i32)?,
        remember: map_sqlx(row.try_get::<i64, _>("remember"))? != 0,
        last_activity: map_sqlx(row.try_get::<i64, _>("last_activity"))?.max(0) as u64,
        last_check: map_sqlx(row.try_get::<i64, _>("last_check"))?.max(0) as u64,
        public_key: map_sqlx(row.try_get::<Option<String>, _>("public_key"))?,
        private_key: map_sqlx(row.try_get::<Option<String>, _>("private_key"))?,
        version: map_sqlx(row.try_get::<i64, _>("version"))? as i32,
        scope: map_sqlx(row.try_get::<Option<String>, _>("scope"))?,
        expires: map_sqlx(row.try_get::<Option<i64>, _>("expires"))?.map(|e| e.max(0) as u64),
        password_invalid: map_sqlx(row.try_get::<i64, _>("password_invalid"))? != 0,
        remote_wipe: map_sqlx(row.try_get::<i64, _>("remote_wipe"))? != 0,
    })
}

fn decode_mysql(row: &sqlx::mysql::MySqlRow) -> UsersResult<AuthToken> {
    Ok(AuthToken {
        id: map_sqlx(row.try_get::<i64, _>("id"))?,
        uid: UserId::new(map_sqlx(row.try_get::<String, _>("uid"))?)?,
        login_name: map_sqlx(row.try_get::<String, _>("login_name"))?,
        password: map_sqlx(row.try_get::<Option<String>, _>("password"))?,
        name: map_sqlx(row.try_get::<String, _>("name"))?,
        token: map_sqlx(row.try_get::<String, _>("token"))?,
        kind: AuthTokenType::from_i32(map_sqlx(row.try_get::<i16, _>("type"))? as i32)?,
        remember: map_sqlx(row.try_get::<i8, _>("remember"))? != 0,
        last_activity: map_sqlx(row.try_get::<i64, _>("last_activity"))?.max(0) as u64,
        last_check: map_sqlx(row.try_get::<i64, _>("last_check"))?.max(0) as u64,
        public_key: map_sqlx(row.try_get::<Option<String>, _>("public_key"))?,
        private_key: map_sqlx(row.try_get::<Option<String>, _>("private_key"))?,
        version: map_sqlx(row.try_get::<i16, _>("version"))? as i32,
        scope: map_sqlx(row.try_get::<Option<String>, _>("scope"))?,
        expires: map_sqlx(row.try_get::<Option<i64>, _>("expires"))?.map(|e| e.max(0) as u64),
        password_invalid: map_sqlx(row.try_get::<i8, _>("password_invalid"))? != 0,
        remote_wipe: map_sqlx(row.try_get::<i8, _>("remote_wipe"))? != 0,
    })
}

fn decode_postgres(row: &sqlx::postgres::PgRow) -> UsersResult<AuthToken> {
    Ok(AuthToken {
        id: map_sqlx(row.try_get::<i64, _>("id"))?,
        uid: UserId::new(map_sqlx(row.try_get::<String, _>("uid"))?)?,
        login_name: map_sqlx(row.try_get::<String, _>("login_name"))?,
        password: map_sqlx(row.try_get::<Option<String>, _>("password"))?,
        name: map_sqlx(row.try_get::<String, _>("name"))?,
        token: map_sqlx(row.try_get::<String, _>("token"))?,
        kind: AuthTokenType::from_i32(map_sqlx(row.try_get::<i16, _>("type"))? as i32)?,
        remember: map_sqlx(row.try_get::<i16, _>("remember"))? != 0,
        last_activity: map_sqlx(row.try_get::<i64, _>("last_activity"))?.max(0) as u64,
        last_check: map_sqlx(row.try_get::<i64, _>("last_check"))?.max(0) as u64,
        public_key: map_sqlx(row.try_get::<Option<String>, _>("public_key"))?,
        private_key: map_sqlx(row.try_get::<Option<String>, _>("private_key"))?,
        version: map_sqlx(row.try_get::<i16, _>("version"))? as i32,
        scope: map_sqlx(row.try_get::<Option<String>, _>("scope"))?,
        expires: map_sqlx(row.try_get::<Option<i64>, _>("expires"))?.map(|e| e.max(0) as u64),
        password_invalid: map_sqlx(row.try_get::<i16, _>("password_invalid"))? != 0,
        remote_wipe: map_sqlx(row.try_get::<i16, _>("remote_wipe"))? != 0,
    })
}

const SELECT_COLUMNS: &str = "id, uid, login_name, password, name, token, type, remember, \
     last_activity, last_check, public_key, private_key, version, scope, \
     expires, password_invalid, remote_wipe";

#[async_trait]
impl TokenStore for SqlTokenStore {
    async fn create(&self, row: &AuthToken) -> UsersResult<i64> {
        let kind_int: i64 = row.kind.as_i32() as i64;
        let remember_int: i64 = if row.remember { 1 } else { 0 };
        let last_activity: i64 = row.last_activity as i64;
        let last_check: i64 = row.last_check as i64;
        let version: i64 = row.version as i64;
        let expires: Option<i64> = row.expires.map(|e| e as i64);
        let pi: i64 = if row.password_invalid { 1 } else { 0 };
        let rw: i64 = if row.remote_wipe { 1 } else { 0 };

        let q_sqlite_mysql = "INSERT INTO oc_authtoken \
            (uid, login_name, password, name, token, type, remember, last_activity, last_check, \
             public_key, private_key, version, scope, expires, password_invalid, remote_wipe) \
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        let q_pg = "INSERT INTO oc_authtoken \
            (uid, login_name, password, name, token, type, remember, last_activity, last_check, \
             public_key, private_key, version, scope, expires, password_invalid, remote_wipe) \
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id";

        let id: i64 = match &self.pool {
            DbPool::Sqlite(p) => {
                let res = map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .execute(p)
                        .await,
                )?;
                res.last_insert_rowid()
            }
            DbPool::MySql(p) => {
                let res = map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .execute(p)
                        .await,
                )?;
                res.last_insert_id() as i64
            }
            DbPool::Postgres(p) => {
                let row: (i64,) = map_sqlx(
                    sqlx::query_as(q_pg)
                        .bind(row.uid.as_str())
                        .bind(&row.login_name)
                        .bind(row.password.as_deref())
                        .bind(&row.name)
                        .bind(&row.token)
                        .bind(kind_int)
                        .bind(remember_int)
                        .bind(last_activity)
                        .bind(last_check)
                        .bind(row.public_key.as_deref())
                        .bind(row.private_key.as_deref())
                        .bind(version)
                        .bind(row.scope.as_deref())
                        .bind(expires)
                        .bind(pi)
                        .bind(rw)
                        .fetch_one(p)
                        .await,
                )?;
                row.0
            }
        };
        Ok(id)
    }

    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>> {
        let q_sqlite_mysql = format!("SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE token = ?");
        let q_pg = format!("SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE token = $1");
        match &self.pool {
            DbPool::Sqlite(p) => {
                let row = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(hash)
                        .fetch_optional(p)
                        .await,
                )?;
                row.as_ref().map(decode_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(hash)
                        .fetch_optional(p)
                        .await,
                )?;
                row.as_ref().map(decode_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = map_sqlx(sqlx::query(&q_pg).bind(hash).fetch_optional(p).await)?;
                row.as_ref().map(decode_postgres).transpose()
            }
        }
    }

    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        let q_sqlite_mysql = format!("SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE id = ?");
        let q_pg = format!("SELECT {SELECT_COLUMNS} FROM oc_authtoken WHERE id = $1");
        match &self.pool {
            DbPool::Sqlite(p) => {
                let row = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(id)
                        .fetch_optional(p)
                        .await,
                )?;
                row.as_ref().map(decode_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(id)
                        .fetch_optional(p)
                        .await,
                )?;
                row.as_ref().map(decode_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = map_sqlx(sqlx::query(&q_pg).bind(id).fetch_optional(p).await)?;
                row.as_ref().map(decode_postgres).transpose()
            }
        }
    }

    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        let q_sqlite_mysql = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken \
             WHERE uid = ? ORDER BY last_activity DESC, id DESC"
        );
        let q_pg = format!(
            "SELECT {SELECT_COLUMNS} FROM oc_authtoken \
             WHERE uid = $1 ORDER BY last_activity DESC, id DESC"
        );
        match &self.pool {
            DbPool::Sqlite(p) => {
                let rows = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(uid.as_str())
                        .fetch_all(p)
                        .await,
                )?;
                rows.iter().map(decode_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = map_sqlx(
                    sqlx::query(&q_sqlite_mysql)
                        .bind(uid.as_str())
                        .fetch_all(p)
                        .await,
                )?;
                rows.iter().map(decode_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = map_sqlx(sqlx::query(&q_pg).bind(uid.as_str()).fetch_all(p).await)?;
                rows.iter().map(decode_postgres).collect()
            }
        }
    }

    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()> {
        let now_i: i64 = now as i64;
        let q_sqlite_mysql =
            "UPDATE oc_authtoken SET last_activity = ?, last_check = ? WHERE id = ?";
        let q_pg = "UPDATE oc_authtoken SET last_activity = $1, last_check = $2 WHERE id = $3";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(now_i)
                        .bind(now_i)
                        .bind(id)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(now_i)
                        .bind(now_i)
                        .bind(id)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(now_i)
                        .bind(now_i)
                        .bind(id)
                        .execute(p)
                        .await,
                )?;
            }
        }
        Ok(())
    }

    async fn revoke(&self, id: i64) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE id = ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE id = $1";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(sqlx::query(q_sqlite_mysql).bind(id).execute(p).await)?;
            }
            DbPool::MySql(p) => {
                map_sqlx(sqlx::query(q_sqlite_mysql).bind(id).execute(p).await)?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(sqlx::query(q_pg).bind(id).execute(p).await)?;
            }
        }
        Ok(())
    }

    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE uid = ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE uid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?;
            }
        }
        Ok(())
    }

    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_authtoken WHERE uid = ? AND id <> ?";
        let q_pg = "DELETE FROM oc_authtoken WHERE uid = $1 AND id <> $2";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .bind(except)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .bind(except)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(uid.as_str())
                        .bind(except)
                        .execute(p)
                        .await,
                )?;
            }
        }
        Ok(())
    }

    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "UPDATE oc_authtoken SET password_invalid = 1 WHERE uid = ?";
        let q_pg = "UPDATE oc_authtoken SET password_invalid = 1 WHERE uid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?;
            }
        }
        Ok(())
    }
}

use crabcloud_cache::Cache;
use std::sync::Arc;
use std::time::Duration;

/// Cache TTL for positive lookups (a hot row is worth ≤30s of staleness).
const TOKEN_CACHE_TTL: Duration = Duration::from_secs(30);

/// Cache TTL for negative lookups — soak up brute-force token bursts so
/// they don't hit the DB.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(5);

/// Min interval between consecutive `bump_activity` writes for the same row.
const ACTIVITY_BUMP_INTERVAL: u64 = 30;

/// Read-through cache over a [`TokenStore`]. The cache key is
/// `{instance_id}:tokens:hash:{hex}`; positive entries are the serialized
/// `AuthToken`, negative entries are an empty byte slice (sentinel). Both
/// are bounded by short TTLs so a token revoke can't be cached forever.
#[derive(Clone)]
pub struct TokenAuthCache {
    inner: Arc<dyn TokenStore>,
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl TokenAuthCache {
    pub fn new(
        inner: Arc<dyn TokenStore>,
        cache: Arc<dyn Cache>,
        instance_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            cache,
            instance_id: instance_id.into(),
        }
    }

    fn hash_key(&self, hash: &str) -> String {
        format!("{}:tokens:hash:{}", self.instance_id, hash)
    }

    fn id_key(&self, id: i64) -> String {
        format!("{}:tokens:id:{}", self.instance_id, id)
    }

    /// Lookup by hash. Reads the cache; on miss, queries the inner store,
    /// caches positive (TTL 30s) or negative (TTL 5s) and returns.
    pub async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>> {
        let key = self.hash_key(hash);
        match self.cache.get(&key).await? {
            Some(bytes) if bytes.is_empty() => return Ok(None),
            Some(bytes) => {
                let row: AuthToken = serde_json::from_slice(&bytes).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token decode: {e}"
                    )))
                })?;
                return Ok(Some(row));
            }
            None => {}
        }
        match self.inner.lookup_by_hash(hash).await? {
            Some(row) => {
                let bytes = serde_json::to_vec(&row).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token encode: {e}"
                    )))
                })?;
                self.cache.set(&key, &bytes, Some(TOKEN_CACHE_TTL)).await?;
                Ok(Some(row))
            }
            None => {
                self.cache.set(&key, &[], Some(NEGATIVE_CACHE_TTL)).await?;
                Ok(None)
            }
        }
    }

    pub async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        let key = self.id_key(id);
        match self.cache.get(&key).await? {
            Some(bytes) if bytes.is_empty() => return Ok(None),
            Some(bytes) => {
                let row: AuthToken = serde_json::from_slice(&bytes).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token decode: {e}"
                    )))
                })?;
                return Ok(Some(row));
            }
            None => {}
        }
        match self.inner.lookup_by_id(id).await? {
            Some(row) => {
                let bytes = serde_json::to_vec(&row).map_err(|e| {
                    UsersError::Cache(crabcloud_cache::CacheError::Io(format!(
                        "token encode: {e}"
                    )))
                })?;
                self.cache.set(&key, &bytes, Some(TOKEN_CACHE_TTL)).await?;
                Ok(Some(row))
            }
            None => {
                self.cache.set(&key, &[], Some(NEGATIVE_CACHE_TTL)).await?;
                Ok(None)
            }
        }
    }

    /// Conditionally bump `last_activity`. Skips the DB write if the row's
    /// cached activity is within the rate-limit interval.
    pub async fn maybe_bump_activity(&self, row: &AuthToken, now: u64) -> UsersResult<()> {
        if now < row.last_activity + ACTIVITY_BUMP_INTERVAL {
            return Ok(());
        }
        self.inner.bump_activity(row.id, now).await?;
        let _ = self.invalidate_hash(&row.token).await;
        let _ = self.invalidate_id(row.id).await;
        Ok(())
    }

    pub async fn invalidate_hash(&self, hash: &str) -> UsersResult<()> {
        self.cache.del(&self.hash_key(hash)).await?;
        Ok(())
    }

    pub async fn invalidate_id(&self, id: i64) -> UsersResult<()> {
        self.cache.del(&self.id_key(id)).await?;
        Ok(())
    }

    /// Delegate mint to the inner store (no cache prewarm).
    pub async fn create(&self, row: &AuthToken) -> UsersResult<i64> {
        self.inner.create(row).await
    }

    /// Forward list to the inner store (no caching — admin op).
    pub async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        self.inner.list_for_user(uid).await
    }

    pub async fn revoke(&self, id: i64) -> UsersResult<()> {
        // Look up first so we know the hash to invalidate.
        if let Some(row) = self.inner.lookup_by_id(id).await? {
            let _ = self.invalidate_hash(&row.token).await;
        }
        let _ = self.invalidate_id(id).await;
        self.inner.revoke(id).await
    }

    pub async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()> {
        let rows = self.inner.list_for_user(uid).await?;
        let result = self.inner.revoke_all_for_user_except(uid, except).await;
        for row in rows {
            if row.id != except {
                let _ = self.invalidate_hash(&row.token).await;
                let _ = self.invalidate_id(row.id).await;
            }
        }
        result
    }

    pub async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        let rows = self.inner.list_for_user(uid).await?;
        let result = self.inner.invalidate_all_for_user(uid).await;
        for row in rows {
            let _ = self.invalidate_hash(&row.token).await;
            let _ = self.invalidate_id(row.id).await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("t.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    fn fixture_token(uid: &str, hash: &str, kind: AuthTokenType) -> AuthToken {
        AuthToken {
            id: 0,
            uid: UserId::new(uid).unwrap(),
            login_name: uid.into(),
            password: None,
            name: "test".into(),
            token: hash.into(),
            kind,
            remember: false,
            last_activity: 100,
            last_check: 100,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        }
    }

    #[tokio::test]
    async fn create_then_lookup_by_hash_roundtrips() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hashA", AuthTokenType::AppPassword))
            .await
            .unwrap();
        assert!(id > 0);
        let got = store.lookup_by_hash("hashA").await.unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.uid.as_str(), "alice");
        assert_eq!(got.kind, AuthTokenType::AppPassword);
    }

    #[tokio::test]
    async fn lookup_by_id_returns_full_row() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("bob", "hashB", AuthTokenType::Session))
            .await
            .unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.token, "hashB");
    }

    #[tokio::test]
    async fn lookup_by_hash_returns_none_on_miss() {
        let store = SqlTokenStore::new(fresh_pool().await);
        assert!(store.lookup_by_hash("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_for_user_returns_rows_newest_first() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let mut a = fixture_token("alice", "h1", AuthTokenType::Session);
        a.last_activity = 100;
        let mut b = fixture_token("alice", "h2", AuthTokenType::AppPassword);
        b.last_activity = 200;
        store.create(&a).await.unwrap();
        store.create(&b).await.unwrap();
        let rows = store
            .list_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].token, "h2");
        assert_eq!(rows[1].token, "h1");
    }

    #[tokio::test]
    async fn bump_activity_writes_now() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hX", AuthTokenType::Session))
            .await
            .unwrap();
        store.bump_activity(id, 9999).await.unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.last_activity, 9999);
        assert_eq!(got.last_check, 9999);
    }

    #[tokio::test]
    async fn revoke_deletes_row() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "hY", AuthTokenType::Session))
            .await
            .unwrap();
        store.revoke(id).await.unwrap();
        assert!(store.lookup_by_id(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_is_idempotent() {
        let store = SqlTokenStore::new(fresh_pool().await);
        store.revoke(9999).await.unwrap();
    }

    #[tokio::test]
    async fn revoke_all_for_user_except_keeps_one() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let keep = store
            .create(&fixture_token("alice", "k", AuthTokenType::Session))
            .await
            .unwrap();
        let _drop = store
            .create(&fixture_token("alice", "d", AuthTokenType::AppPassword))
            .await
            .unwrap();
        store
            .revoke_all_for_user_except(&UserId::new("alice").unwrap(), keep)
            .await
            .unwrap();
        let remaining = store
            .list_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep);
    }

    #[tokio::test]
    async fn invalidate_all_for_user_sets_password_invalid() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "h", AuthTokenType::Session))
            .await
            .unwrap();
        store
            .invalidate_all_for_user(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        let got = store.lookup_by_id(id).await.unwrap().unwrap();
        assert!(got.password_invalid);
    }

    use crabcloud_cache::MemoryCache;

    fn fresh_cache(store: SqlTokenStore) -> TokenAuthCache {
        TokenAuthCache::new(Arc::new(store), Arc::new(MemoryCache::new()), "inst1")
    }

    #[tokio::test]
    async fn cache_hit_does_not_query_db_second_time() {
        let pool = fresh_pool().await;
        let store = SqlTokenStore::new(pool);
        let id = store
            .create(&fixture_token("alice", "hashH", AuthTokenType::AppPassword))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        let first = cache.lookup_by_hash("hashH").await.unwrap().unwrap();
        assert_eq!(first.id, id);
        // Revoke via store directly (bypasses cache invalidation).
        store.revoke(id).await.unwrap();
        // Second lookup: cache returns the stale row (within TTL).
        let second = cache.lookup_by_hash("hashH").await.unwrap().unwrap();
        assert_eq!(second.id, id);
    }

    #[tokio::test]
    async fn negative_cache_absorbs_misses() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let cache = fresh_cache(store.clone());
        assert!(cache.lookup_by_hash("missing").await.unwrap().is_none());
        // Insert via inner store (cache doesn't know).
        store
            .create(&fixture_token("alice", "missing", AuthTokenType::Session))
            .await
            .unwrap();
        // Cache still returns None during negative-cache window.
        assert!(cache.lookup_by_hash("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn maybe_bump_activity_rate_limits_writes() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "h", AuthTokenType::Session))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        let row = cache.lookup_by_hash("h").await.unwrap().unwrap();
        // First bump (now is far in the future) goes through.
        cache.maybe_bump_activity(&row, 10_000).await.unwrap();
        let after = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(after.last_activity, 10_000);
        // Second bump within the rate-limit window — skipped.
        cache.maybe_bump_activity(&after, 10_010).await.unwrap();
        let still = store.lookup_by_id(id).await.unwrap().unwrap();
        assert_eq!(still.last_activity, 10_000);
    }

    #[tokio::test]
    async fn revoke_invalidates_cache() {
        let store = SqlTokenStore::new(fresh_pool().await);
        let id = store
            .create(&fixture_token("alice", "rev", AuthTokenType::Session))
            .await
            .unwrap();
        let cache = fresh_cache(store.clone());
        cache.lookup_by_hash("rev").await.unwrap();
        cache.revoke(id).await.unwrap();
        assert!(cache.lookup_by_hash("rev").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn bump_activity_on_missing_row_is_noop() {
        // Optional follow-up from Batch A's code-quality review.
        let store = SqlTokenStore::new(fresh_pool().await);
        // The trait method should not error on a missing row.
        store.bump_activity(9999, 1234).await.unwrap();
    }
}
