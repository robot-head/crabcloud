//! Migrations for the `core` namespace.
//!
//! The SQL is `include_str!`'d from `migrations/core/`. Adding a new migration:
//!   1. Add files at `migrations/core/<NNNN>_<name>/{sqlite,mysql,postgres}.sql`.
//!   2. Append a `Migration` to `CORE_MIGRATIONS` below with a strictly increasing `version`.
//!   3. Run `cargo xtask prepare` (later phase) to refresh the offline sqlx cache.

use crate::migrate::{Migration, MigrationSet};

/// Migration namespace identifier for the core schema.
pub const CORE_NAMESPACE: &str = "core";

/// All migrations for the `core` namespace, in version order.
pub const CORE_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sqlite: include_str!("../../../migrations/core/0001_initial/sqlite.sql"),
    mysql: include_str!("../../../migrations/core/0001_initial/mysql.sql"),
    postgres: include_str!("../../../migrations/core/0001_initial/postgres.sql"),
}];

/// Returns the `core` migration set ready to be registered with a
/// [`crate::MigrationRunner`].
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
    use crabcloud_config::test_support::minimal_sqlite_config;
    use tempfile::tempdir;

    #[tokio::test]
    async fn core_migration_applies_against_sqlite() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("test.db"));
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
