# WebDAV + Files API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Nextcloud-compatible WebDAV under `/remote.php/dav/files/<user>/<path>` (+ `/dav/...` alias) plus chunked uploads under `/dav/uploads/<user>/<id>/...`. After SP5 existing Nextcloud desktop/iOS/Android clients sync against Crabcloud unmodified.

**Architecture:** HTTP routes live in `crabcloud-http::routes::dav`; mounted twice in `build_router` (legacy + modern prefixes). `PropertyStore` + `LockStore` live in `crabcloud-filecache` and own `oc_properties` + `oc_filelocks`. All file mutations route through 4c's `AppState::view_for(uid)` / `uploads_for(uid)`. XML via `quick-xml`. AuthLayer (from 2b) gates everything.

**Tech Stack:** Rust 1.95 + axum 0.8 + `quick-xml` 0.40 + `urlencoding` 2 + `uuid` (workspace) + `httpdate` (workspace) + `dashmap` (workspace) + `sqlx` 0.8 (sqlite/mysql/postgres).

**Parent spec:** `docs/superpowers/specs/2026-05-12-webdav-and-files-api-design.md` (merged at master).

---

## Conventions

- **Commits:** Conventional Commits (`feat(http,webdav)`, `feat(filecache)`, `test(...)`, `docs(...)`) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD where practical:** test → fail → implement → pass → commit.
- **rustfmt:** `cargo fmt --all` before push.
- **`cargo xtask check-all` must pass before push.**
- **`-D warnings` workspace-wide.** New deps must be referenced immediately.
- **One PR per batch.** Stop at "PR opened, awaiting merge." Do NOT call `gh pr merge`.

---

## File Structure

```
crates/crabcloud-filecache/src/                          MODIFIED
├── lib.rs                                                + pub mod properties; pub mod locks;
├── properties.rs                                         NEW: PropertyStore + tests
└── locks.rs                                              NEW: LockStore + tests

crates/crabcloud-http/src/routes/dav/                    NEW MODULE TREE
├── mod.rs                                                router (dav_router) + AppState wiring
├── extractor.rs                                          UserPath extractor + auth-user resolver
├── headers.rs                                            Destination/Depth/If/Lock-Token/Timeout/Overwrite parsers
├── xml.rs                                                quick-xml writer helpers (Multistatus, lockdiscovery)
├── error.rs                                              DavError enum + IntoResponse mapping
├── methods.rs                                            OPTIONS, GET/HEAD, PUT, MKCOL, DELETE
├── moves.rs                                              MOVE, COPY (Destination + Overwrite)
├── propfind.rs                                           PROPFIND + props builder + Multistatus body
├── proppatch.rs                                          PROPPATCH + protected-prop list + path-rewrite hook
├── lock.rs                                               LOCK, UNLOCK, lock_check helper
└── uploads.rs                                            chunked upload routes (begin/put/commit/abort)

crates/crabcloud-http/src/router.rs                      MODIFIED + .nest twice for /remote.php/dav and /dav
crates/crabcloud-http/Cargo.toml                         MODIFIED + quick-xml + urlencoding + httpdate + uuid
crates/crabcloud-core/src/state.rs                       MODIFIED + upload_id_map: Arc<DashMap<String, String>>
migrations/core/0005_webdav_props_and_locks/             NEW
├── sqlite.sql
├── mysql.sql
└── postgres.sql
crates/crabcloud-db/src/core_migrations.rs               MODIFIED + Migration entry version=5
Cargo.toml                                                MODIFIED + urlencoding workspace dep

e2e/tests/webdav.spec.ts                                  NEW (Batch G)
docs/superpowers/plans/2026-05-12-webdav-and-files-api-implementation.changelog.md   NEW (Batch G)
README.md                                                 MODIFIED in Batch G
```

---

## Batches

| Batch | Tasks | Theme |
|-------|-------|---|
| **A** | 1 | Migration `0005` + `PropertyStore` + `LockStore` + unit tests |
| **B** | 2 | DAV router skeleton + `UserPath` extractor + OPTIONS + GET/HEAD/PUT/MKCOL/DELETE + conditional headers + Range |
| **C** | 3 | MOVE/COPY + Destination/Overwrite header parsing + delete-then-overwrite |
| **D** | 4 | PROPFIND (10 props, Depth 0/1) + Multistatus XML writer + Depth-infinity 403 |
| **E** | 5 | PROPPATCH + protected-prop rejection + path-rewrite on MOVE/COPY + `oc:favorite` round-trip |
| **F** | 6 | LOCK/UNLOCK + If-header parsing + ancestor-lock check + lock-aware mutation enforcement |
| **G** | 7 | Chunked-upload routes + in-process `upload_id_map` + Playwright e2e + changelog + README |

---

## Task 1: Migration + PropertyStore + LockStore (Batch A)

**Files:**
- Create: `migrations/core/0005_webdav_props_and_locks/{sqlite,mysql,postgres}.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs` (register version 5)
- Create: `crates/crabcloud-filecache/src/properties.rs`
- Create: `crates/crabcloud-filecache/src/locks.rs`
- Modify: `crates/crabcloud-filecache/src/lib.rs` (re-exports)
- Modify: `crates/crabcloud-db/src/core_migrations.rs::tests` (bump `applied` count)
- Modify: `crates/crabcloud-db/tests/migrate_end_to_end.rs` (bump 3 sibling counts + add new tables to DROP preludes)

### Step 1: Branch + migration files

```
git checkout -b webdav-batch-a origin/master
```

Create `migrations/core/0005_webdav_props_and_locks/sqlite.sql`:

```sql
CREATE TABLE oc_properties (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    userid         TEXT    NOT NULL,
    propertypath   TEXT    NOT NULL,
    propertyname   TEXT    NOT NULL,
    propertyvalue  TEXT    NULL
);
CREATE        INDEX oc_properties_pathonly ON oc_properties (userid, propertypath);
CREATE UNIQUE INDEX oc_properties_pathname ON oc_properties (userid, propertypath, propertyname);

CREATE TABLE oc_filelocks (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    key    TEXT    NOT NULL UNIQUE,
    ttl    INTEGER NOT NULL DEFAULT 86400,
    lock   INTEGER NOT NULL DEFAULT 0,
    token  TEXT    NULL,
    scope  TEXT    NULL,
    depth  TEXT    NULL,
    owner  TEXT    NULL
);
```

Create `migrations/core/0005_webdav_props_and_locks/mysql.sql`:

```sql
CREATE TABLE oc_properties (
    id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    userid         VARCHAR(64)     NOT NULL,
    propertypath   VARCHAR(4000)   NOT NULL,
    propertyname   VARCHAR(255)    NOT NULL,
    propertyvalue  LONGTEXT        NULL,
    PRIMARY KEY (id),
    KEY        oc_properties_pathonly (userid, propertypath),
    UNIQUE KEY oc_properties_pathname (userid, propertypath, propertyname(191))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_filelocks (
    id     BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    `key`  VARCHAR(2048)   NOT NULL,
    ttl    INT             NOT NULL DEFAULT 86400,
    `lock` INT             NOT NULL DEFAULT 0,
    token  VARCHAR(255)    NULL,
    scope  VARCHAR(32)     NULL,
    depth  VARCHAR(32)     NULL,
    owner  VARCHAR(2048)   NULL,
    PRIMARY KEY (id),
    UNIQUE KEY oc_filelocks_key (`key`(255))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
```

(MySQL's `lock` and `key` are reserved words; quoted with backticks. `(191)` index prefix avoids the 3072-byte InnoDB key limit on utf8mb4.)

Create `migrations/core/0005_webdav_props_and_locks/postgres.sql`:

```sql
CREATE TABLE oc_properties (
    id             BIGSERIAL     PRIMARY KEY,
    userid         VARCHAR(64)   NOT NULL,
    propertypath   VARCHAR(4000) NOT NULL,
    propertyname   VARCHAR(255)  NOT NULL,
    propertyvalue  TEXT          NULL
);
CREATE        INDEX oc_properties_pathonly ON oc_properties (userid, propertypath);
CREATE UNIQUE INDEX oc_properties_pathname ON oc_properties (userid, propertypath, propertyname);

CREATE TABLE oc_filelocks (
    id     BIGSERIAL    PRIMARY KEY,
    key    VARCHAR(2048) NOT NULL UNIQUE,
    ttl    INTEGER      NOT NULL DEFAULT 86400,
    lock   INTEGER      NOT NULL DEFAULT 0,
    token  VARCHAR(255) NULL,
    scope  VARCHAR(32)  NULL,
    depth  VARCHAR(32)  NULL,
    owner  VARCHAR(2048) NULL
);
```

### Step 2: Register migration version 5

Modify `crates/crabcloud-db/src/core_migrations.rs`. Append after the `version: 4` entry:

```rust
    Migration {
        version: 5,
        name: "webdav_props_and_locks",
        sqlite: include_str!("../../../migrations/core/0005_webdav_props_and_locks/sqlite.sql"),
        mysql: include_str!("../../../migrations/core/0005_webdav_props_and_locks/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0005_webdav_props_and_locks/postgres.sql"),
    },
```

In the same file's inline `core_migration_applies_against_sqlite` test, change `assert_eq!(applied, 4)` → `assert_eq!(applied, 5)`.

In `crates/crabcloud-db/tests/migrate_end_to_end.rs`, find all 3 `assert_eq!(applied, 4)` (sqlite/mysql/postgres arms) and bump to `5`. Also: add `oc_properties` and `oc_filelocks` to the MySQL and Postgres DROP TABLE IF EXISTS preludes (at the top of the existing arrays, BEFORE the existing entries — no FK dependencies between these two new tables, but put them first for safety).

### Step 3: Create `crates/crabcloud-filecache/src/properties.rs`

```rust
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
                let mut q = sqlx::query_as(&sql).bind(userid.as_str()).bind(propertyname);
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
                let mut q = sqlx::query_as(&sql).bind(userid.as_str()).bind(propertyname);
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
                let mut q = sqlx::query_as(&sql).bind(userid.as_str()).bind(propertyname);
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
    pub async fn rename_path(
        &self,
        userid: &UserId,
        from: &str,
        to: &str,
    ) -> FileCacheResult<()> {
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
    pub async fn copy_path(
        &self,
        userid: &UserId,
        from: &str,
        to: &str,
    ) -> FileCacheResult<()> {
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
            self.upsert(userid, &new_path, &name, value.as_deref()).await?;
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
        store.upsert(&u, "a", "{oc:}favorite", Some("1")).await.unwrap();
        store.upsert(&u, "a", "{oc:}color", Some("red")).await.unwrap();
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
        store.upsert(&u, "a", "{oc:}favorite", Some("1")).await.unwrap();
        store.upsert(&u, "b", "{oc:}favorite", Some("0")).await.unwrap();
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
        store.upsert(&u, "old", "{oc:}favorite", Some("1")).await.unwrap();
        store.upsert(&u, "old/child.txt", "{oc:}favorite", Some("1")).await.unwrap();
        store.upsert(&u, "old/sub/grand.txt", "{oc:}favorite", Some("0")).await.unwrap();
        store.upsert(&u, "unrelated", "{oc:}favorite", Some("1")).await.unwrap();

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
        store.upsert(&u, "src", "{oc:}favorite", Some("1")).await.unwrap();
        store.upsert(&u, "src/inner.txt", "{oc:}color", Some("blue")).await.unwrap();

        store.copy_path(&u, "src", "dst").await.unwrap();

        // Source still present.
        assert_eq!(store.get(&u, "src").await.unwrap().len(), 1);
        // Dest mirrors source.
        assert_eq!(store.get(&u, "dst").await.unwrap()[0].propertyvalue.as_deref(), Some("1"));
        assert_eq!(store.get(&u, "dst/inner.txt").await.unwrap()[0].propertyvalue.as_deref(), Some("blue"));
    }
}
```

### Step 4: Create `crates/crabcloud-filecache/src/locks.rs`

```rust
//! `oc_filelocks` — exclusive WebDAV locks. SP5 ships exclusive scope only.
//!
//! Keyed by `"files/{uid}/{path}"`. TTL is unix-ts; expired rows persist until
//! a future `crabcloud locks:gc` reaps them. `acquire` upserts, overwriting any
//! stale row.

use crabcloud_db::DbPool;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{FileCacheError, FileCacheResult};

pub struct LockStore {
    pool: DbPool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRow {
    pub key: String,
    pub ttl: i64,
    pub token: String,
    pub scope: String,
    pub depth: String,
    pub owner: Option<String>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl LockStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Return the current lock for `key` if it exists AND is unexpired.
    /// Returns `None` if no row or if `ttl <= now`.
    pub async fn current(&self, key: &str) -> FileCacheResult<Option<LockRow>> {
        let n = now_unix();
        let row: Option<(String, i64, Option<String>, Option<String>, Option<String>, Option<String>)> =
            match &self.pool {
                DbPool::Sqlite(p) => sqlx::query_as(
                    "SELECT key, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE key = ? AND (ttl = 0 OR ttl > ?)",
                )
                .bind(key)
                .bind(n)
                .fetch_optional(p)
                .await
                .map_err(FileCacheError::Db)?,
                DbPool::MySql(p) => sqlx::query_as(
                    "SELECT `key`, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE `key` = ? AND (ttl = 0 OR ttl > ?)",
                )
                .bind(key)
                .bind(n)
                .fetch_optional(p)
                .await
                .map_err(FileCacheError::Db)?,
                DbPool::Postgres(p) => sqlx::query_as(
                    "SELECT key, ttl, token, scope, depth, owner FROM oc_filelocks \
                     WHERE key = $1 AND (ttl = 0 OR ttl > $2)",
                )
                .bind(key)
                .bind(n as i32)
                .fetch_optional(p)
                .await
                .map_err(FileCacheError::Db)?,
            };
        Ok(row.map(|(key, ttl, token, scope, depth, owner)| LockRow {
            key,
            ttl,
            token: token.unwrap_or_default(),
            scope: scope.unwrap_or_else(|| "exclusive".into()),
            depth: depth.unwrap_or_else(|| "0".into()),
            owner,
        }))
    }

    /// Acquire a lock. Upserts: stale rows for the same key get overwritten.
    pub async fn acquire(
        &self,
        key: &str,
        token: &str,
        scope: &str,
        depth: &str,
        owner: Option<&str>,
        ttl: i64,
    ) -> FileCacheResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (key, ttl, lock, token, scope, depth, owner) \
                     VALUES (?, ?, -1, ?, ?, ?, ?) \
                     ON CONFLICT(key) DO UPDATE SET \
                       ttl = excluded.ttl, lock = -1, token = excluded.token, \
                       scope = excluded.scope, depth = excluded.depth, owner = excluded.owner",
                )
                .bind(key)
                .bind(ttl)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (`key`, ttl, `lock`, token, scope, depth, owner) \
                     VALUES (?, ?, -1, ?, ?, ?, ?) \
                     ON DUPLICATE KEY UPDATE \
                       ttl = VALUES(ttl), `lock` = -1, token = VALUES(token), \
                       scope = VALUES(scope), depth = VALUES(depth), owner = VALUES(owner)",
                )
                .bind(key)
                .bind(ttl)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "INSERT INTO oc_filelocks (key, ttl, lock, token, scope, depth, owner) \
                     VALUES ($1, $2, -1, $3, $4, $5, $6) \
                     ON CONFLICT (key) DO UPDATE SET \
                       ttl = EXCLUDED.ttl, lock = -1, token = EXCLUDED.token, \
                       scope = EXCLUDED.scope, depth = EXCLUDED.depth, owner = EXCLUDED.owner",
                )
                .bind(key)
                .bind(ttl as i32)
                .bind(token)
                .bind(scope)
                .bind(depth)
                .bind(owner)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            }
        }
        Ok(())
    }

    /// Release a lock by `(key, token)`. Returns `true` if a row was deleted,
    /// `false` if no such row.
    pub async fn release(&self, key: &str, token: &str) -> FileCacheResult<bool> {
        let rows_affected = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query(
                "DELETE FROM oc_filelocks WHERE key = ? AND token = ?",
            )
            .bind(key)
            .bind(token)
            .execute(p)
            .await
            .map_err(FileCacheError::Db)?
            .rows_affected(),
            DbPool::MySql(p) => sqlx::query(
                "DELETE FROM oc_filelocks WHERE `key` = ? AND token = ?",
            )
            .bind(key)
            .bind(token)
            .execute(p)
            .await
            .map_err(FileCacheError::Db)?
            .rows_affected(),
            DbPool::Postgres(p) => sqlx::query(
                "DELETE FROM oc_filelocks WHERE key = $1 AND token = $2",
            )
            .bind(key)
            .bind(token)
            .execute(p)
            .await
            .map_err(FileCacheError::Db)?
            .rows_affected(),
        };
        Ok(rows_affected > 0)
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
        let cfg = minimal_sqlite_config(dir.path().join("l.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn acquire_then_current_returns_lock() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire("files/alice/a.txt", "urn:uuid:t1", "exclusive", "0", Some("alice"), ttl)
            .await
            .unwrap();
        let lock = store.current("files/alice/a.txt").await.unwrap().unwrap();
        assert_eq!(lock.token, "urn:uuid:t1");
        assert_eq!(lock.scope, "exclusive");
        assert_eq!(lock.depth, "0");
    }

    #[tokio::test]
    async fn current_returns_none_for_absent() {
        let store = LockStore::new(fresh_pool().await);
        assert!(store.current("files/ghost/x").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn current_returns_none_for_expired() {
        let store = LockStore::new(fresh_pool().await);
        let ttl_past = now_unix() - 10;
        store
            .acquire("files/alice/a", "urn:uuid:t", "exclusive", "0", None, ttl_past)
            .await
            .unwrap();
        assert!(store.current("files/alice/a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_correct_token_succeeds() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire("files/alice/a", "urn:uuid:t", "exclusive", "0", None, ttl)
            .await
            .unwrap();
        let ok = store.release("files/alice/a", "urn:uuid:t").await.unwrap();
        assert!(ok);
        assert!(store.current("files/alice/a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_wrong_token_fails() {
        let store = LockStore::new(fresh_pool().await);
        let ttl = now_unix() + 1800;
        store
            .acquire("files/alice/a", "urn:uuid:t1", "exclusive", "0", None, ttl)
            .await
            .unwrap();
        let ok = store.release("files/alice/a", "urn:uuid:other").await.unwrap();
        assert!(!ok);
        // Lock still present.
        assert!(store.current("files/alice/a").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn acquire_overwrites_expired_row() {
        let store = LockStore::new(fresh_pool().await);
        let past = now_unix() - 10;
        store.acquire("files/a", "urn:uuid:old", "exclusive", "0", None, past).await.unwrap();
        // New lock with the same key but a different token.
        let future = now_unix() + 1800;
        store.acquire("files/a", "urn:uuid:new", "exclusive", "0", None, future).await.unwrap();
        let lock = store.current("files/a").await.unwrap().unwrap();
        assert_eq!(lock.token, "urn:uuid:new");
    }
}
```

### Step 5: Update `crates/crabcloud-filecache/src/lib.rs`

Add the new module declarations + re-exports:

```rust
pub mod locks;
pub mod properties;

// In the re-exports section, add:
pub use locks::{LockRow, LockStore};
pub use properties::{PropertyRow, PropertyStore};
```

### Step 6: Run + commit + push + open Batch A PR

```
cargo test -p crabcloud-filecache --lib
cargo test -p crabcloud-db --lib core_migration_applies_against_sqlite
cargo xtask check-all
```

Expected: ~12 new tests (properties: 7, locks: 5) pass; migration count test asserts 5.

```
git add migrations/core/0005_webdav_props_and_locks crates/crabcloud-db crates/crabcloud-filecache
git commit -m "feat(filecache): migration 0005 + PropertyStore + LockStore for WebDAV

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin webdav-batch-a
gh pr create --base master --head webdav-batch-a \
  --title "webdav: batch A — migration 0005 + PropertyStore + LockStore" \
  --body "Sub-project 5, batch A: migration \`0005_webdav_props_and_locks\` creates \`oc_properties\` + \`oc_filelocks\` on sqlite/mysql/postgres. PropertyStore (path-keyed; \`rename_path\` for MOVE) + LockStore (exclusive only; TTL-based expiry) in crabcloud-filecache. 12 unit tests. WebDAV HTTP routes start in batch B."
```

**STOP. Do NOT call `gh pr merge`.**

---

## Task 2: DAV router skeleton + basic methods (Batch B)

**Files:**
- Modify: `Cargo.toml` (workspace deps: add `urlencoding = "2"`)
- Modify: `crates/crabcloud-http/Cargo.toml` (consume quick-xml, urlencoding, httpdate, uuid)
- Create: `crates/crabcloud-http/src/routes/dav/{mod,extractor,headers,error,methods}.rs`
- Modify: `crates/crabcloud-http/src/router.rs` (nest dav_router at both prefixes)
- Modify: `crates/crabcloud-http/src/routes/mod.rs` (add `pub mod dav;`)
- Create: `crates/crabcloud-http/tests/dav_basic.rs` (integration tests)

### Step 1: Branch + workspace dep

```
git checkout -b webdav-batch-b origin/master
```

Add `urlencoding = "2"` to workspace `[workspace.dependencies]` alphabetically.

### Step 2: Update `crates/crabcloud-http/Cargo.toml`

In `[dependencies]`, add:

```toml
httpdate.workspace = true
quick-xml.workspace = true
urlencoding.workspace = true
uuid = { workspace = true, features = ["v4"] }
```

(`uuid` workspace dep is already present; the `v4` feature is what we need for lock-token UUIDs in Batch F — declare it here once.)

### Step 3: Create `crates/crabcloud-http/src/routes/dav/mod.rs`

```rust
//! WebDAV route surface. Mounted by `crate::router::build_router` at
//! BOTH `/remote.php/dav` (legacy) and `/dav` (modern alias).

pub mod error;
pub mod extractor;
pub mod headers;
pub mod methods;

use axum::routing::{any, MethodRouter};
use axum::Router;
use crabcloud_core::AppState;

/// Builds the DAV router. All routes are auth-gated by the outer AuthLayer.
pub fn dav_router() -> Router<AppState> {
    Router::new()
        // /files/{user}/{*path} — the main WebDAV surface.
        // axum 0.8 wildcard path: `{*path}` captures the rest.
        .route(
            "/files/{user}/{*path}",
            any(methods::dispatch_files),
        )
        // /files/{user} — root of the user's filesystem (path is empty).
        .route("/files/{user}", any(methods::dispatch_files_root))
        // /files (root of all users) — OPTIONS only; returns DAV class.
        .route("/files", method_options_only())
}

/// `MethodRouter` that responds to OPTIONS only with DAV capability.
fn method_options_only() -> MethodRouter<AppState> {
    use axum::routing::options;
    options(methods::options_capability_root)
}
```

### Step 4: Create `crates/crabcloud-http/src/routes/dav/error.rs`

```rust
//! `DavError` — protocol-aware error type that converts to the right HTTP
//! status + (for some variants) a small XML body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_filecache::FileCacheError;
use crabcloud_fs::FsError;
use crabcloud_storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum DavError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict")]
    Conflict,
    #[error("precondition failed")]
    PreconditionFailed,
    #[error("locked")]
    Locked,
    #[error("range not satisfiable")]
    RangeNotSatisfiable { file_size: u64 },
    #[error("propfind-finite-depth")]
    PropfindFiniteDepth,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("filecache: {0}")]
    FileCache(#[from] FileCacheError),
    #[error("fs: {0}")]
    Fs(#[from] FsError),
}

impl IntoResponse for DavError {
    fn into_response(self) -> Response {
        use axum::http::header;
        match self {
            DavError::NotFound => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::Forbidden => (StatusCode::FORBIDDEN, "").into_response(),
            DavError::Conflict => (StatusCode::CONFLICT, "").into_response(),
            DavError::PreconditionFailed => (StatusCode::PRECONDITION_FAILED, "").into_response(),
            DavError::Locked => (StatusCode::from_u16(423).unwrap(), "").into_response(),
            DavError::RangeNotSatisfiable { file_size } => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{}", file_size))],
                "",
            )
                .into_response(),
            DavError::PropfindFiniteDepth => {
                let body = r#"<?xml version="1.0" encoding="utf-8"?><d:error xmlns:d="DAV:"><d:propfind-finite-depth/></d:error>"#;
                (
                    StatusCode::FORBIDDEN,
                    [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                    body,
                )
                    .into_response()
            }
            DavError::BadRequest(m) => (StatusCode::BAD_REQUEST, m).into_response(),
            DavError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
            DavError::Storage(StorageError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::Storage(StorageError::AlreadyExists) => (StatusCode::from_u16(405).unwrap(), "").into_response(),
            DavError::Storage(StorageError::NotEmpty) => (StatusCode::from_u16(409).unwrap(), "").into_response(),
            DavError::Storage(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("storage error: {e}"),
            )
                .into_response(),
            DavError::FileCache(FileCacheError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::FileCache(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("filecache error: {e}"),
            )
                .into_response(),
            DavError::Fs(FsError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::Fs(FsError::InvalidPath(m)) => (StatusCode::BAD_REQUEST, m).into_response(),
            DavError::Fs(FsError::CrossMount) => (StatusCode::from_u16(502).unwrap(), "").into_response(),
            DavError::Fs(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("fs error: {e}"),
            )
                .into_response(),
        }
    }
}

pub type DavResult<T> = Result<T, DavError>;
```

### Step 5: Create `crates/crabcloud-http/src/routes/dav/extractor.rs`

```rust
//! Extract `(uid, user_path)` from a DAV request. The URL is shaped
//! `/files/{user}/{*path}` where `path` may be empty (root). The
//! authenticated user MUST match `{user}` — cross-user access lives in
//! the sharing sub-project.

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;

/// Validate that `url_user` matches `authed.user_id` and produce a `(UserId, UserPath)`.
/// `url_path` is the captured wildcard segment (may be empty).
pub fn resolve_target(
    authed: &AuthenticatedUser,
    url_user: &str,
    url_path: &str,
) -> DavResult<(UserId, UserPath)> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid = UserId::new(url_user)
        .map_err(|e| DavError::BadRequest(format!("invalid user id: {e}")))?;
    // url_path is the captured rest after `/files/{user}/`. The leading `/` is
    // already consumed by axum's path-template; prepend it for UserPath.
    let user_path = if url_path.is_empty() {
        UserPath::root()
    } else {
        // URL-decode in case the client percent-encoded segments.
        let decoded = urlencoding::decode(url_path)
            .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
        UserPath::new(format!("/{}", decoded))
            .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?
    };
    Ok((uid, user_path))
}
```

### Step 6: Create `crates/crabcloud-http/src/routes/dav/headers.rs`

```rust
//! Header parsers for DAV: Destination, Depth, If, Lock-Token, Timeout, Overwrite.

use axum::http::HeaderMap;
use std::ops::Range;

use crate::routes::dav::error::{DavError, DavResult};

/// Parse the `Depth:` header. Returns one of `0`, `1`, or an error for
/// `infinity` (the caller decides whether that's allowed). Default `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Zero,
    One,
    Infinity,
}

pub fn parse_depth(headers: &HeaderMap, default: Depth) -> DavResult<Depth> {
    match headers.get("depth").and_then(|v| v.to_str().ok()) {
        None => Ok(default),
        Some("0") => Ok(Depth::Zero),
        Some("1") => Ok(Depth::One),
        Some("infinity") => Ok(Depth::Infinity),
        Some(other) => Err(DavError::BadRequest(format!("invalid Depth: {other}"))),
    }
}

/// Parse the `Overwrite:` header. `T` (default) or `F`.
pub fn parse_overwrite(headers: &HeaderMap) -> DavResult<bool> {
    match headers.get("overwrite").and_then(|v| v.to_str().ok()) {
        None | Some("T") => Ok(true),
        Some("F") => Ok(false),
        Some(other) => Err(DavError::BadRequest(format!("invalid Overwrite: {other}"))),
    }
}

/// Parse the `Destination:` header. Accepts both absolute URL (strips
/// `<scheme>://<host>` prefix up to and including the first `/dav` or
/// `/remote.php/dav`) and path-only forms. Returns the user-facing path
/// segment after `/dav/files/{user}/`.
///
/// Returns the captured `(user, path)` pair. The handler then validates
/// the user matches and constructs a `UserPath`.
pub fn parse_destination_files(
    headers: &HeaderMap,
) -> DavResult<(String, String)> {
    let raw = headers
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| DavError::BadRequest("missing Destination header".into()))?;
    // Strip scheme+host if absolute.
    let path = if let Some(idx) = raw.find("://") {
        let after_scheme = &raw[idx + 3..];
        match after_scheme.find('/') {
            Some(slash) => &after_scheme[slash..],
            None => return Err(DavError::BadRequest("Destination missing path".into())),
        }
    } else {
        raw
    };
    // Find the `/files/` segment after either prefix.
    let after_files = path
        .strip_prefix("/remote.php/dav/files/")
        .or_else(|| path.strip_prefix("/dav/files/"))
        .ok_or_else(|| DavError::BadRequest(format!("Destination not under /dav/files/: {raw}")))?;
    // Split into user + path.
    match after_files.find('/') {
        Some(slash) => Ok((after_files[..slash].into(), after_files[slash + 1..].into())),
        None => Ok((after_files.into(), String::new())),
    }
}

/// Parse a Range header value `bytes=N-M`. Returns the byte range.
/// Errors on multi-range (`bytes=0-499,1000-1499`).
pub fn parse_range(headers: &HeaderMap, file_size: u64) -> DavResult<Option<Range<u64>>> {
    let raw = match headers.get("range").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return Ok(None),
    };
    let rest = raw
        .strip_prefix("bytes=")
        .ok_or_else(|| DavError::RangeNotSatisfiable { file_size })?;
    if rest.contains(',') {
        return Err(DavError::RangeNotSatisfiable { file_size });
    }
    let (start_s, end_s) = rest
        .split_once('-')
        .ok_or(DavError::RangeNotSatisfiable { file_size })?;
    let range = match (start_s.is_empty(), end_s.is_empty()) {
        (false, false) => {
            let start: u64 = start_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            let end: u64 = end_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if end < start || end >= file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            start..(end + 1)
        }
        (false, true) => {
            let start: u64 = start_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if start >= file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            start..file_size
        }
        (true, false) => {
            // `bytes=-N` means the last N bytes.
            let suffix: u64 = end_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if suffix == 0 || suffix > file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            (file_size - suffix)..file_size
        }
        (true, true) => return Err(DavError::RangeNotSatisfiable { file_size }),
    };
    Ok(Some(range))
}

#[derive(Debug, Clone)]
pub enum IfMatch {
    Absent,
    Wildcard,
    Etag(String),
}

pub fn parse_if_match(headers: &HeaderMap) -> IfMatch {
    match headers.get("if-match").and_then(|v| v.to_str().ok()) {
        None => IfMatch::Absent,
        Some("*") => IfMatch::Wildcard,
        Some(raw) => {
            // Strip surrounding quotes if present.
            let s = raw.trim();
            let unquoted = s.trim_matches('"');
            IfMatch::Etag(unquoted.to_string())
        }
    }
}

pub fn parse_if_none_match_wildcard(headers: &HeaderMap) -> bool {
    headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim() == "*")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, axum::http::HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn depth_default() {
        let h = hm(&[]);
        assert_eq!(parse_depth(&h, Depth::One).unwrap(), Depth::One);
    }

    #[test]
    fn depth_zero_one_infinity() {
        assert_eq!(parse_depth(&hm(&[("depth", "0")]), Depth::One).unwrap(), Depth::Zero);
        assert_eq!(parse_depth(&hm(&[("depth", "1")]), Depth::One).unwrap(), Depth::One);
        assert_eq!(
            parse_depth(&hm(&[("depth", "infinity")]), Depth::One).unwrap(),
            Depth::Infinity
        );
    }

    #[test]
    fn overwrite_default_true() {
        assert!(parse_overwrite(&hm(&[])).unwrap());
        assert!(parse_overwrite(&hm(&[("overwrite", "T")])).unwrap());
        assert!(!parse_overwrite(&hm(&[("overwrite", "F")])).unwrap());
    }

    #[test]
    fn destination_absolute_url() {
        let h = hm(&[("destination", "https://example.com/dav/files/alice/photos/cat.jpg")]);
        let (u, p) = parse_destination_files(&h).unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "photos/cat.jpg");
    }

    #[test]
    fn destination_path_only_legacy_prefix() {
        let h = hm(&[("destination", "/remote.php/dav/files/alice/x.txt")]);
        let (u, p) = parse_destination_files(&h).unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "x.txt");
    }

    #[test]
    fn destination_missing_header_errors() {
        assert!(matches!(
            parse_destination_files(&hm(&[])),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn range_simple() {
        let r = parse_range(&hm(&[("range", "bytes=0-9")]), 100).unwrap().unwrap();
        assert_eq!(r, 0..10);
    }

    #[test]
    fn range_open_end() {
        let r = parse_range(&hm(&[("range", "bytes=50-")]), 100).unwrap().unwrap();
        assert_eq!(r, 50..100);
    }

    #[test]
    fn range_suffix() {
        let r = parse_range(&hm(&[("range", "bytes=-10")]), 100).unwrap().unwrap();
        assert_eq!(r, 90..100);
    }

    #[test]
    fn range_invalid_rejects() {
        assert!(matches!(
            parse_range(&hm(&[("range", "bytes=500-999")]), 100),
            Err(DavError::RangeNotSatisfiable { .. })
        ));
        assert!(matches!(
            parse_range(&hm(&[("range", "bytes=0-99,100-199")]), 200),
            Err(DavError::RangeNotSatisfiable { .. })
        ));
    }

    #[test]
    fn if_match_parsing() {
        assert!(matches!(parse_if_match(&hm(&[])), IfMatch::Absent));
        assert!(matches!(parse_if_match(&hm(&[("if-match", "*")])), IfMatch::Wildcard));
        match parse_if_match(&hm(&[("if-match", r#""abc""#)])) {
            IfMatch::Etag(s) => assert_eq!(s, "abc"),
            _ => panic!(),
        }
    }

    #[test]
    fn if_none_match_star() {
        assert!(parse_if_none_match_wildcard(&hm(&[("if-none-match", "*")])));
        assert!(!parse_if_none_match_wildcard(&hm(&[])));
    }
}
```

### Step 7: Create `crates/crabcloud-http/src/routes/dav/methods.rs`

This file is large; the plan provides a complete listing. Continues in `webdav-batch-b` step 8.

Create the file with the following content (handles OPTIONS, GET/HEAD, PUT, MKCOL, DELETE; MOVE/COPY land in Batch C; PROPFIND/PROPPATCH/LOCK/UNLOCK in D/E/F; uploads in G):

```rust
//! WebDAV method handlers. Each handler is dispatched by HTTP method via
//! `dispatch_files` (axum's `any` route).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_storage::FileKind;
use tokio_util::io::ReaderStream;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::extractor::resolve_target;
use crate::routes::dav::headers::{
    parse_if_match, parse_if_none_match_wildcard, parse_range, IfMatch,
};

/// Default Allow header listing methods SP5 supports.
const ALLOW_HEADER: &str = "OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY, PROPFIND, PROPPATCH, LOCK, UNLOCK";

/// `OPTIONS /dav/files` — root capability probe (no user context).
pub async fn options_capability_root() -> Response {
    capability_response()
}

fn capability_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::ALLOW, HeaderValue::from_static(ALLOW_HEADER)),
            ("dav", HeaderValue::from_static("1, 2, 3")),
            ("ms-author-via", HeaderValue::from_static("DAV")),
        ],
        "",
    )
        .into_response()
}

/// Dispatch by method for `/dav/files/{user}` (path is root).
pub async fn dispatch_files_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path(user): Path<String>,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    dispatch_inner(state, authed, headers, user, String::new(), method, body).await
}

/// Dispatch for `/dav/files/{user}/{*path}`.
pub async fn dispatch_files(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((user, path)): Path<(String, String)>,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    dispatch_inner(state, authed, headers, user, path, method, body).await
}

async fn dispatch_inner(
    state: AppState,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    url_user: String,
    url_path: String,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    let (uid, user_path) = resolve_target(&authed, &url_user, &url_path)?;
    match method {
        Method::OPTIONS => Ok(capability_response()),
        Method::GET | Method::HEAD => {
            get_or_head(state, &uid, &user_path, &headers, method == Method::HEAD).await
        }
        Method::PUT => put(state, &uid, &user_path, &headers, body).await,
        m if m.as_str() == "MKCOL" => mkcol(state, &uid, &user_path).await,
        Method::DELETE => delete(state, &uid, &user_path).await,
        // MOVE/COPY land in Batch C.
        m if m.as_str() == "MOVE" || m.as_str() == "COPY" => Err(DavError::BadRequest(
            "MOVE/COPY not yet implemented".into(),
        )),
        // PROPFIND/PROPPATCH/LOCK/UNLOCK land in batches D/E/F.
        m if matches!(
            m.as_str(),
            "PROPFIND" | "PROPPATCH" | "LOCK" | "UNLOCK"
        ) =>
        {
            Err(DavError::BadRequest(format!(
                "{} not yet implemented",
                m.as_str()
            )))
        }
        _ => Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, HeaderValue::from_static(ALLOW_HEADER))],
            "",
        )
            .into_response()),
    }
}

async fn get_or_head(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
    headers: &HeaderMap,
    head_only: bool,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    let meta = view.stat(user_path).await?;
    if matches!(meta.kind, FileKind::Directory) {
        return Err(DavError::BadRequest("GET on a directory".into()));
    }
    let etag = format!("\"{}\"", meta.etag.as_str());
    let last_mod = httpdate::fmt_http_date(meta.mtime);

    // Range handling.
    let range = parse_range(headers, meta.size)?;
    let (status, content_length, content_range, body) = match range {
        None => {
            let reader = view.read(user_path).await?;
            let stream = ReaderStream::new(reader);
            (
                StatusCode::OK,
                meta.size,
                None,
                if head_only {
                    Body::empty()
                } else {
                    Body::from_stream(stream)
                },
            )
        }
        Some(r) => {
            let length = r.end - r.start;
            let cr = format!("bytes {}-{}/{}", r.start, r.end - 1, meta.size);
            let reader = view.read_range(user_path, r).await?;
            let stream = ReaderStream::new(reader);
            (
                StatusCode::PARTIAL_CONTENT,
                length,
                Some(cr),
                if head_only {
                    Body::empty()
                } else {
                    Body::from_stream(stream)
                },
            )
        }
    };

    let mut resp = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::CONTENT_TYPE, meta.mimetype.as_str())
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_mod)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cr) = content_range {
        resp = resp.header(header::CONTENT_RANGE, cr);
    }
    resp.body(body)
        .map_err(|e| DavError::Internal(format!("response build: {e}")))
}

async fn put(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;

    // Conditional checks: resolve target IF needed.
    let if_match = parse_if_match(headers);
    let if_none_match_star = parse_if_none_match_wildcard(headers);
    let existing = view.stat(user_path).await.ok();
    match (&if_match, &existing) {
        (IfMatch::Wildcard, None) => return Err(DavError::PreconditionFailed),
        (IfMatch::Etag(want), Some(meta)) if meta.etag.as_str() != want => {
            return Err(DavError::PreconditionFailed);
        }
        (IfMatch::Etag(_), None) => return Err(DavError::PreconditionFailed),
        _ => {}
    }
    if if_none_match_star && existing.is_some() {
        return Err(DavError::PreconditionFailed);
    }

    let stream = body.into_data_stream();
    let body_reader = tokio_util::io::StreamReader::new(
        stream.map(|r| r.map_err(|e| std::io::Error::other(e.to_string()))),
    );
    let pinned: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = Box::pin(body_reader);

    let meta = view.put_file(user_path, pinned).await?;
    let status = if existing.is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    let etag = format!("\"{}\"", meta.etag.as_str());
    Ok((
        status,
        [
            (header::ETAG, HeaderValue::from_str(&etag).unwrap()),
            (
                header::LAST_MODIFIED,
                HeaderValue::from_str(&httpdate::fmt_http_date(meta.mtime)).unwrap(),
            ),
        ],
        "",
    )
        .into_response())
}

async fn mkcol(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.mkdir(user_path).await?;
    Ok((StatusCode::CREATED, "").into_response())
}

async fn delete(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.delete(user_path).await?;
    Ok((StatusCode::NO_CONTENT, "").into_response())
}

// futures::StreamExt is needed for body.map() above.
use futures::StreamExt as _;
```

### Step 8: Wire dav_router into router.rs

Modify `crates/crabcloud-http/src/router.rs`. In `build_router`, after the `.nest("/ocs", ...)` line, add:

```rust
        .nest(
            "/remote.php/dav",
            crate::routes::dav::dav_router().with_state(state.clone()),
        )
        .nest(
            "/dav",
            crate::routes::dav::dav_router().with_state(state.clone()),
        )
```

And add `pub mod dav;` to `crates/crabcloud-http/src/routes/mod.rs`.

### Step 9: Integration tests

Create `crates/crabcloud-http/tests/dav_basic.rs`:

```rust
//! Integration tests for batch B: OPTIONS, GET/HEAD/PUT/MKCOL/DELETE,
//! conditional headers, single Range support, /remote.php/dav alias.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use crabcloud_users::{BcryptVerifier, GroupId, PasswordVerifier, SqlGroupStore, User, UserId};
use secrecy::ExposeSecret;
use tempfile::tempdir;
use tower::ServiceExt;

// Anchor workspace deps used only by sibling tests.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use hex as _;
use thiserror as _;
use tracing as _;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> crabcloud_core::AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    state
}

async fn login_cookie(state: &crabcloud_core::AppState) -> String {
    use crabcloud_users::AuthTokenType;
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "test",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    let cookie_value = crate::session::encode_cookie(
        raw.expose(),
        state.config.secret.expose_secret().as_bytes(),
    );
    format!("{}={}", crate::session::COOKIE_NAME, cookie_value)
}

#[tokio::test]
async fn options_returns_dav_class_and_allow() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/alice")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("dav").unwrap().to_str().unwrap(),
        "1, 2, 3"
    );
    let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("PROPFIND"));
    assert!(allow.contains("LOCK"));
}

#[tokio::test]
async fn put_creates_file_returns_201_etag() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/hello.txt")
        .header("cookie", cookie)
        .body(Body::from("hello world"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = resp.headers().get("etag").unwrap().to_str().unwrap();
    assert!(etag.starts_with('"') && etag.ends_with('"') && etag.len() == 42);
}

#[tokio::test]
async fn put_with_if_none_match_star_on_existing_returns_412() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/x.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("v1"))
        .unwrap();
    let resp1 = app.clone().oneshot(r1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::CREATED);

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/x.txt")
        .header("cookie", cookie)
        .header("if-none-match", "*")
        .body(Body::from("v2"))
        .unwrap();
    let resp2 = app.oneshot(r2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn put_with_if_match_mismatch_returns_412() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/y.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("v1"))
        .unwrap();
    app.clone().oneshot(r1).await.unwrap();

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/y.txt")
        .header("cookie", cookie)
        .header("if-match", "\"wrong-etag\"")
        .body(Body::from("v2"))
        .unwrap();
    let resp = app.oneshot(r2).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn get_returns_file_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("g.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/hi.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("hello"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/hi.txt")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"hello");
}

#[tokio::test]
async fn get_with_range_returns_206() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("r.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/big.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("0123456789"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/big.txt")
        .header("cookie", cookie)
        .header("range", "bytes=2-5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let cr = resp
        .headers()
        .get("content-range")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cr, "bytes 2-5/10");
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"2345");
}

#[tokio::test]
async fn get_with_invalid_range_returns_416() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("r.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/small.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("hi"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/small.txt")
        .header("cookie", cookie)
        .header("range", "bytes=100-200")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn mkcol_creates_directory() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("MKCOL")
        .uri("/dav/files/alice/newdir")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn delete_removes_file_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("d.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/to-delete.txt")
        .header("cookie", cookie.clone())
        .body(Body::from("bye"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let del = Request::builder()
        .method("DELETE")
        .uri("/dav/files/alice/to-delete.txt")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(del).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn legacy_remote_php_dav_alias_works() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/remote.php/dav/files/alice")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn unauthenticated_dav_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/alice")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cross_user_access_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/bob")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
```

### Step 10: Run + commit + push + open Batch B PR

```
cargo test -p crabcloud-http --tests dav_basic
cargo xtask check-all
```

Expected: ~13 integration tests + ~15 header parser unit tests pass; build clean.

```
git add Cargo.toml crates/crabcloud-http
git commit -m "feat(http,webdav): DAV router skeleton + basic methods + conditional + Range

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin webdav-batch-b
gh pr create --base master --head webdav-batch-b \
  --title "webdav: batch B — router skeleton + OPTIONS/GET/HEAD/PUT/MKCOL/DELETE" \
  --body "Sub-project 5, batch B: DAV router mounted at \`/remote.php/dav\` AND \`/dav\`. UserPath URL extractor + auth check. OPTIONS advertises \`DAV: 1, 2, 3\`. GET/HEAD with single Range support (206 + Content-Range; 416 on invalid). PUT with If-Match / If-None-Match: * conditional checks. MKCOL + DELETE happy paths. Cross-user access returns 403. Unauthenticated returns 401. MOVE/COPY land in batch C; PROPFIND/PROPPATCH/LOCK/UNLOCK in batches D/E/F."
```

**STOP.**

---

## Task 3: MOVE + COPY (Batch C)

**Files:**
- Create: `crates/crabcloud-http/src/routes/dav/moves.rs`
- Modify: `crates/crabcloud-http/src/routes/dav/mod.rs` (export `moves`)
- Modify: `crates/crabcloud-http/src/routes/dav/methods.rs` (dispatch MOVE/COPY)
- Modify: `crates/crabcloud-http/tests/dav_basic.rs` (or new test file)

### Step 1: Branch

```
git checkout -b webdav-batch-c origin/master
```

### Step 2: Create `crates/crabcloud-http/src/routes/dav/moves.rs`

```rust
//! MOVE + COPY handlers. Both honor `Destination:` and `Overwrite:` headers.
//! If `Overwrite: T` (default) and destination exists, the handler DELETEs
//! it first before calling `View::rename`/`copy` (which error on existing
//! destination in 4a's Storage trait).

use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_destination_files, parse_overwrite};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn move_(
    state: AppState,
    uid: &UserId,
    from: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let (to_user, to_path_raw) = parse_destination_files(headers)?;
    if to_user != uid.as_str() {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&to_path_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let to = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?;
    let overwrite = parse_overwrite(headers)?;
    let view = state.view_for(uid).await?;

    let dest_existed = view.stat(&to).await.is_ok();
    if dest_existed && !overwrite {
        return Err(DavError::PreconditionFailed);
    }
    if dest_existed {
        view.delete(&to).await?;
    }
    view.rename(from, &to).await?;
    Ok((
        if dest_existed {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        },
        "",
    )
        .into_response())
}

pub async fn copy(
    state: AppState,
    uid: &UserId,
    from: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let (to_user, to_path_raw) = parse_destination_files(headers)?;
    if to_user != uid.as_str() {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&to_path_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let to = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?;
    let overwrite = parse_overwrite(headers)?;
    let view = state.view_for(uid).await?;

    let dest_existed = view.stat(&to).await.is_ok();
    if dest_existed && !overwrite {
        return Err(DavError::PreconditionFailed);
    }
    if dest_existed {
        view.delete(&to).await?;
    }
    view.copy(from, &to).await?;
    Ok((
        if dest_existed {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        },
        "",
    )
        .into_response())
}
```

### Step 3: Wire MOVE + COPY into `methods.rs::dispatch_inner`

Replace the `MOVE/COPY` arm with:

```rust
        m if m.as_str() == "MOVE" => {
            crate::routes::dav::moves::move_(state, &uid, &user_path, &headers).await
        }
        m if m.as_str() == "COPY" => {
            crate::routes::dav::moves::copy(state, &uid, &user_path, &headers).await
        }
```

### Step 4: Add `pub mod moves;` to `crates/crabcloud-http/src/routes/dav/mod.rs`.

### Step 5: Tests in `crates/crabcloud-http/tests/dav_moves.rs`

Create a new test file with:

```rust
//! Integration tests for batch C: MOVE + COPY + Destination + Overwrite.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use secrecy::ExposeSecret;
use tempfile::tempdir;
use tower::ServiceExt;

use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use hex as _;
use thiserror as _;
use tracing as _;

// Reuse setup helpers from dav_basic.rs. For brevity, duplicate the helpers
// here OR (preferred) extract into a tests/support/dav.rs module. The plan
// duplicates them for simplicity; consider lifting in a follow-up.

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> crabcloud_core::AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    state
}

async fn login_cookie(state: &crabcloud_core::AppState) -> String {
    use crabcloud_users::AuthTokenType;
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "test",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    let v = crabcloud_http::session::encode_cookie(
        raw.expose(),
        state.config.secret.expose_secret().as_bytes(),
    );
    format!("{}={}", crabcloud_http::session::COOKIE_NAME, v)
}

async fn seed(app: &axum::Router, cookie: &str, path: &str, body: &[u8]) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/dav/files/alice/{path}"))
        .header("cookie", cookie)
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(resp.status().is_success(), "seed put failed: {}", resp.status());
}

#[tokio::test]
async fn move_renames_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &cookie, "from.txt", b"data").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/from.txt")
        .header("cookie", cookie.clone())
        .header("destination", "/dav/files/alice/to.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Source gone.
    let src = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/from.txt")
        .header("cookie", cookie.clone())
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(src).await.unwrap().status(), StatusCode::NOT_FOUND);

    // Dest present with the body.
    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/to.txt")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"data");
}

#[tokio::test]
async fn move_overwrite_f_blocks_when_dest_exists() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &cookie, "a.txt", b"A").await;
    seed(&app, &cookie, "b.txt", b"B").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("cookie", cookie)
        .header("destination", "/dav/files/alice/b.txt")
        .header("overwrite", "F")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn move_overwrite_t_replaces_dest_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &cookie, "a.txt", b"AAA").await;
    seed(&app, &cookie, "b.txt", b"old").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("cookie", cookie.clone())
        .header("destination", "/dav/files/alice/b.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/b.txt")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"AAA");
}

#[tokio::test]
async fn copy_duplicates_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &cookie, "src.txt", b"copy-me").await;

    let req = Request::builder()
        .method("COPY")
        .uri("/dav/files/alice/src.txt")
        .header("cookie", cookie.clone())
        .header("destination", "/dav/files/alice/dst.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    for path in ["src.txt", "dst.txt"] {
        let r = Request::builder()
            .method("GET")
            .uri(format!("/dav/files/alice/{path}"))
            .header("cookie", cookie.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let b = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&b[..], b"copy-me");
    }
}

#[tokio::test]
async fn move_to_other_user_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let cookie = login_cookie(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &cookie, "x.txt", b"X").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/x.txt")
        .header("cookie", cookie)
        .header("destination", "/dav/files/bob/x.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
```

### Step 6: Run + commit + push + open Batch C PR

```
cargo test -p crabcloud-http --tests
cargo xtask check-all
```

```
git add crates/crabcloud-http
git commit -m "feat(http,webdav): MOVE + COPY + Destination header + Overwrite header

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
git push -u origin webdav-batch-c
gh pr create --base master --head webdav-batch-c \
  --title "webdav: batch C — MOVE + COPY + Destination + Overwrite" \
  --body "Sub-project 5, batch C: MOVE/COPY handlers parse Destination + Overwrite headers. Overwrite=T (default) DELETEs the dest first before View::rename/copy (4a's Storage trait errors on existing dest). Cross-user destinations return 403."
```

**STOP.**

---

## Task 4: PROPFIND (Batch D)

**Files:**
- Create: `crates/crabcloud-http/src/routes/dav/xml.rs`
- Create: `crates/crabcloud-http/src/routes/dav/propfind.rs`
- Modify: `crates/crabcloud-http/src/routes/dav/methods.rs` (dispatch PROPFIND)
- Modify: `crates/crabcloud-http/src/routes/dav/mod.rs` (export propfind + xml)
- Create: `crates/crabcloud-http/tests/dav_propfind.rs`

### Step 1: Branch

```
git checkout -b webdav-batch-d origin/master
```

### Step 2: Create `crates/crabcloud-http/src/routes/dav/xml.rs`

Minimal Multistatus + propstat writer. The full PROPFIND response shape is in spec §7.2.

```rust
//! Shared XML helpers for DAV responses. Uses `quick_xml::writer::Writer`.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use std::io::Cursor;

/// Build a `<d:multistatus>` document from a builder closure that emits one
/// or more `<d:response>` blocks via the supplied callback.
pub fn multistatus<F>(build_responses: F) -> Vec<u8>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    let mut w = Writer::new(Cursor::new(Vec::new()));
    let _ = w.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("utf-8"),
        None,
    )));
    let mut start = BytesStart::new("d:multistatus");
    start.push_attribute(("xmlns:d", "DAV:"));
    start.push_attribute(("xmlns:oc", "http://owncloud.org/ns"));
    start.push_attribute(("xmlns:nc", "http://nextcloud.org/ns"));
    let _ = w.write_event(Event::Start(start));
    let _ = build_responses(&mut w);
    let _ = w.write_event(Event::End(BytesEnd::new("d:multistatus")));
    w.into_inner().into_inner()
}

/// Write a single `<d:response>` with one or more propstat blocks.
pub fn write_response<F>(
    w: &mut Writer<Cursor<Vec<u8>>>,
    href: &str,
    build_propstats: F,
) -> Result<(), quick_xml::Error>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    w.write_event(Event::Start(BytesStart::new("d:response")))?;
    w.write_event(Event::Start(BytesStart::new("d:href")))?;
    w.write_event(Event::Text(BytesText::new(href)))?;
    w.write_event(Event::End(BytesEnd::new("d:href")))?;
    build_propstats(w)?;
    w.write_event(Event::End(BytesEnd::new("d:response")))?;
    Ok(())
}

/// Write a `<d:propstat>` with a status line and inner props.
pub fn write_propstat<F>(
    w: &mut Writer<Cursor<Vec<u8>>>,
    status: &str,
    build_props: F,
) -> Result<(), quick_xml::Error>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    w.write_event(Event::Start(BytesStart::new("d:propstat")))?;
    w.write_event(Event::Start(BytesStart::new("d:prop")))?;
    build_props(w)?;
    w.write_event(Event::End(BytesEnd::new("d:prop")))?;
    w.write_event(Event::Start(BytesStart::new("d:status")))?;
    w.write_event(Event::Text(BytesText::new(status)))?;
    w.write_event(Event::End(BytesEnd::new("d:status")))?;
    w.write_event(Event::End(BytesEnd::new("d:propstat")))?;
    Ok(())
}

/// Helper: write a leaf element with text content.
pub fn write_leaf(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    text: &str,
) -> Result<(), quick_xml::Error> {
    w.write_event(Event::Start(BytesStart::new(name)))?;
    w.write_event(Event::Text(BytesText::new(text)))?;
    w.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

/// Helper: write an empty self-closing element.
pub fn write_empty(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
) -> Result<(), quick_xml::Error> {
    w.write_event(Event::Empty(BytesStart::new(name)))?;
    Ok(())
}
```

### Step 3: Create `crates/crabcloud-http/src/routes/dav/propfind.rs`

```rust
//! PROPFIND handler. Returns 207 Multi-Status with the 10-prop set per
//! spec §7.3. Depth 0 = resource only; Depth 1 = resource + children.
//! Depth: infinity rejected with 403 propfind-finite-depth.

use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_storage::{FileKind, Permissions};
use crabcloud_users::UserId;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use std::io::Cursor;

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_depth, Depth};
use crate::routes::dav::xml::{multistatus, write_empty, write_leaf, write_propstat, write_response};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const FAVORITE_PROP: &str = "{http://owncloud.org/ns}favorite";

/// Encode permission bitmap to letter string per Nextcloud convention.
fn permissions_str(p: Permissions, kind: FileKind) -> String {
    let mut s = String::new();
    if p.contains(Permissions::new(Permissions::SHARE)) {
        s.push('R');
    }
    if p.contains(Permissions::new(Permissions::DELETE)) {
        s.push('D');
    }
    if p.contains(Permissions::new(Permissions::UPDATE)) {
        s.push('N');
        s.push('V');
        s.push('W');
    }
    if p.contains(Permissions::new(Permissions::CREATE)) {
        s.push('C');
        if matches!(kind, FileKind::Directory) {
            s.push('K');
        }
    }
    s
}

/// Build the `oc:id` string: format!("{:020}{}", fileid, instanceid).
fn oc_id(fileid: i64, instanceid: &str) -> String {
    format!("{:020}{}", fileid, instanceid)
}

pub async fn handle(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let depth = parse_depth(headers, Depth::One)?;
    if matches!(depth, Depth::Infinity) {
        return Err(DavError::PropfindFiniteDepth);
    }

    let view = state.view_for(uid).await?;
    let meta = view.stat(user_path).await?;

    // Build the list of (user_path, metadata, fileid, favorite) tuples.
    let mut entries: Vec<(UserPath, _, i64, Option<String>)> = Vec::new();

    // The resource itself.
    let self_row = state
        .filecache
        .lookup(state.view_for(uid).await?.mounts()[0].storage.id(), &resolve_storage_path(user_path)?)
        .await
        .map_err(DavError::from)?
        .ok_or(DavError::NotFound)?;
    entries.push((user_path.clone(), meta.clone(), self_row.fileid, None));

    // Depth 1 → enumerate children.
    if matches!(depth, Depth::One) && matches!(meta.kind, FileKind::Directory) {
        let children = view.list(user_path).await?;
        let storage_id = view.mounts()[0].storage.id().to_string();
        for entry in children {
            let child_user_path = if user_path.is_root() {
                UserPath::new(format!("/{}", entry.name))?
            } else {
                user_path.join(&entry.name)?
            };
            let child_storage_path = resolve_storage_path(&child_user_path)?;
            let row = state
                .filecache
                .lookup(&storage_id, &child_storage_path)
                .await
                .map_err(DavError::from)?;
            let fileid = row.map(|r| r.fileid).unwrap_or(0);
            entries.push((child_user_path, entry.metadata, fileid, None));
        }
    }

    // Batched favorite lookup.
    let storage_paths: Vec<String> = entries
        .iter()
        .map(|(p, _, _, _)| resolve_storage_path(p).map(|s| s.as_str().to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let favorites = state
        .filecache
        .get_property_many(uid, &storage_paths, FAVORITE_PROP)
        .await
        .unwrap_or_default();
    // NOTE: the plan calls `FileCache::get_property_many` here. That method
    // belongs to `PropertyStore`; expose a thin pass-through on `FileCache`
    // OR call `state.properties().get_many(...)` if the AppState surfaces it.
    // For this plan we assume Batch A's PropertyStore is accessed via an
    // `AppState::properties()` accessor — add that in Batch A or Batch E if
    // missing. (See plan-bug note in §plan-bugs at end of plan.)

    let favorite_map: std::collections::HashMap<String, Option<String>> =
        favorites.into_iter().collect();

    let instanceid = state.config.instanceid.clone();
    let prefix = "/remote.php/dav/files";

    let body = multistatus(|w| {
        for (path, m, fileid, _) in &entries {
            let href = format!(
                "{}/{}{}",
                prefix,
                uid.as_str(),
                if path.is_root() {
                    String::new()
                } else {
                    path.as_str().to_string()
                }
            );
            let sp = resolve_storage_path(path).map_err(|_| quick_xml::Error::TextNotFound)?;
            let favorite = favorite_map
                .get(sp.as_str())
                .and_then(|v| v.as_deref())
                .unwrap_or("0");
            write_response(w, &href, |w| {
                write_propstat(w, "HTTP/1.1 200 OK", |w| {
                    if matches!(m.kind, FileKind::File) {
                        write_leaf(w, "d:getcontentlength", &m.size.to_string())?;
                        write_leaf(w, "d:getcontenttype", m.mimetype.as_str())?;
                    }
                    write_leaf(w, "d:getetag", &format!("\"{}\"", m.etag.as_str()))?;
                    write_leaf(w, "d:getlastmodified", &httpdate::fmt_http_date(m.mtime))?;
                    // resourcetype: empty for files; <d:collection/> for dirs.
                    w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
                    if matches!(m.kind, FileKind::Directory) {
                        write_empty(w, "d:collection")?;
                    }
                    w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
                    write_leaf(w, "d:displayname", path.basename())?;
                    write_leaf(w, "oc:id", &oc_id(*fileid, &instanceid))?;
                    write_leaf(w, "oc:permissions", &permissions_str(m.permissions, m.kind))?;
                    write_leaf(w, "oc:size", &m.size.to_string())?;
                    write_leaf(w, "oc:favorite", favorite)?;
                    Ok(())
                })
            })?;
        }
        Ok(())
    });

    Ok((
        StatusCode::from_u16(207).unwrap(),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        )],
        Body::from(body),
    )
        .into_response())
}

/// Map a `UserPath` to the storage-relative `StoragePath` (strips leading `/`).
/// For the home mount only (4c's HomeMountResolver).
fn resolve_storage_path(p: &UserPath) -> DavResult<crabcloud_storage::StoragePath> {
    let trimmed = p.as_str().trim_start_matches('/');
    crabcloud_storage::StoragePath::new(trimmed)
        .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))
}
```

### Step 4: Add `state.filecache.get_property_many` pass-through

In `crates/crabcloud-filecache/src/lib.rs`, add a thin pass-through on `FileCache`:

```rust
impl FileCache {
    // ... existing methods ...

    /// Pass-through to PropertyStore::get_many for one named property
    /// across many paths.
    pub async fn get_property_many(
        &self,
        userid: &crabcloud_users::UserId,
        propertypaths: &[String],
        propertyname: &str,
    ) -> FileCacheResult<Vec<(String, Option<String>)>> {
        // The PropertyStore borrows the same pool. Construct on the fly;
        // it's cheap (just a Pool clone).
        let ps = crate::properties::PropertyStore::new(self.pool().clone());
        ps.get_many(userid, propertypaths, propertyname).await
    }
}
```

Note: `FileCache::pool()` must be `pub(crate)` or expose another way to obtain a `DbPool`. If `pool()` is already private, add a new public accessor OR have `FileCache::new` store an `Arc<DbPool>` for cheap clone. The plan assumes `pool()` returns `&DbPool` and `DbPool: Clone`. Verify and adjust.

### Step 5: Wire PROPFIND into `methods.rs::dispatch_inner`

Replace the PROPFIND arm with:

```rust
        m if m.as_str() == "PROPFIND" => {
            crate::routes::dav::propfind::handle(state, &uid, &user_path, &headers).await
        }
```

### Step 6: Tests

Create `crates/crabcloud-http/tests/dav_propfind.rs` covering:

```rust
//! Integration tests for batch D: PROPFIND.
// Same setup/cookie helpers as dav_basic; consider lifting to a support module
// after batch G if duplication grows.

use axum::body::Body;
use axum::http::{Request, StatusCode};
// (copy make_state_with_user + login_cookie + seed helpers from dav_basic.rs)

#[tokio::test]
async fn propfind_depth_0_returns_resource() {
    // 1. Seed a file.
    // 2. PROPFIND /dav/files/alice/file.txt with Depth: 0.
    // 3. Parse body; assert 207 + one <d:response> + the 10 props.
    // (Body is XML; use quick_xml::reader to walk it, OR a substring assertion
    // for each expected prop name. Substring is fine for SP5 tests.)
}

#[tokio::test]
async fn propfind_depth_1_returns_children() {
    // 1. Seed /a, /b, /c via PUT.
    // 2. PROPFIND /dav/files/alice with Depth: 1.
    // 3. Assert 207 + 4 <d:response> blocks (the dir + 3 children).
}

#[tokio::test]
async fn propfind_depth_infinity_returns_403() {
    // 1. PROPFIND with Depth: infinity.
    // 2. Assert 403 + body contains <d:propfind-finite-depth/>.
}

#[tokio::test]
async fn propfind_404_props_appear_for_unknown() {
    // (Optional — SP5's handler returns the 10-prop set regardless of what
    // the client requested. A future hardening parses the request body
    // and segregates "200 OK" vs "404 Not Found" propstats. Defer.)
}
```

The tests use substring assertions on the XML body — quick + robust enough for SP5.

### Step 7: Run + commit + push + open Batch D PR

(Same flow as previous batches.)

---

## Task 5: PROPPATCH (Batch E)

**Files:**
- Create: `crates/crabcloud-http/src/routes/dav/proppatch.rs`
- Modify: `crates/crabcloud-http/src/routes/dav/methods.rs` (dispatch + invoke rename_path on MOVE/copy_path on COPY)
- Modify: `crates/crabcloud-http/src/routes/dav/moves.rs` (call PropertyStore::rename_path / copy_path)
- Create: `crates/crabcloud-http/tests/dav_proppatch.rs`

### Step 1: Branch

```
git checkout -b webdav-batch-e origin/master
```

### Step 2: Create `crates/crabcloud-http/src/routes/dav/proppatch.rs`

Handler steps per spec §8.2:

```rust
//! PROPPATCH handler. Parses set/remove ops via quick_xml::reader;
//! rejects protected props; upserts/deletes via PropertyStore.

use crabcloud_core::AppState;
use crabcloud_filecache::PropertyStore;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::xml::{multistatus, write_leaf, write_propstat, write_response};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const PROTECTED_PROPS: &[&str] = &[
    "{DAV:}getetag",
    "{DAV:}getcontentlength",
    "{DAV:}getlastmodified",
    "{DAV:}getcontenttype",
    "{DAV:}resourcetype",
    "{DAV:}displayname",
    "{http://owncloud.org/ns}id",
    "{http://owncloud.org/ns}permissions",
    "{http://owncloud.org/ns}size",
];

#[derive(Debug)]
enum PropOp {
    Set { name: String, value: Option<String> },
    Remove { name: String },
}

fn parse_body(body: &[u8]) -> DavResult<Vec<PropOp>> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut ops = Vec::new();
    let mut mode: Option<&'static str> = None; // "set" or "remove"
    let mut current_name: Option<String> = None;
    let mut current_value: Option<String> = None;
    let mut current_ns_prefix: std::collections::HashMap<String, String> = Default::default();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name_bytes = e.name();
                let name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| DavError::BadRequest("non-utf8 prop name".into()))?
                    .to_string();
                // Capture xmlns:* attributes.
                for attr in e.attributes().flatten() {
                    let k = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|_| DavError::BadRequest("non-utf8 attr".into()))?
                        .to_string();
                    let v = std::str::from_utf8(&attr.value)
                        .map_err(|_| DavError::BadRequest("non-utf8 attr value".into()))?
                        .to_string();
                    if let Some(prefix) = k.strip_prefix("xmlns:") {
                        current_ns_prefix.insert(prefix.to_string(), v);
                    } else if k == "xmlns" {
                        current_ns_prefix.insert(String::new(), v);
                    }
                }
                match name.as_str() {
                    "d:set" | "set" => mode = Some("set"),
                    "d:remove" | "remove" => mode = Some("remove"),
                    other if other == "d:prop" || other == "prop" => {}
                    other => {
                        if mode.is_some() {
                            // This is the prop name.
                            current_name = Some(name_to_clark(other, &current_ns_prefix));
                            current_value = Some(String::new());
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(v) = current_value.as_mut() {
                    v.push_str(t.unescape().unwrap_or_default().as_ref());
                }
            }
            Ok(Event::End(e)) => {
                let name_bytes = e.name();
                let name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| DavError::BadRequest("non-utf8 prop name".into()))?
                    .to_string();
                match name.as_str() {
                    "d:set" | "d:remove" | "set" | "remove" => mode = None,
                    "d:prop" | "prop" => {}
                    _other => {
                        if let Some(name) = current_name.take() {
                            match mode {
                                Some("set") => ops.push(PropOp::Set {
                                    name,
                                    value: current_value.take(),
                                }),
                                Some("remove") => ops.push(PropOp::Remove { name }),
                                _ => {}
                            }
                            current_value = None;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(DavError::BadRequest(format!("xml parse: {e}"))),
            _ => {}
        }
    }
    Ok(ops)
}

/// Convert a prefixed element name (`oc:favorite`) to Clark notation
/// (`{http://owncloud.org/ns}favorite`) using the in-scope prefix map.
fn name_to_clark(name: &str, prefixes: &std::collections::HashMap<String, String>) -> String {
    if let Some((prefix, local)) = name.split_once(':') {
        if let Some(ns) = prefixes.get(prefix) {
            return format!("{{{}}}{}", ns, local);
        }
    }
    // Default-namespace case.
    if let Some(ns) = prefixes.get("") {
        return format!("{{{}}}{}", ns, name);
    }
    name.to_string()
}

pub async fn handle(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    body: Body,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    let _meta = view.stat(user_path).await?;

    let body_bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| DavError::BadRequest(format!("body read: {e}")))?;
    let ops = parse_body(&body_bytes)?;
    let store = PropertyStore::new(state.filecache.pool().clone());
    let property_path = user_path.as_str().trim_start_matches('/').to_string();

    let mut results: Vec<(String, &'static str)> = Vec::new();
    for op in ops {
        match op {
            PropOp::Set { name, value } => {
                if PROTECTED_PROPS.iter().any(|p| **p == name) {
                    results.push((name, "HTTP/1.1 403 Forbidden"));
                    continue;
                }
                store
                    .upsert(uid, &property_path, &name, value.as_deref())
                    .await?;
                results.push((name, "HTTP/1.1 200 OK"));
            }
            PropOp::Remove { name } => {
                if PROTECTED_PROPS.iter().any(|p| **p == name) {
                    results.push((name, "HTTP/1.1 403 Forbidden"));
                    continue;
                }
                store.delete(uid, &property_path, &name).await?;
                results.push((name, "HTTP/1.1 200 OK"));
            }
        }
    }

    let prefix = "/remote.php/dav/files";
    let href = format!("{}/{}{}", prefix, uid.as_str(), user_path.as_str());
    let body = multistatus(|w| {
        write_response(w, &href, |w| {
            for (name, status) in &results {
                write_propstat(w, status, |w| write_leaf(w, name, ""))?;
            }
            Ok(())
        })
    });

    Ok((
        StatusCode::from_u16(207).unwrap(),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        )],
        Body::from(body),
    )
        .into_response())
}
```

### Step 3: Wire PROPPATCH dispatch + MOVE/COPY path rewrite

In `methods.rs::dispatch_inner`, replace the PROPPATCH arm:

```rust
        m if m.as_str() == "PROPPATCH" => {
            crate::routes::dav::proppatch::handle(state, &uid, &user_path, body).await
        }
```

In `moves.rs`, after the `view.rename(from, &to).await?` line, add:

```rust
    let store = crabcloud_filecache::PropertyStore::new(state.filecache.pool().clone());
    let from_sp = from.as_str().trim_start_matches('/');
    let to_sp = to.as_str().trim_start_matches('/');
    store.rename_path(uid, from_sp, to_sp).await?;
```

Same for `copy`:

```rust
    let store = crabcloud_filecache::PropertyStore::new(state.filecache.pool().clone());
    let from_sp = from.as_str().trim_start_matches('/');
    let to_sp = to.as_str().trim_start_matches('/');
    store.copy_path(uid, from_sp, to_sp).await?;
```

### Step 4: Tests

Create `crates/crabcloud-http/tests/dav_proppatch.rs` with:
- `proppatch_sets_oc_favorite_and_propfind_reads_it_back`
- `proppatch_protected_prop_returns_403_in_propstat`
- `proppatch_paths_follow_move`

### Step 5: Run + commit + push + open Batch E PR

(Same flow.)

---

## Task 6: LOCK + UNLOCK (Batch F)

**Files:**
- Create: `crates/crabcloud-http/src/routes/dav/lock.rs`
- Modify: `crates/crabcloud-http/src/routes/dav/headers.rs` (add If-header + Lock-Token + Timeout parsers)
- Modify: `crates/crabcloud-http/src/routes/dav/methods.rs` (lock_check before mutations)
- Modify: `crates/crabcloud-http/src/routes/dav/moves.rs` (same)
- Modify: `crates/crabcloud-http/src/routes/dav/proppatch.rs` (same)
- Create: `crates/crabcloud-http/tests/dav_lock.rs`

### Step 1: Branch

```
git checkout -b webdav-batch-f origin/master
```

### Step 2: Headers — add If, Lock-Token, Timeout parsers

In `headers.rs`, append:

```rust
/// Parse the `If:` header. SP5 supports only the `(<urn:uuid:...>)` form
/// (Nextcloud's clients use this). Returns the list of submitted tokens.
pub fn parse_if_tokens(headers: &HeaderMap) -> Vec<String> {
    let raw = match headers.get("if").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    // Strip outer parens; split by whitespace.
    let mut tokens = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut t = String::new();
            for cc in chars.by_ref() {
                if cc == '>' {
                    break;
                }
                t.push(cc);
            }
            if !t.is_empty() {
                tokens.push(t);
            }
        }
    }
    tokens
}

/// Parse the `Lock-Token:` header (single value). Strips `<` and `>`.
pub fn parse_lock_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("lock-token").and_then(|v| v.to_str().ok())?;
    let trimmed = raw.trim();
    let inner = trimmed.trim_start_matches('<').trim_end_matches('>');
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// Parse `Timeout: Second-<N>` or `Timeout: Infinite`. Returns the
/// clamped TTL in seconds (cap at 1800; default 1800).
pub fn parse_timeout(headers: &HeaderMap) -> i64 {
    const DEFAULT_TTL: i64 = 1800;
    const MAX_TTL: i64 = 1800;
    let raw = match headers.get("timeout").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return DEFAULT_TTL,
    };
    for part in raw.split(',') {
        let p = part.trim();
        if p.eq_ignore_ascii_case("infinite") {
            return MAX_TTL;
        }
        if let Some(n) = p.strip_prefix("Second-").or_else(|| p.strip_prefix("second-")) {
            if let Ok(v) = n.parse::<i64>() {
                return v.min(MAX_TTL);
            }
        }
    }
    DEFAULT_TTL
}
```

### Step 3: Create `crates/crabcloud-http/src/routes/dav/lock.rs`

```rust
//! LOCK + UNLOCK handlers + lock_check helper for use by mutation methods.

use crabcloud_core::AppState;
use crabcloud_filecache::LockStore;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_depth, parse_if_tokens, parse_lock_token, parse_timeout, Depth};
use crate::routes::dav::xml::{multistatus, write_leaf, write_propstat, write_response};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

fn lock_key(uid: &UserId, user_path: &UserPath) -> String {
    let p = user_path.as_str().trim_start_matches('/');
    if p.is_empty() {
        format!("files/{}", uid.as_str())
    } else {
        format!("files/{}/{}", uid.as_str(), p)
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Lock-aware mutation check. Errors with `DavError::Locked` if the resource
/// itself OR any ancestor with `depth = "infinity"` is locked AND none of the
/// submitted tokens match.
pub async fn lock_check(
    locks: &LockStore,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<()> {
    let submitted = parse_if_tokens(headers);
    // Self check.
    let self_key = lock_key(uid, user_path);
    if let Some(lock) = locks.current(&self_key).await? {
        if !submitted.iter().any(|t| t == &lock.token) {
            return Err(DavError::Locked);
        }
    }
    // Ancestor check (only depth=infinity blocks).
    let mut parent = user_path.parent();
    while let Some(p) = parent {
        let pkey = lock_key(uid, &p);
        if let Some(lock) = locks.current(&pkey).await? {
            if lock.depth == "infinity" && !submitted.iter().any(|t| t == &lock.token) {
                return Err(DavError::Locked);
            }
        }
        if p.is_root() {
            break;
        }
        parent = p.parent();
    }
    Ok(())
}

pub async fn acquire(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.stat(user_path).await?;
    let key = lock_key(uid, user_path);
    let locks = LockStore::new(state.filecache.pool().clone());

    // If already locked AND no matching token → 423.
    let submitted = parse_if_tokens(headers);
    if let Some(lock) = locks.current(&key).await? {
        if !submitted.iter().any(|t| t == &lock.token) {
            return Err(DavError::Locked);
        }
    }

    let depth = parse_depth(headers, Depth::Zero)?;
    let depth_str = match depth {
        Depth::Zero => "0",
        Depth::One => "0", // LOCK Depth: 1 is unusual; collapse to 0.
        Depth::Infinity => "infinity",
    };
    let ttl_secs = parse_timeout(headers);
    let ttl = now_unix() + ttl_secs;
    let token = format!("urn:uuid:{}", uuid::Uuid::new_v4());

    // Owner XML (best-effort: pass body through; not parsed).
    let owner = String::from_utf8(
        axum::body::to_bytes(body, 64 * 1024)
            .await
            .map_err(|e| DavError::BadRequest(format!("lock body: {e}")))?
            .to_vec(),
    )
    .ok();

    locks
        .acquire(&key, &token, "exclusive", depth_str, owner.as_deref(), ttl)
        .await?;

    // Compose response body (lockdiscovery).
    let prefix = "/remote.php/dav/files";
    let href = format!("{}/{}{}", prefix, uid.as_str(), user_path.as_str());
    let body = multistatus(|w| {
        write_response(w, &href, |w| {
            write_propstat(w, "HTTP/1.1 200 OK", |w| {
                write_leaf(w, "d:locktype", "")?;
                write_leaf(w, "d:lockscope", "")?;
                write_leaf(w, "d:depth", depth_str)?;
                write_leaf(w, "d:timeout", &format!("Second-{}", ttl_secs))?;
                write_leaf(w, "d:locktoken", &token)?;
                write_leaf(w, "d:lockroot", &href)?;
                Ok(())
            })
        })
    });

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/xml; charset=utf-8"),
            ),
            (
                header::HeaderName::from_static("lock-token"),
                HeaderValue::from_str(&format!("<{}>", token)).unwrap(),
            ),
        ],
        Body::from(body),
    )
        .into_response())
}

pub async fn release(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let token = parse_lock_token(headers).ok_or(DavError::Conflict)?;
    let key = lock_key(uid, user_path);
    let locks = LockStore::new(state.filecache.pool().clone());
    if locks.release(&key, &token).await? {
        Ok((StatusCode::NO_CONTENT, "").into_response())
    } else {
        Err(DavError::Conflict)
    }
}
```

### Step 4: Wire lock_check + dispatch

Modify `methods.rs::dispatch_inner`. Replace the LOCK/UNLOCK arm:

```rust
        m if m.as_str() == "LOCK" => {
            crate::routes::dav::lock::acquire(state, &uid, &user_path, &headers, body).await
        }
        m if m.as_str() == "UNLOCK" => {
            crate::routes::dav::lock::release(state, &uid, &user_path, &headers).await
        }
```

Add a `lock_check` call at the top of `put`, `mkcol`, `delete` (in methods.rs) and at the top of `move_`, `copy` (in moves.rs) and at the top of `proppatch::handle`. Example for `put`:

```rust
async fn put(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    let locks = crabcloud_filecache::LockStore::new(state.filecache.pool().clone());
    crate::routes::dav::lock::lock_check(&locks, uid, user_path, headers).await?;
    // ... rest of existing logic ...
}
```

### Step 5: Tests in `crates/crabcloud-http/tests/dav_lock.rs`

Cover:
- `lock_acquire_returns_token`
- `lock_on_locked_resource_returns_423`
- `unlock_with_correct_token_releases`
- `unlock_with_wrong_token_returns_409`
- `put_on_locked_without_if_returns_423`
- `put_on_locked_with_if_succeeds`
- `lock_infinity_depth_locks_children`
- `expired_lock_can_be_reacquired`

### Step 6: Run + commit + push + open Batch F PR

(Same flow.)

---

## Task 7: Chunked uploads + Playwright e2e + acceptance docs (Batch G)

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs` — add `upload_id_map: Arc<DashMap<String, String>>` field on AppState; construct in builder
- Create: `crates/crabcloud-http/src/routes/dav/uploads.rs`
- Modify: `crates/crabcloud-http/src/routes/dav/mod.rs` (export uploads + new uploads_router)
- Modify: `crates/crabcloud-http/src/router.rs` (mount uploads_router at both prefixes)
- Create: `crates/crabcloud-http/tests/dav_uploads.rs`
- Create: `e2e/tests/webdav.spec.ts`
- Create: `docs/superpowers/plans/2026-05-12-webdav-and-files-api-implementation.changelog.md`
- Modify: `README.md`

### Step 1: Branch

```
git checkout -b webdav-batch-g origin/master
```

### Step 2: Add `upload_id_map` to AppState

In `crates/crabcloud-core/src/state.rs`, add a field:

```rust
pub upload_id_map: Arc<dashmap::DashMap<String, String>>,
```

(With a doc-comment explaining the in-process map maps the client's URL-segment `upload_id` → server-encoded `upload_id` for the duration of an upload.)

Construct in `AppStateBuilder::build`:

```rust
let upload_id_map = Arc::new(dashmap::DashMap::new());
```

Include in the `AppState { ... }` literal.

Make sure `dashmap` is in `crabcloud-core`'s deps.

### Step 3: Create `crates/crabcloud-http/src/routes/dav/uploads.rs`

```rust
//! Chunked upload route handlers per spec §11.

use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::parse_destination_files;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;

pub async fn mkcol_begin(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid = UserId::new(&url_user)
        .map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let (dest_user, dest_path) = parse_destination_files(&headers)?;
    if dest_user != url_user {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&dest_path)
        .map_err(|e| DavError::BadRequest(format!("invalid encoding: {e}")))?;
    let destination = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid dest path: {e}")))?;

    let uploads = state.uploads_for(&uid).await?;
    let handle = uploads.begin(&destination).await?;
    state.upload_id_map.insert(upload_id, handle.upload_id);
    Ok((StatusCode::CREATED, "").into_response())
}

pub async fn put_chunk(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((url_user, upload_id, part_n)): Path<(String, String, u32)>,
    body: Body,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid = UserId::new(&url_user)
        .map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = state
        .upload_id_map
        .get(&upload_id)
        .ok_or(DavError::NotFound)?
        .clone();

    let uploads = state.uploads_for(&uid).await?;
    let stream = body.into_data_stream();
    let reader = tokio_util::io::StreamReader::new(
        stream.map(|r| r.map_err(|e| std::io::Error::other(e.to_string()))),
    );
    let pinned: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = Box::pin(reader);
    let tag = uploads.put_part(&server_id, part_n, pinned).await?;

    Ok((
        StatusCode::CREATED,
        [(header::ETAG, HeaderValue::from_str(&tag.etag).unwrap())],
        "",
    )
        .into_response())
}

pub async fn move_commit(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid = UserId::new(&url_user)
        .map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = state
        .upload_id_map
        .get(&upload_id)
        .ok_or(DavError::NotFound)?
        .clone();

    let (dest_user, dest_path) = parse_destination_files(&headers)?;
    if dest_user != url_user {
        return Err(DavError::Forbidden);
    }
    let decoded = urlencoding::decode(&dest_path)
        .map_err(|e| DavError::BadRequest(format!("invalid encoding: {e}")))?;
    let destination = UserPath::new(format!("/{}", decoded))
        .map_err(|e| DavError::BadRequest(format!("invalid dest path: {e}")))?;

    // Parse X-Crabcloud-Part-Tags JSON.
    let tags_raw = headers
        .get("x-crabcloud-part-tags")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| DavError::BadRequest("missing X-Crabcloud-Part-Tags".into()))?;
    let tags: Vec<crabcloud_storage::PartTag> = serde_json::from_str(tags_raw)
        .map_err(|e| DavError::BadRequest(format!("invalid part tags json: {e}")))?;

    let uploads = state.uploads_for(&uid).await?;
    let meta = uploads.commit(&server_id, &destination, tags).await?;
    state.upload_id_map.remove(&upload_id);

    Ok((
        StatusCode::CREATED,
        [(
            header::ETAG,
            HeaderValue::from_str(&format!("\"{}\"", meta.etag.as_str())).unwrap(),
        )],
        "",
    )
        .into_response())
}

pub async fn delete_abort(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((url_user, upload_id)): Path<(String, String)>,
) -> DavResult<Response> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid = UserId::new(&url_user)
        .map_err(|e| DavError::BadRequest(format!("invalid uid: {e}")))?;
    let server_id = match state.upload_id_map.remove(&upload_id) {
        Some((_, v)) => v,
        None => return Ok((StatusCode::NO_CONTENT, "").into_response()),
    };
    let uploads = state.uploads_for(&uid).await?;
    uploads.abort(&server_id).await?;
    Ok((StatusCode::NO_CONTENT, "").into_response())
}
```

Note: `crabcloud_storage::PartTag` needs `serde::{Serialize, Deserialize}` derives. Add to `crabcloud-storage::meta.rs` if missing.

### Step 4: Register `PartTag` serde derives in `crabcloud-storage`

In `crates/crabcloud-storage/src/meta.rs`, add `#[derive(Serialize, Deserialize)]` to the `PartTag` struct + add `serde.workspace = true` to `crabcloud-storage/Cargo.toml` `[dependencies]` if not already there.

### Step 5: Add uploads sub-router to `dav_router`

Modify `dav/mod.rs`:

```rust
pub mod uploads;

// In dav_router():
fn uploads_branch() -> Router<AppState> {
    use axum::routing::any;
    Router::new()
        // MKCOL + DELETE on /uploads/{user}/{upload_id}
        .route("/uploads/{user}/{upload_id}", any(dispatch_uploads_root))
        // PUT + MOVE on /uploads/{user}/{upload_id}/{*part}
        .route(
            "/uploads/{user}/{upload_id}/{*part}",
            any(dispatch_uploads_part),
        )
}

async fn dispatch_uploads_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((user, upload_id)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, DavError> {
    match method.as_str() {
        "MKCOL" => uploads::mkcol_begin(State(state), authed, headers, Path((user, upload_id))).await,
        "DELETE" => uploads::delete_abort(State(state), authed, Path((user, upload_id))).await,
        _ => Ok((StatusCode::METHOD_NOT_ALLOWED, "").into_response()),
    }
}

async fn dispatch_uploads_part(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((user, upload_id, part)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, DavError> {
    match method.as_str() {
        "PUT" => {
            let part_n: u32 = part.parse().map_err(|_| DavError::BadRequest(format!("invalid part: {part}")))?;
            uploads::put_chunk(State(state), authed, Path((user, upload_id, part_n)), body).await
        }
        "MOVE" => {
            // The trailing path is `.file` per Nextcloud convention.
            if part != ".file" {
                return Err(DavError::BadRequest(format!("expected .file, got {part}")));
            }
            uploads::move_commit(State(state), authed, headers, Path((user, upload_id))).await
        }
        _ => Ok((StatusCode::METHOD_NOT_ALLOWED, "").into_response()),
    }
}
```

Combine into `dav_router`:

```rust
pub fn dav_router() -> Router<AppState> {
    Router::new()
        // files routes (existing)
        .merge(uploads_branch())
}
```

### Step 6: Tests in `crates/crabcloud-http/tests/dav_uploads.rs`

Cover:
- `chunked_upload_begin_put_commit_flow`
- `chunked_upload_unknown_id_returns_404_on_put`
- `chunked_upload_abort_returns_204`

### Step 7: Playwright e2e

Create `e2e/tests/webdav.spec.ts`:

```ts
import { test, expect } from "@playwright/test";

test.describe("WebDAV files API", () => {
    test.afterAll(async ({ request }) => {
        try {
            const login = await request.post("/index.php/login", {
                data: { username: "admin", password: "hunter2" },
                headers: { "content-type": "application/json" },
                maxRedirects: 0,
            });
            const setCookie = login.headers()["set-cookie"];
            if (!setCookie) return;
            const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie);
            if (!m) return;
            const cookie = `oc_sessionPassphrase=${m[1]}`;
            await request.delete("/dav/files/admin/webdav-test.txt", {
                headers: { cookie },
            });
        } catch {}
    });

    test("OPTIONS advertises DAV class", async ({ request }) => {
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        expect(login.status()).toBe(200);
        const setCookie = login.headers()["set-cookie"];
        expect(setCookie).toBeTruthy();
        const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie!);
        expect(m).not.toBeNull();
        const cookie = `oc_sessionPassphrase=${m![1]}`;

        const r = await request.fetch("/dav/files/admin", {
            method: "OPTIONS",
            headers: { cookie },
        });
        expect(r.status()).toBe(200);
        expect(r.headers()["dav"]).toContain("1");
    });

    test("PUT then GET then DELETE round-trip", async ({ request }) => {
        // Login + cookie capture as above; helper inlined for clarity.
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        const setCookie = login.headers()["set-cookie"]!;
        const cookie = `oc_sessionPassphrase=${/oc_sessionPassphrase=([^;\n]+)/.exec(setCookie)![1]}`;

        const put = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "PUT",
            headers: { cookie },
            data: "hello world",
        });
        expect(put.status()).toBe(201);

        const get = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "GET",
            headers: { cookie },
        });
        expect(get.status()).toBe(200);
        expect(await get.text()).toBe("hello world");

        const del = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "DELETE",
            headers: { cookie },
        });
        expect(del.status()).toBe(204);
    });

    test("PROPFIND Depth:0 returns 207", async ({ request }) => {
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        const setCookie = login.headers()["set-cookie"]!;
        const cookie = `oc_sessionPassphrase=${/oc_sessionPassphrase=([^;\n]+)/.exec(setCookie)![1]}`;

        const r = await request.fetch("/dav/files/admin", {
            method: "PROPFIND",
            headers: { cookie, depth: "0" },
        });
        expect(r.status()).toBe(207);
        const body = await r.text();
        expect(body).toContain("<d:multistatus");
        expect(body).toContain("<d:getetag>");
        expect(body).toContain("<oc:permissions>");
    });
});
```

### Step 8: Changelog + README

Create the changelog with the 22-row acceptance table from spec §14. README gets a new bullet under the workspace-layout section noting `crabcloud-http` now hosts the WebDAV routes.

### Step 9: Run + commit + push + open Batch G PR

```
cargo test -p crabcloud-http --tests
cargo xtask check-all
```

Final batch — closes the sub-project.

---

## Plan-bugs called out for the implementers

1. **`FileCache::pool()` visibility:** the plan calls it from `propfind.rs`, `proppatch.rs`, `lock.rs`, `uploads.rs` to construct PropertyStore / LockStore on the fly. If `pool()` is currently private, Batch A needs to expose it (`pub fn pool(&self) -> DbPool` if `DbPool: Clone`, otherwise add `Arc<DbPool>` to `FileCache` and expose). Adjust in Batch A or expose in Batch D as needed.

2. **`PartTag` needs serde derives** (Batch G uses `serde_json::from_str`). Add in Batch G or earlier.

3. **`dashmap`** must be in `crabcloud-core`'s deps for the `upload_id_map` field. Add in Batch G.

4. **`tokio_util` and `futures`** are needed for `ReaderStream`/`StreamReader` body wrapping in `methods.rs` and `uploads.rs`. Add to `crabcloud-http/Cargo.toml` in Batch B.

5. **Cookie/login helpers** are duplicated across `dav_basic.rs`, `dav_moves.rs`, `dav_propfind.rs`, `dav_proppatch.rs`, `dav_lock.rs`, `dav_uploads.rs`. After Batch C, consider lifting into a `tests/support/dav.rs` module. Not blocking.

6. **`crabcloud_http::session::encode_cookie` + `COOKIE_NAME`** are referenced by tests. Verify they're `pub` from the session module; expose if private.

## Final acceptance

After all 7 PRs land:

1. `git pull --ff-only origin master`.
2. `cargo xtask check-all` green.
3. CI green on master (all 5 checks).
4. Update program memory: mark SP5 complete.

Open questions deferred — see spec §16.
