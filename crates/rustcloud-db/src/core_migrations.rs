//! Migrations for the `core` namespace.
//!
//! The SQL is `include_str!`'d from `migrations/core/`. Adding a new migration:
//!   1. Add files at `migrations/core/<NNNN>_<name>/{sqlite,mysql,postgres}.sql`.
//!   2. Append a `Migration` to `CORE_MIGRATIONS` below with a strictly increasing `version`.
//!   3. Run `cargo xtask prepare` (later phase) to refresh the offline sqlx cache.

use crate::migrate::{Migration, MigrationSet};

pub const CORE_NAMESPACE: &str = "core";

pub const CORE_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sqlite: include_str!("../../../migrations/core/0001_initial/sqlite.sql"),
    mysql: include_str!("../../../migrations/core/0001_initial/mysql.sql"),
    postgres: include_str!("../../../migrations/core/0001_initial/postgres.sql"),
}];

pub fn core_set() -> MigrationSet {
    MigrationSet {
        namespace: CORE_NAMESPACE,
        migrations: CORE_MIGRATIONS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DbPool, MigrationRunner};
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
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
            bootstrap_admin: None,
        }
    }

    #[tokio::test]
    async fn core_migration_applies_against_sqlite() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        let applied = runner.run().await.unwrap();
        assert_eq!(applied, 1);

        // Verify oc_appconfig exists and accepts a row.
        match &pool {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES (?, ?, ?)",
                )
                .bind("core")
                .bind("instanceid")
                .bind("hello")
                .execute(p)
                .await
                .unwrap();
                let value: String = sqlx::query_scalar(
                    "SELECT configvalue FROM oc_appconfig WHERE appid = ? AND configkey = ?",
                )
                .bind("core")
                .bind("instanceid")
                .fetch_one(p)
                .await
                .unwrap();
                assert_eq!(value, "hello");
            }
            _ => unreachable!(),
        }
        pool.close().await;
    }
}
