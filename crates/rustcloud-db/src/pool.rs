use crate::error::{DbError, DbResult};
use rustcloud_config::{DbType, FileConfig};
use secrecy::ExposeSecret;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};
use sqlx::{MySqlPool, PgPool, SqlitePool};

#[derive(Debug, Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
    Postgres(PgPool),
}

impl DbPool {
    /// Connect using settings from `config`.
    pub async fn connect(config: &FileConfig) -> DbResult<Self> {
        let max = config.db_pool_max;
        match config.dbtype {
            DbType::Sqlite => {
                let opts = SqliteConnectOptions::new()
                    .filename(&config.dbname)
                    .create_if_missing(true);
                let pool = SqlitePoolOptions::new()
                    .max_connections(max)
                    .connect_with(opts)
                    .await?;
                Ok(DbPool::Sqlite(pool))
            }
            DbType::Mysql => {
                let url = build_url(config)?;
                let pool = MySqlPoolOptions::new()
                    .max_connections(max)
                    .connect(&url)
                    .await?;
                Ok(DbPool::MySql(pool))
            }
            DbType::Pgsql => {
                let url = build_url(config)?;
                let pool = PgPoolOptions::new()
                    .max_connections(max)
                    .connect(&url)
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

/// Build the dialect-appropriate connection URL for MySQL or Postgres.
/// SQLite uses `SqliteConnectOptions` directly to avoid Windows-path URL issues.
fn build_url(config: &FileConfig) -> DbResult<String> {
    match config.dbtype {
        DbType::Sqlite => Err(DbError::InvalidUrl(
            "build_url is not used for sqlite; SqliteConnectOptions is used directly".into(),
        )),
        DbType::Mysql => {
            let host = config
                .dbhost
                .as_deref()
                .ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
            let port = config.dbport.unwrap_or(3306);
            let user = config.dbuser.as_deref().unwrap_or("");
            let password = config
                .dbpassword
                .as_ref()
                .map(|s| s.expose_secret().to_string())
                .unwrap_or_default();
            Ok(format!(
                "mysql://{}:{}@{}:{}/{}",
                user, password, host, port, config.dbname
            ))
        }
        DbType::Pgsql => {
            let host = config
                .dbhost
                .as_deref()
                .ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
            let port = config.dbport.unwrap_or(5432);
            let user = config.dbuser.as_deref().unwrap_or("");
            let password = config
                .dbpassword
                .as_ref()
                .map(|s| s.expose_secret().to_string())
                .unwrap_or_default();
            Ok(format!(
                "postgres://{}:{}@{}:{}/{}",
                user, password, host, port, config.dbname
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_config::CacheConfig;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
        }
    }

    #[tokio::test]
    async fn connects_to_sqlite_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let cfg = cfg_sqlite(path);
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

    #[test]
    fn mysql_url_without_host_errors() {
        let mut cfg = cfg_sqlite(PathBuf::from("ignored.db"));
        cfg.dbtype = DbType::Mysql;
        cfg.dbhost = None;
        let err = build_url(&cfg).unwrap_err();
        assert!(matches!(err, DbError::InvalidUrl(_)));
    }

    #[test]
    fn pgsql_url_builds() {
        let mut cfg = cfg_sqlite(PathBuf::from("ignored.db"));
        cfg.dbtype = DbType::Pgsql;
        cfg.dbhost = Some("127.0.0.1".into());
        cfg.dbport = Some(5433);
        cfg.dbuser = Some("u".into());
        cfg.dbpassword = Some(SecretString::new("p".into()));
        cfg.dbname = "d".into();
        let url = build_url(&cfg).unwrap();
        assert_eq!(url, "postgres://u:p@127.0.0.1:5433/d");
    }
}
