//! `oc_properties` — per-user PROPPATCH custom DAV property storage.
//!
//! Key shape: `(userid, propertypath, propertyname) -> propertyvalue`. Path-keyed
//! (matches Nextcloud upstream); MOVE/COPY handlers must call `rename_path` /
//! `copy_path` to keep props synchronized with the file tree.

use crabcloud_db::DbPool;
use crabcloud_users::UserId;

use crate::error::{FileCacheError, FileCacheResult};

pub struct PropertyStore {
    pool: DbPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyRow {
    pub propertyname: String,
    pub propertyvalue: Option<String>,
}

impl PropertyStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// All props for a single resource. Returns rows in `propertyname` ASC order.
    pub async fn get(
        &self,
        userid: &UserId,
        propertypath: &str,
    ) -> FileCacheResult<Vec<PropertyRow>> {
        let rows: Vec<(String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query_as(
                "SELECT propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = ? AND propertypath = ? ORDER BY propertyname ASC",
            )
            .bind(userid.as_str())
            .bind(propertypath)
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::MySql(p) => sqlx::query_as(
                "SELECT propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = ? AND propertypath = ? ORDER BY propertyname ASC",
            )
            .bind(userid.as_str())
            .bind(propertypath)
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::Postgres(p) => sqlx::query_as(
                "SELECT propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = $1 AND propertypath = $2 ORDER BY propertyname ASC",
            )
            .bind(userid.as_str())
            .bind(propertypath)
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
        };
        Ok(rows
            .into_iter()
            .map(|(n, v)| PropertyRow {
                propertyname: n,
                propertyvalue: v,
            })
            .collect())
    }

    /// One named property's value across many paths. Used by PROPFIND Depth: 1
    /// to fetch `{oc:}favorite` for every child in one query.
    pub async fn get_many(
        &self,
        userid: &UserId,
        propertypaths: &[String],
        propertyname: &str,
    ) -> FileCacheResult<Vec<(String, Option<String>)>> {
        if propertypaths.is_empty() {
            return Ok(Vec::new());
        }
        // Build a placeholder list; sqlx 0.8 doesn't have native array binding
        // across dialects, so we expand inline.
        let placeholders: String = (0..propertypaths.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let pg_placeholders: String = (1..=propertypaths.len())
            .map(|i| format!("${}", i + 2))
            .collect::<Vec<_>>()
            .join(",");

        match &self.pool {
            DbPool::Sqlite(p) => {
                let sql = format!(
                    "SELECT propertypath, propertyvalue FROM oc_properties \
                     WHERE userid = ? AND propertyname = ? AND propertypath IN ({})",
                    placeholders
                );
                let mut q = sqlx::query_as(&sql)
                    .bind(userid.as_str())
                    .bind(propertyname);
                for p in propertypaths {
                    q = q.bind(p);
                }
                let rows: Vec<(String, Option<String>)> =
                    q.fetch_all(p).await.map_err(FileCacheError::Db)?;
                Ok(rows)
            }
            DbPool::MySql(p) => {
                let sql = format!(
                    "SELECT propertypath, propertyvalue FROM oc_properties \
                     WHERE userid = ? AND propertyname = ? AND propertypath IN ({})",
                    placeholders
                );
                let mut q = sqlx::query_as(&sql)
                    .bind(userid.as_str())
                    .bind(propertyname);
                for p in propertypaths {
                    q = q.bind(p);
                }
                let rows: Vec<(String, Option<String>)> =
                    q.fetch_all(p).await.map_err(FileCacheError::Db)?;
                Ok(rows)
            }
            DbPool::Postgres(p) => {
                let sql = format!(
                    "SELECT propertypath, propertyvalue FROM oc_properties \
                     WHERE userid = $1 AND propertyname = $2 AND propertypath IN ({})",
                    pg_placeholders
                );
                let mut q = sqlx::query_as(&sql)
                    .bind(userid.as_str())
                    .bind(propertyname);
                for p in propertypaths {
                    q = q.bind(p);
                }
                let rows: Vec<(String, Option<String>)> =
                    q.fetch_all(p).await.map_err(FileCacheError::Db)?;
                Ok(rows)
            }
        }
    }

    /// Insert-or-update one prop.
    pub async fn upsert(
        &self,
        userid: &UserId,
        propertypath: &str,
        propertyname: &str,
        propertyvalue: Option<&str>,
    ) -> FileCacheResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_properties (userid, propertypath, propertyname, propertyvalue) \
                     VALUES (?, ?, ?, ?) \
                     ON CONFLICT(userid, propertypath, propertyname) DO UPDATE \
                     SET propertyvalue = excluded.propertyvalue",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .bind(propertyvalue)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "INSERT INTO oc_properties (userid, propertypath, propertyname, propertyvalue) \
                     VALUES (?, ?, ?, ?) \
                     ON DUPLICATE KEY UPDATE propertyvalue = VALUES(propertyvalue)",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .bind(propertyvalue)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "INSERT INTO oc_properties (userid, propertypath, propertyname, propertyvalue) \
                     VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (userid, propertypath, propertyname) DO UPDATE \
                     SET propertyvalue = EXCLUDED.propertyvalue",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .bind(propertyvalue)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
        }
        Ok(())
    }

    /// Remove one prop. No-op if absent.
    pub async fn delete(
        &self,
        userid: &UserId,
        propertypath: &str,
        propertyname: &str,
    ) -> FileCacheResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "DELETE FROM oc_properties \
                     WHERE userid = ? AND propertypath = ? AND propertyname = ?",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "DELETE FROM oc_properties \
                     WHERE userid = ? AND propertypath = ? AND propertyname = ?",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "DELETE FROM oc_properties \
                     WHERE userid = $1 AND propertypath = $2 AND propertyname = $3",
                )
                .bind(userid.as_str())
                .bind(propertypath)
                .bind(propertyname)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
        }
        Ok(())
    }

    /// Rewrite paths after a MOVE. Single UPDATE for the resource itself AND
    /// every descendant (matching `from/` prefix).
    pub async fn rename_path(&self, userid: &UserId, from: &str, to: &str) -> FileCacheResult<()> {
        let from_prefix = format!("{}/", from);
        let to_prefix = format!("{}/", to);
        match &self.pool {
            DbPool::Sqlite(p) => {
                // Exact-match row.
                sqlx::query(
                    "UPDATE oc_properties SET propertypath = ? \
                     WHERE userid = ? AND propertypath = ?",
                )
                .bind(to)
                .bind(userid.as_str())
                .bind(from)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
                // Descendant rows.
                sqlx::query(
                    "UPDATE oc_properties \
                     SET propertypath = ? || SUBSTR(propertypath, ? + 1) \
                     WHERE userid = ? AND propertypath LIKE ?",
                )
                .bind(&to_prefix)
                .bind(from_prefix.len() as i64)
                .bind(userid.as_str())
                .bind(format!("{}%", from_prefix))
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "UPDATE oc_properties SET propertypath = ? \
                     WHERE userid = ? AND propertypath = ?",
                )
                .bind(to)
                .bind(userid.as_str())
                .bind(from)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
                sqlx::query(
                    "UPDATE oc_properties \
                     SET propertypath = CONCAT(?, SUBSTRING(propertypath, ? + 1)) \
                     WHERE userid = ? AND propertypath LIKE ?",
                )
                .bind(&to_prefix)
                .bind(from_prefix.len() as i64)
                .bind(userid.as_str())
                .bind(format!("{}%", from_prefix))
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "UPDATE oc_properties SET propertypath = $1 \
                     WHERE userid = $2 AND propertypath = $3",
                )
                .bind(to)
                .bind(userid.as_str())
                .bind(from)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
                sqlx::query(
                    "UPDATE oc_properties \
                     SET propertypath = $1 || SUBSTRING(propertypath FROM $2::int + 1) \
                     WHERE userid = $3 AND propertypath LIKE $4",
                )
                .bind(&to_prefix)
                .bind(from_prefix.len() as i32)
                .bind(userid.as_str())
                .bind(format!("{}%", from_prefix))
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
        }
        Ok(())
    }

    /// Copy all props from one path subtree to another. Used by COPY handler.
    pub async fn copy_path(&self, userid: &UserId, from: &str, to: &str) -> FileCacheResult<()> {
        let from_prefix = format!("{}/", from);
        let to_prefix = format!("{}/", to);
        // Read all rows under `from` (exact + descendants).
        let rows: Vec<(String, String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query_as(
                "SELECT propertypath, propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = ? AND (propertypath = ? OR propertypath LIKE ?)",
            )
            .bind(userid.as_str())
            .bind(from)
            .bind(format!("{}%", from_prefix))
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::MySql(p) => sqlx::query_as(
                "SELECT propertypath, propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = ? AND (propertypath = ? OR propertypath LIKE ?)",
            )
            .bind(userid.as_str())
            .bind(from)
            .bind(format!("{}%", from_prefix))
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
            DbPool::Postgres(p) => sqlx::query_as(
                "SELECT propertypath, propertyname, propertyvalue FROM oc_properties \
                 WHERE userid = $1 AND (propertypath = $2 OR propertypath LIKE $3)",
            )
            .bind(userid.as_str())
            .bind(from)
            .bind(format!("{}%", from_prefix))
            .fetch_all(p)
            .await
            .map_err(FileCacheError::Db)?,
        };
        // Insert each row at the rewritten path.
        for (path, name, value) in rows {
            let new_path = if path == from {
                to.to_string()
            } else {
                format!("{}{}", to_prefix, &path[from_prefix.len()..])
            };
            self.upsert(userid, &new_path, &name, value.as_deref())
                .await?;
        }
        Ok(())
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
        let cfg = minimal_sqlite_config(dir.path().join("p.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    fn uid(s: &str) -> UserId {
        UserId::new(s).unwrap()
    }

    #[tokio::test]
    async fn upsert_then_get_returns_value() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "photos/cat.jpg", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        let rows = store.get(&u, "photos/cat.jpg").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].propertyname, "{oc:}favorite");
        assert_eq!(rows[0].propertyvalue.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn upsert_twice_overwrites() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "a", "{oc:}favorite", Some("0"))
            .await
            .unwrap();
        store
            .upsert(&u, "a", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        let rows = store.get(&u, "a").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].propertyvalue.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn delete_removes_one_prop() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "a", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        store
            .upsert(&u, "a", "{oc:}color", Some("red"))
            .await
            .unwrap();
        store.delete(&u, "a", "{oc:}favorite").await.unwrap();
        let rows = store.get(&u, "a").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].propertyname, "{oc:}color");
    }

    #[tokio::test]
    async fn delete_absent_is_noop() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store.delete(&u, "a", "{oc:}ghost").await.unwrap();
    }

    #[tokio::test]
    async fn get_many_batches_lookup() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "a", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        store
            .upsert(&u, "b", "{oc:}favorite", Some("0"))
            .await
            .unwrap();
        store.upsert(&u, "c", "{oc:}favorite", None).await.unwrap();
        let paths = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let rows = store.get_many(&u, &paths, "{oc:}favorite").await.unwrap();
        assert_eq!(rows.len(), 3);
        let map: std::collections::HashMap<_, _> = rows.into_iter().collect();
        assert_eq!(map.get("a").unwrap().as_deref(), Some("1"));
        assert_eq!(map.get("b").unwrap().as_deref(), Some("0"));
        assert_eq!(map.get("c").unwrap(), &None);
    }

    #[tokio::test]
    async fn rename_path_rewrites_exact_and_descendants() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "old", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        store
            .upsert(&u, "old/child.txt", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        store
            .upsert(&u, "old/sub/grand.txt", "{oc:}favorite", Some("0"))
            .await
            .unwrap();
        store
            .upsert(&u, "unrelated", "{oc:}favorite", Some("1"))
            .await
            .unwrap();

        store.rename_path(&u, "old", "new").await.unwrap();

        assert_eq!(store.get(&u, "old").await.unwrap().len(), 0);
        assert_eq!(store.get(&u, "old/child.txt").await.unwrap().len(), 0);
        assert_eq!(store.get(&u, "new").await.unwrap().len(), 1);
        assert_eq!(store.get(&u, "new/child.txt").await.unwrap().len(), 1);
        assert_eq!(store.get(&u, "new/sub/grand.txt").await.unwrap().len(), 1);
        assert_eq!(store.get(&u, "unrelated").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn copy_path_duplicates_subtree() {
        let store = PropertyStore::new(fresh_pool().await);
        let u = uid("alice");
        store
            .upsert(&u, "src", "{oc:}favorite", Some("1"))
            .await
            .unwrap();
        store
            .upsert(&u, "src/inner.txt", "{oc:}color", Some("blue"))
            .await
            .unwrap();

        store.copy_path(&u, "src", "dst").await.unwrap();

        // Source still present.
        assert_eq!(store.get(&u, "src").await.unwrap().len(), 1);
        // Dest mirrors source.
        assert_eq!(
            store.get(&u, "dst").await.unwrap()[0]
                .propertyvalue
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            store.get(&u, "dst/inner.txt").await.unwrap()[0]
                .propertyvalue
                .as_deref(),
            Some("blue")
        );
    }
}
