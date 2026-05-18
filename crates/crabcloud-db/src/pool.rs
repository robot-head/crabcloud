use crate::error::{DbError, DbResult};
use crabcloud_config::{DbType, FileConfig};
use secrecy::ExposeSecret;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::postgres::PgConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};
use sqlx::{MySqlPool, PgPool, SqlitePool};

/// Dialect-tagged connection pool. Cloning shares the underlying pool.
#[derive(Debug, Clone)]
pub enum DbPool {
    /// SQLite-backed pool.
    Sqlite(SqlitePool),
    /// MySQL/MariaDB-backed pool.
    MySql(MySqlPool),
    /// PostgreSQL-backed pool.
    Postgres(PgPool),
}

impl DbPool {
    /// Connect using settings from `config`.
    pub async fn connect(config: &FileConfig) -> DbResult<Self> {
        let max = config.db_pool_max;
        match config.dbtype {
            DbType::Sqlite => {
                // `busy_timeout` makes concurrent writers wait for the
                // lock instead of erroring immediately with "database
                // is locked". 5 s comfortably covers all in-tree
                // background work (scanner apply, search indexer
                // upserts, sweepers, mail worker) under typical
                // contention. WAL is intentionally NOT enabled here:
                // it interacts poorly with sqlx's default DEFERRED
                // transactions, surfacing as SQLITE_BUSY_SNAPSHOT when
                // an indexer read-then-write tx races a scanner
                // commit, which busy_timeout does NOT retry.
                let opts = SqliteConnectOptions::new()
                    .filename(&config.dbname)
                    .create_if_missing(true)
                    .busy_timeout(std::time::Duration::from_secs(5));
                let pool = SqlitePoolOptions::new()
                    .max_connections(max)
                    .connect_with(opts)
                    .await?;
                Ok(DbPool::Sqlite(pool))
            }
            DbType::Mysql => {
                let host = config
                    .dbhost
                    .as_deref()
                    .ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
                let mut opts = MySqlConnectOptions::new()
                    .host(host)
                    .port(config.dbport.unwrap_or(3306))
                    .database(&config.dbname);
                if let Some(user) = config.dbuser.as_deref() {
                    opts = opts.username(user);
                }
                if let Some(pw) = config.dbpassword.as_ref() {
                    opts = opts.password(pw.expose_secret());
                }
                let pool = MySqlPoolOptions::new()
                    .max_connections(max)
                    .connect_with(opts)
                    .await?;
                Ok(DbPool::MySql(pool))
            }
            DbType::Pgsql => {
                let host = config
                    .dbhost
                    .as_deref()
                    .ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
                let mut opts = PgConnectOptions::new()
                    .host(host)
                    .port(config.dbport.unwrap_or(5432))
                    .database(&config.dbname);
                if let Some(user) = config.dbuser.as_deref() {
                    opts = opts.username(user);
                }
                if let Some(pw) = config.dbpassword.as_ref() {
                    opts = opts.password(pw.expose_secret());
                }
                let pool = PgPoolOptions::new()
                    .max_connections(max)
                    .connect_with(opts)
                    .await?;
                Ok(DbPool::Postgres(pool))
            }
        }
    }

    /// Convenience: a short label for logging.
    pub fn dialect(&self) -> &'static str {
        match self {
            DbPool::Sqlite(_) => "sqlite",
            DbPool::MySql(_) => "mysql",
            DbPool::Postgres(_) => "postgres",
        }
    }

    /// Close the pool and wait for in-flight connections to drain.
    pub async fn close(&self) {
        match self {
            DbPool::Sqlite(p) => p.close().await,
            DbPool::MySql(p) => p.close().await,
            DbPool::Postgres(p) => p.close().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[tokio::test]
    async fn connects_to_sqlite_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let cfg = minimal_sqlite_config(path);
        let pool = DbPool::connect(&cfg).await.unwrap();
        assert_eq!(pool.dialect(), "sqlite");

        // Smoke-test an actual query through the connection.
        let one: i64 = match &pool {
            DbPool::Sqlite(p) => sqlx::query_scalar("SELECT 1").fetch_one(p).await.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(one, 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn mysql_without_host_errors() {
        let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
        cfg.dbtype = DbType::Mysql;
        cfg.dbhost = None;
        let err = DbPool::connect(&cfg).await.unwrap_err();
        assert!(matches!(err, DbError::InvalidUrl(_)));
    }
}
