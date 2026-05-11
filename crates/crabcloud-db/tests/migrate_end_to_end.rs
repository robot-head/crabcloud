//! End-to-end migrate flow per dialect.
//!
//! Reads URLs from env vars; SQLite uses a temp file. MySQL and Postgres tests are
//! `#[ignore]` by default so contributors without Docker aren't blocked. CI runs
//! `cargo test -- --include-ignored` to enable them.

// Integration tests pull in all the crate's deps even when they only exercise a
// narrow surface — quiet the workspace `unused_crate_dependencies` lint here.
#![allow(unused_crate_dependencies)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_config::{DbType, FileConfig};
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use secrecy::SecretString;
use std::path::PathBuf;
use tempfile::tempdir;

async fn assert_appconfig_table_usable(pool: &DbPool) {
    // Cross-dialect placeholders: SQLite/MySQL use `?`, Postgres uses `$N`.
    // For a smoke test, write a row using the dialect-appropriate query.
    let insert_sql: &str = match pool {
        DbPool::Postgres(_) => {
            "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES ($1, $2, $3)"
        }
        _ => "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES (?, ?, ?)",
    };
    let select_sql: &str = match pool {
        DbPool::Postgres(_) => {
            "SELECT configvalue FROM oc_appconfig WHERE appid = $1 AND configkey = $2"
        }
        _ => "SELECT configvalue FROM oc_appconfig WHERE appid = ? AND configkey = ?",
    };
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(insert_sql)
                .bind("core")
                .bind("k")
                .bind("v")
                .execute(p)
                .await
                .unwrap();
            let v: String = sqlx::query_scalar(select_sql)
                .bind("core")
                .bind("k")
                .fetch_one(p)
                .await
                .unwrap();
            assert_eq!(v, "v");
        }
        DbPool::MySql(p) => {
            sqlx::query(insert_sql)
                .bind("core")
                .bind("k")
                .bind("v")
                .execute(p)
                .await
                .unwrap();
            let v: String = sqlx::query_scalar(select_sql)
                .bind("core")
                .bind("k")
                .fetch_one(p)
                .await
                .unwrap();
            assert_eq!(v, "v");
        }
        DbPool::Postgres(p) => {
            sqlx::query(insert_sql)
                .bind("core")
                .bind("k")
                .bind("v")
                .execute(p)
                .await
                .unwrap();
            let v: String = sqlx::query_scalar(select_sql)
                .bind("core")
                .bind("k")
                .fetch_one(p)
                .await
                .unwrap();
            assert_eq!(v, "v");
        }
    }
}

#[tokio::test]
async fn migrate_sqlite() {
    let dir = tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("it.db"));

    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires MySQL — run with --include-ignored after `cargo xtask up`"]
async fn migrate_mysql() {
    let url = std::env::var("CRABCLOUD_TEST_MYSQL_URL")
        .unwrap_or_else(|_| "mysql://crabcloud:crabcloud@127.0.0.1:3307/crabcloud".into());
    let cfg = mysql_config_from_url(&url);
    let pool = DbPool::connect(&cfg).await.unwrap();

    // Tests may share a database; drop our migration tracking + appconfig first.
    if let DbPool::MySql(p) = &pool {
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_appconfig")
            .execute(p)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_migrations")
            .execute(p)
            .await;
    }

    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires Postgres — run with --include-ignored after `cargo xtask up`"]
async fn migrate_postgres() {
    let url = std::env::var("CRABCLOUD_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://crabcloud:crabcloud@127.0.0.1:5433/crabcloud".into());
    let cfg = postgres_config_from_url(&url);
    let pool = DbPool::connect(&cfg).await.unwrap();

    if let DbPool::Postgres(p) = &pool {
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_appconfig")
            .execute(p)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_migrations")
            .execute(p)
            .await;
    }

    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

// --- URL → config helpers (parsing a URL is the simplest way to populate FileConfig
//     fields from env without reinventing the wheel) ---

fn mysql_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Mysql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

fn postgres_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Pgsql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

struct ParsedUrl {
    user: String,
    password: Option<String>,
    host: String,
    port: u16,
    database: String,
}

fn parse_url(url: &str) -> ParsedUrl {
    // Format: scheme://user:pass@host:port/db
    let after_scheme = url.split_once("://").expect("scheme").1;
    let (auth, host_db) = after_scheme.split_once('@').expect("auth");
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (host_port, database) = host_db.split_once('/').expect("path");
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap()),
        None => (host_port.to_string(), 0),
    };
    ParsedUrl {
        user,
        password,
        host,
        port,
        database: database.to_string(),
    }
}
