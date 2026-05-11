//! Per-namespace migration runner.
//!
//! See spec §6.4.

use crate::{DbPool, DbResult};
use std::collections::BTreeMap;

/// A single migration: its version within a namespace, and SQL for each dialect.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i64,
    /// Short human-readable identifier.
    pub name: &'static str,
    /// SQLite-dialect SQL.
    pub sqlite: &'static str,
    /// MySQL-dialect SQL.
    pub mysql: &'static str,
    /// Postgres-dialect SQL.
    pub postgres: &'static str,
}

/// A set of migrations for one namespace.
///
/// The `migrations` slice MUST be sorted by ascending `version` and contain no duplicate
/// version numbers; this is debug-asserted in `MigrationRunner::register`.
#[derive(Debug, Clone)]
pub struct MigrationSet {
    pub namespace: &'static str,
    pub migrations: &'static [Migration],
}

pub struct MigrationRunner<'a> {
    pool: &'a DbPool,
    sets: Vec<MigrationSet>,
    prefix: String,
}

impl<'a> MigrationRunner<'a> {
    pub fn new(pool: &'a DbPool, prefix: impl Into<String>) -> Self {
        Self {
            pool,
            sets: Vec::new(),
            prefix: prefix.into(),
        }
    }

    pub fn register(&mut self, set: MigrationSet) -> &mut Self {
        debug_assert!(
            set.migrations
                .windows(2)
                .all(|w| w[0].version < w[1].version),
            "migrations for namespace `{}` must be sorted by ascending version and use distinct version numbers",
            set.namespace
        );
        self.sets.push(set);
        self
    }

    /// Apply all pending migrations across registered namespaces.
    /// Returns the count actually applied.
    pub async fn run(&self) -> DbResult<usize> {
        ensure_tracking_table(self.pool, &self.prefix).await?;
        let mut applied = 0;
        for set in &self.sets {
            applied += run_namespace(self.pool, &self.prefix, set).await?;
        }
        Ok(applied)
    }

    /// List applied (namespace, version) pairs. For debugging / tests.
    pub async fn applied(&self) -> DbResult<BTreeMap<String, Vec<i64>>> {
        ensure_tracking_table(self.pool, &self.prefix).await?;
        list_applied(self.pool, &self.prefix).await
    }
}

async fn ensure_tracking_table(pool: &DbPool, prefix: &str) -> DbResult<()> {
    let table = format!("{}migrations", prefix);
    let sql = match pool {
        DbPool::Sqlite(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace TEXT NOT NULL,
                version INTEGER NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (namespace, version)
            )"
        ),
        DbPool::MySql(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace VARCHAR(64) NOT NULL,
                version BIGINT NOT NULL,
                applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (namespace, version)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        ),
        DbPool::Postgres(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace TEXT NOT NULL,
                version BIGINT NOT NULL,
                applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (namespace, version)
            )"
        ),
    };
    execute(pool, &sql).await?;
    Ok(())
}

async fn run_namespace(pool: &DbPool, prefix: &str, set: &MigrationSet) -> DbResult<usize> {
    let applied = list_applied_for(pool, prefix, set.namespace).await?;
    let mut count = 0;
    for migration in set.migrations {
        if applied.contains(&migration.version) {
            continue;
        }
        let sql = pick_sql(pool, migration);
        // Each migration runs in its own transaction-ish unit: execute the SQL, then
        // record the row. For SQLite/MySQL we can't easily wrap multi-statement migration
        // SQL inside sqlx's transaction without DDL caveats — so we keep it simple and
        // accept that a partial failure leaves an unrecorded migration. The runner is
        // designed to be idempotent at the SQL level (CREATE TABLE IF NOT EXISTS, etc.).
        execute_multi(pool, sql)
            .await
            .map_err(|e| crate::DbError::Migration {
                namespace: set.namespace.into(),
                version: migration.version,
                message: e.to_string(),
            })?;
        record_migration(pool, prefix, set.namespace, migration.version).await?;
        tracing::info!(
            namespace = set.namespace,
            version = migration.version,
            name = migration.name,
            "applied migration"
        );
        count += 1;
    }
    Ok(count)
}

fn pick_sql<'m>(pool: &DbPool, migration: &'m Migration) -> &'m str {
    match pool {
        DbPool::Sqlite(_) => migration.sqlite,
        DbPool::MySql(_) => migration.mysql,
        DbPool::Postgres(_) => migration.postgres,
    }
}

async fn execute(pool: &DbPool, sql: &str) -> DbResult<()> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(sql).execute(p).await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(sql).execute(p).await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(sql).execute(p).await?;
        }
    }
    Ok(())
}

/// Execute a migration SQL string that may contain multiple statements separated by `;`.
/// sqlx's `query().execute()` only runs a single statement, so we split.
async fn execute_multi(pool: &DbPool, sql: &str) -> DbResult<()> {
    for statement in split_statements(sql) {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        execute(pool, trimmed).await?;
    }
    Ok(())
}

/// Naive `;` splitter. Migration SQL must not contain semicolons inside string literals or
/// comments. This is a deliberate simplifying constraint; the migrations we write follow it.
fn split_statements(sql: &str) -> Vec<&str> {
    sql.split(';').collect()
}

async fn record_migration(
    pool: &DbPool,
    prefix: &str,
    namespace: &str,
    version: i64,
) -> DbResult<()> {
    let table = format!("{}migrations", prefix);
    let sql = format!("INSERT INTO {table} (namespace, version) VALUES (?, ?)");
    let pg_sql = format!("INSERT INTO {table} (namespace, version) VALUES ($1, $2)");
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(&sql)
                .bind(namespace)
                .bind(version)
                .execute(p)
                .await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(&sql)
                .bind(namespace)
                .bind(version)
                .execute(p)
                .await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(&pg_sql)
                .bind(namespace)
                .bind(version)
                .execute(p)
                .await?;
        }
    }
    Ok(())
}

async fn list_applied_for(pool: &DbPool, prefix: &str, namespace: &str) -> DbResult<Vec<i64>> {
    let table = format!("{}migrations", prefix);
    let sql = format!("SELECT version FROM {table} WHERE namespace = ? ORDER BY version");
    let pg_sql = format!("SELECT version FROM {table} WHERE namespace = $1 ORDER BY version");
    let rows: Vec<i64> = match pool {
        DbPool::Sqlite(p) => {
            sqlx::query_scalar(&sql)
                .bind(namespace)
                .fetch_all(p)
                .await?
        }
        DbPool::MySql(p) => {
            sqlx::query_scalar(&sql)
                .bind(namespace)
                .fetch_all(p)
                .await?
        }
        DbPool::Postgres(p) => {
            sqlx::query_scalar(&pg_sql)
                .bind(namespace)
                .fetch_all(p)
                .await?
        }
    };
    Ok(rows)
}

async fn list_applied(pool: &DbPool, prefix: &str) -> DbResult<BTreeMap<String, Vec<i64>>> {
    let table = format!("{}migrations", prefix);
    let sql = format!("SELECT namespace, version FROM {table} ORDER BY namespace, version");
    let rows: Vec<(String, i64)> = match pool {
        DbPool::Sqlite(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        DbPool::MySql(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        DbPool::Postgres(p) => sqlx::query_as(&sql).fetch_all(p).await?,
    };
    let mut out: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (ns, v) in rows {
        out.entry(ns).or_default().push(v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_config::test_support::minimal_sqlite_config;
    use tempfile::tempdir;

    const TEST_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "create_widgets",
            sqlite: "CREATE TABLE widgets (id INTEGER PRIMARY KEY, label TEXT NOT NULL)",
            mysql: "CREATE TABLE widgets (id BIGINT PRIMARY KEY, label VARCHAR(255) NOT NULL)",
            postgres: "CREATE TABLE widgets (id BIGINT PRIMARY KEY, label TEXT NOT NULL)",
        },
        Migration {
            version: 2,
            name: "add_widget_color",
            sqlite: "ALTER TABLE widgets ADD COLUMN color TEXT",
            mysql: "ALTER TABLE widgets ADD COLUMN color VARCHAR(32)",
            postgres: "ALTER TABLE widgets ADD COLUMN color TEXT",
        },
    ];

    #[tokio::test]
    async fn applies_migrations_in_order() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet {
            namespace: "core_test",
            migrations: TEST_MIGRATIONS,
        });
        let applied = runner.run().await.unwrap();
        assert_eq!(applied, 2);

        let map = runner.applied().await.unwrap();
        assert_eq!(map.get("core_test"), Some(&vec![1, 2]));

        pool.close().await;
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet {
            namespace: "core_test",
            migrations: TEST_MIGRATIONS,
        });
        let first = runner.run().await.unwrap();
        let second = runner.run().await.unwrap();
        assert_eq!(first, 2);
        assert_eq!(second, 0);

        pool.close().await;
    }

    #[tokio::test]
    async fn separate_namespaces_track_independently() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        static NS_A_MIGRATIONS: &[Migration] = &[Migration {
            version: 1,
            name: "a1",
            sqlite: "CREATE TABLE a (id INTEGER PRIMARY KEY)",
            mysql: "",
            postgres: "",
        }];
        static NS_B_MIGRATIONS: &[Migration] = &[Migration {
            version: 1,
            name: "b1",
            sqlite: "CREATE TABLE b (id INTEGER PRIMARY KEY)",
            mysql: "",
            postgres: "",
        }];

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet {
            namespace: "ns_a",
            migrations: NS_A_MIGRATIONS,
        });
        runner.register(MigrationSet {
            namespace: "ns_b",
            migrations: NS_B_MIGRATIONS,
        });
        runner.run().await.unwrap();

        let map = runner.applied().await.unwrap();
        assert_eq!(map.get("ns_a"), Some(&vec![1]));
        assert_eq!(map.get("ns_b"), Some(&vec![1]));
        pool.close().await;
    }
}
