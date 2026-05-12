# File Cache + Async Scanner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `crabcloud-filecache` — a new workspace crate with the `FileCache` façade, `Scanner` background consumer, and an `oc_filecache`/`oc_storages`/`oc_mimetypes` schema mirroring upstream Nextcloud — plus `ChannelEventSink` in `crabcloud-storage`.

**Architecture:** New crate `crabcloud-filecache` owns DB-backed cache + scanner. `ChannelEventSink` (added to `crabcloud-storage`) wraps `tokio::sync::broadcast`; scanner subscribes + applies events to the cache; full-scan reconciles drift. Cache-miss populate serializes per-path via `DashMap<(String, StoragePath), Arc<Mutex<()>>>`. Ancestor size + ETag propagation runs in a single DB transaction per event (match upstream Nextcloud write-through behavior).

**Tech Stack:** Rust 1.95 + sqlx 0.8 (sqlite/mysql/postgres via `DbPool`) + tokio (broadcast + sync) + `dashmap` + `md-5` + `crabcloud-storage` + `crabcloud-db`.

**Parent spec:** `docs/superpowers/specs/2026-05-12-filecache-and-scanner-design.md` (merged at master).

**Important plan-bug note:** The spec named the new migration `0003_filecache`. **`0003_auth_tokens` is already on master.** The actual migration in this plan is **`0004_filecache`** (version `4`). All references to "0003_filecache" in the spec should be read as "0004_filecache".

**Branch protection:** master is rules-gated (PR required); auto-merge enabled at repo level. After `gh pr create`, queue with `gh pr merge --squash --delete-branch --auto`. The controller verifies + waits per the established admin-OCS / storage-4a pattern.

---

## Conventions

- **Commits:** Conventional Commits (`feat(filecache)`, `feat(storage)`, `test(filecache)`, `docs(filecache)`) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD:** test → fail → implement → pass → commit.
- **rustfmt:** `cargo fmt --all` before push.
- **`cargo xtask check-all` must pass before push** (this runs fmt/clippy/sqlite tests/multi-dialect tests/e2e).
- **`-D warnings` workspace-wide.** Every new dep gets a real use site.
- **One PR per batch.** Stop at "PR opened, awaiting merge."

---

## File Structure

```
crates/crabcloud-filecache/                          NEW CRATE
├── Cargo.toml
└── src/
    ├── lib.rs                                       FileCache facade + public types + module wires
    ├── error.rs                                     FileCacheError + FileCacheResult
    ├── schema.rs                                    FilecacheRow + sqlx FromRow impls
    ├── mimetypes.rs                                 intern_mimetype + type-half helper
    ├── storages.rs                                  intern_storage + last_checked helper
    ├── populate.rs                                  cache-miss populate path (DashMap lock)
    ├── propagate.rs                                 ancestor walk + size/etag bump in tx
    └── scanner/
        ├── mod.rs                                   Scanner struct + spawn/register
        ├── apply.rs                                 StorageEvent -> mutation dispatch
        ├── full_scan.rs                             BFS walk + populate top-down
        └── cli.rs                                   files:scan subcommand entrypoint

crates/crabcloud-filecache/tests/                    Integration tests
├── support/
│   └── mod.rs                                       CountingStorage, test fixtures
└── (one file per integration test scenario)

crates/crabcloud-storage/src/lib.rs                  MODIFIED + ChannelEventSink
crates/crabcloud-core/src/state.rs                   MODIFIED + storage_sink / filecache / scanner fields
crates/crabcloud-config/src/lib.rs                   MODIFIED + FilecacheConfig
crates/crabcloud-server/src/cli.rs                   MODIFIED + files:scan subcommand
migrations/core/0004_filecache/                      NEW
├── sqlite.sql
├── mysql.sql
└── postgres.sql
crates/crabcloud-db/src/core_migrations.rs           MODIFIED + Migration entry version=4
Cargo.toml                                            MODIFIED + dashmap, md-5 in workspace deps
README.md                                             MODIFIED + crabcloud-filecache bullet (Batch F)
```

---

## Batches

| Batch | Tasks | Theme |
|-------|-------|---|
| **A** | 1 | Crate skeleton + migration `0004_filecache` + error types + interning helpers + `FilecacheRow` |
| **B** | 2 | `FileCache::apply` for all 5 event variants + ancestor propagation + 6 integration tests |
| **C** | 3 | Cache-miss populate (`FileCache::stat`/`list`) + per-path lock + 4 integration tests |
| **D** | 4 | `ChannelEventSink` (in `crabcloud-storage`) + `Scanner` + full-scan + 3 integration tests |
| **E** | 5 | `AppStateBuilder` wiring + `[filecache]` config block + `files:scan` CLI subcommand |
| **F** | 6 | Acceptance docs (changelog + README + 4b-S3 prep notes) |

---

## Task 1: Crate skeleton + migration + types (Batch A)

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/crabcloud-filecache/Cargo.toml`
- Create: `crates/crabcloud-filecache/src/lib.rs`
- Create: `crates/crabcloud-filecache/src/error.rs`
- Create: `crates/crabcloud-filecache/src/schema.rs`
- Create: `crates/crabcloud-filecache/src/mimetypes.rs`
- Create: `crates/crabcloud-filecache/src/storages.rs`
- Create: `migrations/core/0004_filecache/sqlite.sql`
- Create: `migrations/core/0004_filecache/mysql.sql`
- Create: `migrations/core/0004_filecache/postgres.sql`
- Modify: `crates/crabcloud-db/src/core_migrations.rs` (add version=4 entry)

### Step 1: Branch + workspace dep additions

```
git checkout -b filecache-batch-a origin/master
```

Modify root `Cargo.toml`. Find `[workspace] members = [...]` and add `"crates/crabcloud-filecache",` alphabetically (between `-db` and `-http`):

```toml
[workspace]
members = [
    "crates/crabcloud-cache",
    "crates/crabcloud-config",
    "crates/crabcloud-core",
    "crates/crabcloud-db",
    "crates/crabcloud-filecache",
    "crates/crabcloud-http",
    "crates/crabcloud-i18n",
    "crates/crabcloud-ocs",
    "crates/crabcloud-server",
    "crates/crabcloud-storage",
    "crates/crabcloud-ui",
    "crates/crabcloud-users",
    "xtask",
]
```

Find `[workspace.dependencies]` and add (alphabetically):

```toml
dashmap = "6"
md-5 = "0.11"
```

### Step 2: Create `crates/crabcloud-filecache/Cargo.toml`

```toml
[package]
name = "crabcloud-filecache"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
crabcloud-cache.workspace = true
crabcloud-config.workspace = true
crabcloud-db.workspace = true
crabcloud-storage.workspace = true
dashmap.workspace = true
hex.workspace = true
md-5.workspace = true
sqlx = { workspace = true, default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlite", "mysql", "postgres", "chrono"] }
thiserror.workspace = true
tokio = { workspace = true, features = ["fs", "io-util", "sync", "macros"] }
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "fs", "io-util", "sync", "time"] }

[lints]
workspace = true
```

`crabcloud-cache`/`crabcloud-config`/`crabcloud-db`/`crabcloud-storage` are existing workspace crates with `path = ...` entries already set; the `workspace = true` syntax inherits from `[workspace.dependencies]` (if not present there, fall back to a direct `path = "../crabcloud-storage"`). Check what existing crates use as the pattern when in doubt.

### Step 3: Create the migration files

Create `migrations/core/0004_filecache/sqlite.sql`:

```sql
CREATE TABLE oc_storages (
    numeric_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    id            TEXT    NOT NULL UNIQUE,
    available     INTEGER NOT NULL DEFAULT 1,
    last_checked  INTEGER NULL
);

CREATE TABLE oc_mimetypes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    mimetype  TEXT    NOT NULL UNIQUE
);

CREATE TABLE oc_filecache (
    fileid         INTEGER PRIMARY KEY AUTOINCREMENT,
    storage        INTEGER NOT NULL,
    path           TEXT    NOT NULL,
    path_hash      TEXT    NOT NULL,
    parent         INTEGER NULL,
    name           TEXT    NOT NULL,
    mimetype       INTEGER NOT NULL,
    mimepart       INTEGER NOT NULL,
    size           INTEGER NOT NULL DEFAULT 0,
    mtime          INTEGER NOT NULL DEFAULT 0,
    storage_mtime  INTEGER NOT NULL DEFAULT 0,
    encrypted      INTEGER NOT NULL DEFAULT 0,
    etag           TEXT    NOT NULL,
    permissions    INTEGER NOT NULL DEFAULT 0,
    checksum       TEXT    NULL,
    FOREIGN KEY (storage)  REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    FOREIGN KEY (mimetype) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    FOREIGN KEY (mimepart) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    FOREIGN KEY (parent)   REFERENCES oc_filecache(fileid)     ON DELETE CASCADE
);

CREATE UNIQUE INDEX fs_storage_path  ON oc_filecache (storage, path_hash);
CREATE        INDEX fs_parent        ON oc_filecache (parent);
CREATE        INDEX fs_mimepart      ON oc_filecache (mimepart);
CREATE        INDEX fs_mimetype      ON oc_filecache (mimetype);
CREATE        INDEX fs_storage_size  ON oc_filecache (storage, size);
```

Create `migrations/core/0004_filecache/mysql.sql`:

```sql
CREATE TABLE oc_storages (
    numeric_id    INT          UNSIGNED NOT NULL AUTO_INCREMENT,
    id            VARCHAR(64)           NOT NULL,
    available     TINYINT      UNSIGNED NOT NULL DEFAULT 1,
    last_checked  INT          UNSIGNED NULL,
    PRIMARY KEY (numeric_id),
    UNIQUE KEY oc_storages_id_uniq (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_mimetypes (
    id        INT          UNSIGNED NOT NULL AUTO_INCREMENT,
    mimetype  VARCHAR(255)          NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY oc_mimetypes_mimetype_uniq (mimetype)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_filecache (
    fileid         BIGINT       UNSIGNED NOT NULL AUTO_INCREMENT,
    storage        INT          UNSIGNED NOT NULL,
    path           VARCHAR(4000)          NOT NULL,
    path_hash      CHAR(32)               NOT NULL,
    parent         BIGINT       UNSIGNED NULL,
    name           VARCHAR(250)           NOT NULL,
    mimetype       INT          UNSIGNED NOT NULL,
    mimepart       INT          UNSIGNED NOT NULL,
    size           BIGINT                 NOT NULL DEFAULT 0,
    mtime          INT          UNSIGNED NOT NULL DEFAULT 0,
    storage_mtime  INT          UNSIGNED NOT NULL DEFAULT 0,
    encrypted      TINYINT      UNSIGNED NOT NULL DEFAULT 0,
    etag           VARCHAR(40)            NOT NULL,
    permissions    INT          UNSIGNED NOT NULL DEFAULT 0,
    checksum       VARCHAR(255)           NULL,
    PRIMARY KEY (fileid),
    UNIQUE KEY fs_storage_path  (storage, path_hash),
    KEY        fs_parent        (parent),
    KEY        fs_mimepart      (mimepart),
    KEY        fs_mimetype      (mimetype),
    KEY        fs_storage_size  (storage, size),
    CONSTRAINT oc_filecache_storage_fk  FOREIGN KEY (storage)  REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    CONSTRAINT oc_filecache_mimetype_fk FOREIGN KEY (mimetype) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    CONSTRAINT oc_filecache_mimepart_fk FOREIGN KEY (mimepart) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    CONSTRAINT oc_filecache_parent_fk   FOREIGN KEY (parent)   REFERENCES oc_filecache(fileid)     ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
```

Create `migrations/core/0004_filecache/postgres.sql`:

```sql
CREATE TABLE oc_storages (
    numeric_id    SERIAL       PRIMARY KEY,
    id            VARCHAR(64)  NOT NULL UNIQUE,
    available     SMALLINT     NOT NULL DEFAULT 1,
    last_checked  INTEGER      NULL
);

CREATE TABLE oc_mimetypes (
    id        SERIAL        PRIMARY KEY,
    mimetype  VARCHAR(255)  NOT NULL UNIQUE
);

CREATE TABLE oc_filecache (
    fileid         BIGSERIAL     PRIMARY KEY,
    storage        INTEGER       NOT NULL REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    path           VARCHAR(4000) NOT NULL,
    path_hash      CHAR(32)      NOT NULL,
    parent         BIGINT        NULL REFERENCES oc_filecache(fileid) ON DELETE CASCADE,
    name           VARCHAR(250)  NOT NULL,
    mimetype       INTEGER       NOT NULL REFERENCES oc_mimetypes(id) ON DELETE RESTRICT,
    mimepart       INTEGER       NOT NULL REFERENCES oc_mimetypes(id) ON DELETE RESTRICT,
    size           BIGINT        NOT NULL DEFAULT 0,
    mtime          INTEGER       NOT NULL DEFAULT 0,
    storage_mtime  INTEGER       NOT NULL DEFAULT 0,
    encrypted      SMALLINT      NOT NULL DEFAULT 0,
    etag           VARCHAR(40)   NOT NULL,
    permissions    INTEGER       NOT NULL DEFAULT 0,
    checksum       VARCHAR(255)  NULL
);

CREATE UNIQUE INDEX fs_storage_path  ON oc_filecache (storage, path_hash);
CREATE        INDEX fs_parent        ON oc_filecache (parent);
CREATE        INDEX fs_mimepart      ON oc_filecache (mimepart);
CREATE        INDEX fs_mimetype      ON oc_filecache (mimetype);
CREATE        INDEX fs_storage_size  ON oc_filecache (storage, size);
```

### Step 4: Register migration in `crabcloud-db/src/core_migrations.rs`

Append after the `Migration { version: 3, name: "auth_tokens", ... }` entry, before the closing `];`:

```rust
    Migration {
        version: 4,
        name: "filecache",
        sqlite: include_str!("../../../migrations/core/0004_filecache/sqlite.sql"),
        mysql: include_str!("../../../migrations/core/0004_filecache/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0004_filecache/postgres.sql"),
    },
```

And update the existing `core_migration_applies_against_sqlite` test in the same file: `assert_eq!(applied, 3);` → `assert_eq!(applied, 4);`.

### Step 5: Create `crates/crabcloud-filecache/src/error.rs`

```rust
//! Error types for `crabcloud-filecache`.

use crabcloud_storage::{StorageError, StoragePath};

#[derive(Debug, thiserror::Error)]
pub enum FileCacheError {
    #[error("not found")]
    NotFound,
    #[error("ancestor missing: {0}")]
    AncestorMissing(StoragePath),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("invalid state: {0}")]
    Invalid(String),
}

pub type FileCacheResult<T> = Result<T, FileCacheError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_error_wraps() {
        let e: FileCacheError = sqlx::Error::RowNotFound.into();
        assert!(matches!(e, FileCacheError::Db(_)));
    }

    #[test]
    fn storage_error_wraps() {
        let e: FileCacheError = StorageError::NotFound.into();
        assert!(matches!(e, FileCacheError::Storage(_)));
    }

    #[test]
    fn ancestor_missing_holds_path() {
        let p = StoragePath::new("a/b").unwrap();
        let e = FileCacheError::AncestorMissing(p.clone());
        match e {
            FileCacheError::AncestorMissing(got) => assert_eq!(got, p),
            _ => panic!("wrong variant"),
        }
    }
}
```

### Step 6: Create `crates/crabcloud-filecache/src/schema.rs`

```rust
//! Row shapes used by `FileCache`. `FilecacheRow` is the public type;
//! `FilecacheRowRaw` is the per-dialect FromRow target that maps directly
//! to `oc_filecache` columns + the joined storage/mimetype strings.

use crabcloud_storage::{ETag, FileKind, Mimetype, Permissions, StoragePath};

use crate::error::{FileCacheError, FileCacheResult};

/// Public cache row. Fields are typed (StoragePath, ETag, etc.) — convert
/// from `FilecacheRowRaw` via `try_into`.
#[derive(Debug, Clone)]
pub struct FilecacheRow {
    pub fileid: i64,
    pub storage_id: String,
    pub path: StoragePath,
    pub parent: Option<i64>,
    pub name: String,
    pub kind: FileKind,
    pub mimetype: Mimetype,
    pub size: u64,
    pub mtime: u64,
    pub storage_mtime: u64,
    pub etag: ETag,
    pub permissions: Permissions,
}

/// SQL row shape: scalar columns + the two joined strings (storage id,
/// mimetype). Construct via the per-dialect SELECT queries in
/// `populate.rs` / `propagate.rs`.
#[derive(Debug, Clone)]
pub struct FilecacheRowRaw {
    pub fileid: i64,
    pub storage_id: String,
    pub path: String,
    pub parent: Option<i64>,
    pub name: String,
    pub mimetype: String,
    pub size: i64,
    pub mtime: i64,
    pub storage_mtime: i64,
    pub etag: String,
    pub permissions: i64,
}

impl FilecacheRowRaw {
    pub fn into_row(self) -> FileCacheResult<FilecacheRow> {
        let path = StoragePath::new(self.path).map_err(|e| {
            FileCacheError::Invalid(format!("filecache row has invalid path: {e}"))
        })?;
        let etag = ETag::from_hex(&self.etag).map_err(|e| {
            FileCacheError::Invalid(format!("filecache row has invalid etag: {e}"))
        })?;
        let mimetype = Mimetype::parse(&self.mimetype).map_err(|e| {
            FileCacheError::Invalid(format!("filecache row has invalid mimetype: {e}"))
        })?;
        let kind = if self.mimetype.starts_with("httpd/unix-directory") {
            FileKind::Directory
        } else {
            FileKind::File
        };
        Ok(FilecacheRow {
            fileid: self.fileid,
            storage_id: self.storage_id,
            path,
            parent: self.parent,
            name: self.name,
            kind,
            mimetype,
            size: self.size as u64,
            mtime: self.mtime as u64,
            storage_mtime: self.storage_mtime as u64,
            etag,
            permissions: Permissions::new((self.permissions as u8) & Permissions::ALL),
        })
    }
}

/// hex(md5(path)). Used as the indexed lookup column on `oc_filecache`.
/// Matches upstream Nextcloud's path_hash convention.
pub fn path_hash(path: &StoragePath) -> String {
    use md5::Digest;
    let digest = md5::Md5::digest(path.as_str().as_bytes());
    hex::encode(digest)
}

/// "image/png" -> "image" (the part used for `oc_filecache.mimepart`).
/// "x" without a slash returns "x" (mimetype constructor already rejects
/// missing-slash strings, but defend against bad rows from older DBs).
pub fn type_half(mimetype: &str) -> &str {
    mimetype.split_once('/').map(|(t, _)| t).unwrap_or(mimetype)
}

/// Marker mimetype for directories. Matches upstream Nextcloud.
pub const DIRECTORY_MIMETYPE: &str = "httpd/unix-directory";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_is_32_hex_chars() {
        let p = StoragePath::new("a/b/c.txt").unwrap();
        let h = path_hash(&p);
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn path_hash_root_is_md5_of_empty() {
        let h = path_hash(&StoragePath::root());
        // md5("") = d41d8cd98f00b204e9800998ecf8427e
        assert_eq!(h, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn path_hash_known_value() {
        let p = StoragePath::new("hello.txt").unwrap();
        // md5("hello.txt") = 09f7e02f1290be211da707a266f153b3
        assert_eq!(path_hash(&p), "09f7e02f1290be211da707a266f153b3");
    }

    #[test]
    fn type_half_splits_mimetype() {
        assert_eq!(type_half("image/png"), "image");
        assert_eq!(type_half("application/octet-stream"), "application");
        assert_eq!(type_half("text/x-rust"), "text");
    }

    #[test]
    fn type_half_passes_through_malformed() {
        assert_eq!(type_half("malformed"), "malformed");
    }
}
```

### Step 7: Create `crates/crabcloud-filecache/src/mimetypes.rs`

```rust
//! `oc_mimetypes` interning. Each mimetype string (full mimetype AND its
//! type-half) becomes a row; subsequent inserts re-use the existing id.
//! Per-process intern cache (`DashMap`) avoids redundant DB hits.

use crabcloud_db::DbPool;
use dashmap::DashMap;
use sqlx::Executor;

use crate::error::FileCacheResult;

/// Look up or insert a mimetype row, returning its `id`. Acts on the
/// per-process intern cache first; falls back to a per-dialect upsert.
pub async fn intern_mimetype<'e, E>(
    pool: &DbPool,
    cache: &DashMap<String, i64>,
    executor: E,
    mimetype: &str,
) -> FileCacheResult<i64>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    if let Some(id) = cache.get(mimetype) {
        return Ok(*id);
    }
    let id = upsert_mimetype(pool, executor, mimetype).await?;
    cache.insert(mimetype.to_string(), id);
    Ok(id)
}

async fn upsert_mimetype<'e, E>(
    pool: &DbPool,
    _executor: E,
    mimetype: &str,
) -> FileCacheResult<i64>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    // sqlx::Any here is too lossy for a real upsert across dialects; the
    // real implementation routes by `pool` arm. Below is the explicit
    // dispatch.
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("INSERT OR IGNORE INTO oc_mimetypes (mimetype) VALUES (?)")
                .bind(mimetype)
                .execute(p)
                .await
                .map_err(crate::error::FileCacheError::Db)?;
            let id: i64 = sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = ?")
                .bind(mimetype)
                .fetch_one(p)
                .await
                .map_err(crate::error::FileCacheError::Db)?;
            Ok(id)
        }
        DbPool::MySql(p) => {
            sqlx::query("INSERT IGNORE INTO oc_mimetypes (mimetype) VALUES (?)")
                .bind(mimetype)
                .execute(p)
                .await
                .map_err(crate::error::FileCacheError::Db)?;
            let id: u64 = sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = ?")
                .bind(mimetype)
                .fetch_one(p)
                .await
                .map_err(crate::error::FileCacheError::Db)?;
            Ok(id as i64)
        }
        DbPool::Postgres(p) => {
            sqlx::query(
                "INSERT INTO oc_mimetypes (mimetype) VALUES ($1) ON CONFLICT (mimetype) DO NOTHING",
            )
            .bind(mimetype)
            .execute(p)
            .await
            .map_err(crate::error::FileCacheError::Db)?;
            let id: i32 =
                sqlx::query_scalar("SELECT id FROM oc_mimetypes WHERE mimetype = $1")
                    .bind(mimetype)
                    .fetch_one(p)
                    .await
                    .map_err(crate::error::FileCacheError::Db)?;
            Ok(id as i64)
        }
    }
}

// Note: the `executor: E` parameter in `intern_mimetype` exists so the
// caller can pass an in-flight transaction to keep the upsert + select
// atomic. In Batch B we route through `pool` directly because sqlx::Any
// transactions don't compose with the typed `Pool<Sqlite>`/`Pool<MySql>`/
// `Pool<Postgres>` arms. The `_executor` parameter is currently unused
// but reserved for the dispatch refactor in Batch B.

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("m.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn intern_mimetype_returns_stable_id() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let a = intern_mimetype(&pool, &cache, &pool.as_any_executor(), "image/png")
            .await
            .unwrap();
        let b = intern_mimetype(&pool, &cache, &pool.as_any_executor(), "image/png")
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn intern_mimetype_uses_cache_on_repeat() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        intern_mimetype(&pool, &cache, &pool.as_any_executor(), "image/png")
            .await
            .unwrap();
        assert_eq!(cache.len(), 1);
        intern_mimetype(&pool, &cache, &pool.as_any_executor(), "image/png")
            .await
            .unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn intern_distinct_mimetypes_get_distinct_ids() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let png = intern_mimetype(&pool, &cache, &pool.as_any_executor(), "image/png")
            .await
            .unwrap();
        let txt = intern_mimetype(&pool, &cache, &pool.as_any_executor(), "text/plain")
            .await
            .unwrap();
        assert_ne!(png, txt);
    }
}
```

**Note on `pool.as_any_executor()`:** the existing `DbPool` enum doesn't expose an `Any` executor for type-uniformity in tests. The `_executor: E` parameter on `intern_mimetype` is reserved for Batch B when we replace it with a real transaction handle (per-dialect dispatch). For Batch A, you can either:
1. Drop the `_executor` parameter from the signature entirely (simplifies tests; Batch B re-introduces it with the proper transaction type via the `db_dispatch!` macro pattern from `crabcloud-db`).
2. Stub `as_any_executor()` on `DbPool` returning the right thing for sqlx::Any.

**Recommended:** option 1 — drop the `executor: E` parameter for Batch A. Batch B will reintroduce per-dialect tx handles. Adjust the signature:

```rust
pub async fn intern_mimetype(
    pool: &DbPool,
    cache: &DashMap<String, i64>,
    mimetype: &str,
) -> FileCacheResult<i64>
```

…and remove the `executor: E` argument from the body + test call sites. The non-transactional path is correct for Batch A's purposes (the test asserts ID stability + cache reuse, not transactional atomicity).

### Step 8: Create `crates/crabcloud-filecache/src/storages.rs`

Same pattern as `mimetypes.rs` — upsert storage id (matching upstream Nextcloud's auto-incrementing `numeric_id`), cache locally.

```rust
//! `oc_storages` interning. Each `Storage::id()` string becomes a row;
//! subsequent inserts re-use the existing `numeric_id`. Per-process intern
//! cache (`DashMap`) avoids redundant DB hits.

use crabcloud_db::DbPool;
use dashmap::DashMap;

use crate::error::{FileCacheError, FileCacheResult};

pub async fn intern_storage(
    pool: &DbPool,
    cache: &DashMap<String, i64>,
    storage_id: &str,
) -> FileCacheResult<i64> {
    if let Some(id) = cache.get(storage_id) {
        return Ok(*id);
    }
    let id = upsert_storage(pool, storage_id).await?;
    cache.insert(storage_id.to_string(), id);
    Ok(id)
}

async fn upsert_storage(pool: &DbPool, storage_id: &str) -> FileCacheResult<i64> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("INSERT OR IGNORE INTO oc_storages (id) VALUES (?)")
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: i64 = sqlx::query_scalar("SELECT numeric_id FROM oc_storages WHERE id = ?")
                .bind(storage_id)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id)
        }
        DbPool::MySql(p) => {
            sqlx::query("INSERT IGNORE INTO oc_storages (id) VALUES (?)")
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
            let id: u32 = sqlx::query_scalar("SELECT numeric_id FROM oc_storages WHERE id = ?")
                .bind(storage_id)
                .fetch_one(p)
                .await
                .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
        DbPool::Postgres(p) => {
            sqlx::query(
                "INSERT INTO oc_storages (id) VALUES ($1) ON CONFLICT (id) DO NOTHING",
            )
            .bind(storage_id)
            .execute(p)
            .await
            .map_err(FileCacheError::Db)?;
            let id: i32 = sqlx::query_scalar(
                "SELECT numeric_id FROM oc_storages WHERE id = $1",
            )
            .bind(storage_id)
            .fetch_one(p)
            .await
            .map_err(FileCacheError::Db)?;
            Ok(id as i64)
        }
    }
}

/// Update `oc_storages.last_checked` to the current unix timestamp.
/// Idempotent; called at the end of `Scanner::full_scan`.
pub async fn stamp_last_checked(pool: &DbPool, storage_id: &str) -> FileCacheResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = ? WHERE id = ?")
                .bind(now)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = ? WHERE id = ?")
                .bind(now)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            sqlx::query("UPDATE oc_storages SET last_checked = $1 WHERE id = $2")
                .bind(now as i32)
                .bind(storage_id)
                .execute(p)
                .await
                .map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("s.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn intern_storage_returns_stable_id() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let a = intern_storage(&pool, &cache, "local::/srv/data/alice").await.unwrap();
        let b = intern_storage(&pool, &cache, "local::/srv/data/alice").await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn intern_distinct_storages_get_distinct_ids() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        let alice = intern_storage(&pool, &cache, "local::/srv/data/alice").await.unwrap();
        let bob = intern_storage(&pool, &cache, "local::/srv/data/bob").await.unwrap();
        assert_ne!(alice, bob);
    }

    #[tokio::test]
    async fn stamp_last_checked_updates_row() {
        let pool = fresh_pool().await;
        let cache: DashMap<String, i64> = DashMap::new();
        intern_storage(&pool, &cache, "local::/x").await.unwrap();
        stamp_last_checked(&pool, "local::/x").await.unwrap();
        let DbPool::Sqlite(p) = &pool else { panic!() };
        let lc: Option<i64> = sqlx::query_scalar(
            "SELECT last_checked FROM oc_storages WHERE id = ?",
        )
        .bind("local::/x")
        .fetch_one(p)
        .await
        .unwrap();
        assert!(lc.is_some());
    }
}
```

### Step 9: Create `crates/crabcloud-filecache/src/lib.rs`

```rust
//! `crabcloud-filecache` — DB-backed cache for storage state.
//!
//! Mirrors 4a's storage events in `oc_filecache`/`oc_storages`/`oc_mimetypes`
//! so consumers (sub-project 5's WebDAV, future indexes) can serve `stat`/
//! `list` in O(1). Cache-miss populate happens through real-backend stats
//! under a per-path lock. Ancestor `size` + `etag` propagation runs in one
//! DB transaction per event — matches upstream Nextcloud behavior so desktop
//! sync clients see byte-identical ETags at every level.

pub mod error;
pub mod mimetypes;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

// Batches B–E will add:
//   pub mod populate;
//   pub mod propagate;
//   pub mod scanner;
//   pub struct FileCache { ... }
//   pub struct Scanner { ... }
```

### Step 10: Run + commit + push + open Batch A PR

```
cargo build -p crabcloud-filecache
cargo test -p crabcloud-filecache --lib
cargo test -p crabcloud-db --lib core_migration_applies_against_sqlite
cargo xtask check-all
```

Expected: builds clean; error/schema/mimetypes/storages unit tests pass (~12 tests total); migration runs the expected `4` migrations now.

```
git add Cargo.toml crates/crabcloud-filecache crates/crabcloud-db/src/core_migrations.rs migrations/core/0004_filecache
git commit -m "feat(filecache): crate skeleton + migration 0004 + interning helpers

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-a
gh pr create --base master --head filecache-batch-a \
  --title "filecache: batch A — crate skeleton + 0004_filecache migration + interning" \
  --body "Sub-project 4b, batch A: new \`crabcloud-filecache\` crate (path_hash + mimetype/storage interning helpers + FilecacheRow + FileCacheError), migration 0004_filecache for sqlite/mysql/postgres (3 tables + 5 indexes + 4 FKs), and registration. No FileCache facade yet — that's Batch B."
```

**STOP.** Do NOT call `gh pr merge`.

---

## Task 2: `FileCache::apply` + ancestor propagation (Batch B)

**Files:**
- Create: `crates/crabcloud-filecache/src/propagate.rs`
- Modify: `crates/crabcloud-filecache/src/lib.rs` (add `FileCache` struct + `apply` method)
- Create: `crates/crabcloud-filecache/tests/apply_events.rs`
- Create: `crates/crabcloud-filecache/tests/support/mod.rs`

### Step 1: Branch + start with the `FileCache` skeleton

```
git checkout -b filecache-batch-b origin/master
```

Replace `crates/crabcloud-filecache/src/lib.rs`:

```rust
//! `crabcloud-filecache` — DB-backed cache for storage state.

pub mod error;
pub mod mimetypes;
pub mod propagate;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

use crabcloud_db::DbPool;
use crabcloud_storage::{StorageEvent, StoragePath};
use dashmap::DashMap;

/// The cache façade. Constructed via [`FileCache::new`]; subsequent reads
/// (`lookup`/`lookup_by_id`) and writes (`apply`) all dispatch through the
/// shared `DbPool`. Per-process intern caches for storages + mimetypes
/// keep round-trip cost down on the hot path.
pub struct FileCache {
    pool: DbPool,
    pub(crate) storage_ids: DashMap<String, i64>,
    pub(crate) mimetypes: DashMap<String, i64>,
}

impl FileCache {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            storage_ids: DashMap::new(),
            mimetypes: DashMap::new(),
        }
    }

    pub(crate) fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Apply a `StorageEvent` to the cache. Each event handler runs its
    /// leaf mutation + ancestor propagation in one transaction.
    pub async fn apply(&self, event: &StorageEvent) -> FileCacheResult<()> {
        propagate::apply_event(self, event).await
    }

    /// Lookup a row by `(storage_id, path)` without populating on miss.
    pub async fn lookup(
        &self,
        storage_id: &str,
        path: &StoragePath,
    ) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row(self, storage_id, path).await
    }

    /// Lookup a row by `fileid`.
    pub async fn lookup_by_id(
        &self,
        fileid: i64,
    ) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row_by_id(self, fileid).await
    }
}
```

### Step 2: Create `crates/crabcloud-filecache/src/propagate.rs`

This is the largest single file in Batch B. The 5 event handlers + ancestor walk + lookup helpers all live here.

```rust
//! Single transactional apply path for each `StorageEvent` variant. Walks
//! the ancestor chain, propagates size delta + fresh ETag, commits.

use crabcloud_db::DbPool;
use crabcloud_storage::{
    ETag, FileMetadata, Mimetype, Permissions, StorageEvent, StoragePath,
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{FileCacheError, FileCacheResult};
use crate::mimetypes::intern_mimetype;
use crate::schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};
use crate::storages::intern_storage;
use crate::FileCache;

const SELECT_ROW_BY_STORAGE_PATH_SQLITE: &str = "
    SELECT
        f.fileid,
        s.id as storage_id,
        f.path,
        f.parent,
        f.name,
        m.mimetype as mimetype,
        f.size,
        f.mtime,
        f.storage_mtime,
        f.etag,
        f.permissions
    FROM oc_filecache f
    JOIN oc_storages  s ON s.numeric_id = f.storage
    JOIN oc_mimetypes m ON m.id = f.mimetype
    WHERE s.id = ? AND f.path_hash = ?
";

const SELECT_ROW_BY_STORAGE_PATH_MYSQL: &str = SELECT_ROW_BY_STORAGE_PATH_SQLITE;
const SELECT_ROW_BY_STORAGE_PATH_PG: &str = "
    SELECT
        f.fileid,
        s.id as storage_id,
        f.path,
        f.parent,
        f.name,
        m.mimetype as mimetype,
        f.size,
        f.mtime,
        f.storage_mtime,
        f.etag,
        f.permissions
    FROM oc_filecache f
    JOIN oc_storages  s ON s.numeric_id = f.storage
    JOIN oc_mimetypes m ON m.id = f.mimetype
    WHERE s.id = $1 AND f.path_hash = $2
";

/// Apply one event in one transaction.
pub async fn apply_event(cache: &FileCache, event: &StorageEvent) -> FileCacheResult<()> {
    match event {
        StorageEvent::Written {
            storage_id,
            path,
            metadata,
        } => apply_written(cache, storage_id, path, metadata, false).await,
        StorageEvent::DirCreated {
            storage_id,
            path,
            metadata,
        } => apply_written(cache, storage_id, path, metadata, true).await,
        StorageEvent::Deleted { storage_id, path } => {
            apply_deleted(cache, storage_id, path).await
        }
        StorageEvent::Moved {
            storage_id,
            from,
            to,
        } => apply_moved(cache, storage_id, from, to).await,
        StorageEvent::Copied {
            storage_id,
            from,
            to,
        } => apply_copied(cache, storage_id, from, to).await,
    }
}

/// Lookup `(storage_id, path)` -> row.
pub async fn lookup_row(
    cache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
) -> FileCacheResult<Option<FilecacheRow>> {
    let ph = path_hash(path);
    let row: Option<FilecacheRowRaw> = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(
            SELECT_ROW_BY_STORAGE_PATH_SQLITE,
            (storage_id, ph.as_str()),
        )
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?,
        DbPool::MySql(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(
            SELECT_ROW_BY_STORAGE_PATH_MYSQL,
            (storage_id, ph.as_str()),
        )
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?,
        DbPool::Postgres(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(
            SELECT_ROW_BY_STORAGE_PATH_PG,
            (storage_id, ph.as_str()),
        )
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?,
    };
    row.map(FilecacheRowRaw::into_row).transpose()
}

/// Lookup by `fileid`.
pub async fn lookup_row_by_id(
    cache: &FileCache,
    fileid: i64,
) -> FileCacheResult<Option<FilecacheRow>> {
    let sql_sqlite = "SELECT f.fileid, s.id as storage_id, f.path, f.parent, f.name,
        m.mimetype as mimetype, f.size, f.mtime, f.storage_mtime, f.etag, f.permissions
        FROM oc_filecache f
        JOIN oc_storages  s ON s.numeric_id = f.storage
        JOIN oc_mimetypes m ON m.id = f.mimetype
        WHERE f.fileid = ?";
    let sql_pg = "SELECT f.fileid, s.id as storage_id, f.path, f.parent, f.name,
        m.mimetype as mimetype, f.size, f.mtime, f.storage_mtime, f.etag, f.permissions
        FROM oc_filecache f
        JOIN oc_storages  s ON s.numeric_id = f.storage
        JOIN oc_mimetypes m ON m.id = f.mimetype
        WHERE f.fileid = $1";
    let row: Option<FilecacheRowRaw> = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(sql_sqlite, (fileid,))
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
        DbPool::MySql(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(sql_sqlite, (fileid,))
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
        DbPool::Postgres(p) => sqlx::query_as_with::<_, FilecacheRowRaw, _>(sql_pg, (fileid,))
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?,
    };
    row.map(FilecacheRowRaw::into_row).transpose()
}

async fn apply_written(
    cache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
    metadata: &FileMetadata,
    is_dir: bool,
) -> FileCacheResult<()> {
    // Intern storage + mimetypes outside the tx (each is its own upsert);
    // the in-process cache keeps repeat hits cheap.
    let storage_pk = intern_storage(cache.pool(), &cache.storage_ids, storage_id).await?;
    let mimetype_str = if is_dir {
        DIRECTORY_MIMETYPE.to_string()
    } else {
        metadata.mimetype.as_str().to_string()
    };
    let mimepart_str = type_half(&mimetype_str).to_string();
    let mimetype_pk =
        intern_mimetype(cache.pool(), &cache.mimetypes, &mimetype_str).await?;
    let mimepart_pk =
        intern_mimetype(cache.pool(), &cache.mimetypes, &mimepart_str).await?;

    let new_size = if is_dir { 0i64 } else { metadata.size as i64 };
    let new_etag = metadata.etag.as_str().to_string();
    let mtime = sys_to_unix(metadata.mtime);
    let permissions = metadata.permissions.bits() as i64;

    // Resolve parent fileid (if any) — must already exist or AncestorMissing.
    let parent_fileid =
        resolve_parent_fileid(cache, storage_pk, path).await?;

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) =
                upsert_leaf_sqlite(&mut tx, storage_pk, path, parent_fileid, mimetype_pk,
                    mimepart_pk, new_size, mtime, mtime, &new_etag, permissions).await?;
            let delta = new_size - old_size;
            propagate_ancestors_sqlite(&mut tx, storage_pk, path, delta, mtime).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) =
                upsert_leaf_mysql(&mut tx, storage_pk, path, parent_fileid, mimetype_pk,
                    mimepart_pk, new_size, mtime, mtime, &new_etag, permissions).await?;
            let delta = new_size - old_size;
            propagate_ancestors_mysql(&mut tx, storage_pk, path, delta, mtime).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) =
                upsert_leaf_postgres(&mut tx, storage_pk, path, parent_fileid, mimetype_pk,
                    mimepart_pk, new_size, mtime, mtime, &new_etag, permissions).await?;
            let delta = new_size - old_size;
            propagate_ancestors_postgres(&mut tx, storage_pk, path, delta, mtime).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

async fn apply_deleted(
    cache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
) -> FileCacheResult<()> {
    let storage_pk = intern_storage(cache.pool(), &cache.storage_ids, storage_id).await?;
    let ph = path_hash(path);

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(i64, i64, i64)> = sqlx::query_as(
                "SELECT fileid, size, mtime FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            )
            .bind(storage_pk).bind(&ph)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size, _)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            sqlx::query("DELETE FROM oc_filecache WHERE storage = ? AND path_hash = ?")
                .bind(storage_pk).bind(&ph)
                .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            let now = unix_now();
            propagate_ancestors_sqlite(&mut tx, storage_pk, path, -old_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(u64, i64, i64)> = sqlx::query_as(
                "SELECT fileid, size, mtime FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            )
            .bind(storage_pk as u32).bind(&ph)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size, _)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            sqlx::query("DELETE FROM oc_filecache WHERE storage = ? AND path_hash = ?")
                .bind(storage_pk as u32).bind(&ph)
                .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            let now = unix_now();
            propagate_ancestors_mysql(&mut tx, storage_pk, path, -old_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(i64, i64, i64)> = sqlx::query_as(
                "SELECT fileid, size, mtime FROM oc_filecache WHERE storage = $1 AND path_hash = $2"
            )
            .bind(storage_pk as i32).bind(&ph)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size, _)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            sqlx::query("DELETE FROM oc_filecache WHERE storage = $1 AND path_hash = $2")
                .bind(storage_pk as i32).bind(&ph)
                .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            let now = unix_now();
            propagate_ancestors_postgres(&mut tx, storage_pk, path, -old_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

async fn apply_moved(
    cache: &FileCache,
    storage_id: &str,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let storage_pk = intern_storage(cache.pool(), &cache.storage_ids, storage_id).await?;
    let from_hash = path_hash(from);
    let to_hash = path_hash(to);
    let to_parent_pk = resolve_parent_fileid(cache, storage_pk, to).await?;
    let new_name = to.basename().to_string();

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            // Lookup source row.
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            // Update the leaf.
            sqlx::query(
                "UPDATE oc_filecache SET path = ?, path_hash = ?, parent = ?, name = ? WHERE fileid = ?"
            )
            .bind(to.as_str()).bind(&to_hash).bind(to_parent_pk).bind(&new_name).bind(leaf_id)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            // If the leaf is a directory, rewrite every descendant's path.
            rewrite_descendant_paths_sqlite(&mut tx, storage_pk, from, to).await?;
            let now = unix_now();
            // Cross-parent: subtract from source chain + add to dest chain.
            if from.parent() != to.parent() {
                propagate_ancestors_sqlite(&mut tx, storage_pk, from, -old_size, now).await?;
                propagate_ancestors_sqlite(&mut tx, storage_pk, to, old_size, now).await?;
            } else {
                propagate_ancestors_sqlite(&mut tx, storage_pk, to, 0, now).await?;
            }
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(u64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk as u32).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "UPDATE oc_filecache SET path = ?, path_hash = ?, parent = ?, name = ? WHERE fileid = ?"
            )
            .bind(to.as_str()).bind(&to_hash).bind(to_parent_pk).bind(&new_name).bind(leaf_id as u64)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            rewrite_descendant_paths_mysql(&mut tx, storage_pk, from, to).await?;
            let now = unix_now();
            if from.parent() != to.parent() {
                propagate_ancestors_mysql(&mut tx, storage_pk, from, -old_size, now).await?;
                propagate_ancestors_mysql(&mut tx, storage_pk, to, old_size, now).await?;
            } else {
                propagate_ancestors_mysql(&mut tx, storage_pk, to, 0, now).await?;
            }
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = $1 AND path_hash = $2",
            )
            .bind(storage_pk as i32).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "UPDATE oc_filecache SET path = $1, path_hash = $2, parent = $3, name = $4 WHERE fileid = $5"
            )
            .bind(to.as_str()).bind(&to_hash).bind(to_parent_pk).bind(&new_name).bind(leaf_id)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            rewrite_descendant_paths_postgres(&mut tx, storage_pk, from, to).await?;
            let now = unix_now();
            if from.parent() != to.parent() {
                propagate_ancestors_postgres(&mut tx, storage_pk, from, -old_size, now).await?;
                propagate_ancestors_postgres(&mut tx, storage_pk, to, old_size, now).await?;
            } else {
                propagate_ancestors_postgres(&mut tx, storage_pk, to, 0, now).await?;
            }
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

async fn apply_copied(
    cache: &FileCache,
    storage_id: &str,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let storage_pk = intern_storage(cache.pool(), &cache.storage_ids, storage_id).await?;
    let from_hash = path_hash(from);
    let to_hash = path_hash(to);
    let to_parent_pk = resolve_parent_fileid(cache, storage_pk, to).await?;
    let new_name = to.basename().to_string();
    let new_etag = ETag::new().as_str().to_string();
    let now = unix_now();

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let src: Option<(i64, i64, i64, i64)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            )
            .bind(storage_pk).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_src_id, mimetype_pk, mimepart_pk, src_size)) = src else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(storage_pk).bind(to.as_str()).bind(&to_hash).bind(to_parent_pk)
            .bind(&new_name).bind(mimetype_pk).bind(mimepart_pk)
            .bind(src_size).bind(now).bind(now).bind(&new_etag).bind(0i64)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            propagate_ancestors_sqlite(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let src: Option<(u64, u32, u32, i64)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            )
            .bind(storage_pk as u32).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_src_id, mimetype_pk, mimepart_pk, src_size)) = src else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(storage_pk as u32).bind(to.as_str()).bind(&to_hash).bind(to_parent_pk.map(|x| x as u64))
            .bind(&new_name).bind(mimetype_pk).bind(mimepart_pk)
            .bind(src_size).bind(now).bind(now).bind(&new_etag).bind(0u32)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            propagate_ancestors_mysql(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let src: Option<(i64, i32, i32, i64)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size FROM oc_filecache WHERE storage = $1 AND path_hash = $2"
            )
            .bind(storage_pk as i32).bind(&from_hash)
            .fetch_optional(&mut *tx).await.map_err(FileCacheError::Db)?;
            let Some((_src_id, mimetype_pk, mimepart_pk, src_size)) = src else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"
            )
            .bind(storage_pk as i32).bind(to.as_str()).bind(&to_hash).bind(to_parent_pk)
            .bind(&new_name).bind(mimetype_pk).bind(mimepart_pk)
            .bind(src_size).bind(now as i32).bind(now as i32).bind(&new_etag).bind(0i32)
            .execute(&mut *tx).await.map_err(FileCacheError::Db)?;
            propagate_ancestors_postgres(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

// --- per-dialect helpers ---

async fn upsert_leaf_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    storage_pk: i64,
    path: &StoragePath,
    parent_pk: Option<i64>,
    mimetype_pk: i64,
    mimepart_pk: i64,
    new_size: i64,
    mtime: i64,
    storage_mtime: i64,
    etag: &str,
    permissions: i64,
) -> FileCacheResult<(i64, Option<i64>)> {
    let ph = path_hash(path);
    let existing: Option<(i64, i64)> = sqlx::query_as(
        "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
    )
    .bind(storage_pk).bind(&ph)
    .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = ?, name = ?, mimetype = ?, mimepart = ?,
             size = ?, mtime = ?, storage_mtime = ?, etag = ?, permissions = ?
             WHERE fileid = ?"
        )
        .bind(parent_pk).bind(path.basename())
        .bind(mimetype_pk).bind(mimepart_pk)
        .bind(new_size).bind(mtime).bind(storage_mtime).bind(etag).bind(permissions)
        .bind(id)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(storage_pk).bind(path.as_str()).bind(&ph).bind(parent_pk)
        .bind(path.basename()).bind(mimetype_pk).bind(mimepart_pk)
        .bind(new_size).bind(mtime).bind(storage_mtime).bind(etag).bind(permissions)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

async fn upsert_leaf_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    storage_pk: i64,
    path: &StoragePath,
    parent_pk: Option<i64>,
    mimetype_pk: i64,
    mimepart_pk: i64,
    new_size: i64,
    mtime: i64,
    storage_mtime: i64,
    etag: &str,
    permissions: i64,
) -> FileCacheResult<(i64, Option<i64>)> {
    let ph = path_hash(path);
    let existing: Option<(u64, i64)> = sqlx::query_as(
        "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
    )
    .bind(storage_pk as u32).bind(&ph)
    .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id as i64)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = ?, name = ?, mimetype = ?, mimepart = ?,
             size = ?, mtime = ?, storage_mtime = ?, etag = ?, permissions = ?
             WHERE fileid = ?"
        )
        .bind(parent_pk.map(|x| x as u64)).bind(path.basename())
        .bind(mimetype_pk as u32).bind(mimepart_pk as u32)
        .bind(new_size).bind(mtime).bind(storage_mtime).bind(etag).bind(permissions as u32)
        .bind(id as u64)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(storage_pk as u32).bind(path.as_str()).bind(&ph).bind(parent_pk.map(|x| x as u64))
        .bind(path.basename()).bind(mimetype_pk as u32).bind(mimepart_pk as u32)
        .bind(new_size).bind(mtime).bind(storage_mtime).bind(etag).bind(permissions as u32)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

async fn upsert_leaf_postgres(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    storage_pk: i64,
    path: &StoragePath,
    parent_pk: Option<i64>,
    mimetype_pk: i64,
    mimepart_pk: i64,
    new_size: i64,
    mtime: i64,
    storage_mtime: i64,
    etag: &str,
    permissions: i64,
) -> FileCacheResult<(i64, Option<i64>)> {
    let ph = path_hash(path);
    let existing: Option<(i64, i64)> = sqlx::query_as(
        "SELECT fileid, size FROM oc_filecache WHERE storage = $1 AND path_hash = $2",
    )
    .bind(storage_pk as i32).bind(&ph)
    .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = $1, name = $2, mimetype = $3, mimepart = $4,
             size = $5, mtime = $6, storage_mtime = $7, etag = $8, permissions = $9
             WHERE fileid = $10"
        )
        .bind(parent_pk).bind(path.basename())
        .bind(mimetype_pk as i32).bind(mimepart_pk as i32)
        .bind(new_size).bind(mtime as i32).bind(storage_mtime as i32).bind(etag).bind(permissions as i32)
        .bind(id)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, mimetype, mimepart, size, mtime, storage_mtime, etag, permissions)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"
        )
        .bind(storage_pk as i32).bind(path.as_str()).bind(&ph).bind(parent_pk)
        .bind(path.basename()).bind(mimetype_pk as i32).bind(mimepart_pk as i32)
        .bind(new_size).bind(mtime as i32).bind(storage_mtime as i32).bind(etag).bind(permissions as i32)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

async fn propagate_ancestors_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    storage_pk: i64,
    leaf: &StoragePath,
    delta: i64,
    mtime: i64,
) -> FileCacheResult<()> {
    let mut cur = leaf.parent();
    while let Some(anc) = cur {
        let ph = path_hash(&anc);
        let fileid: Option<i64> = sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
        )
        .bind(storage_pk).bind(&ph)
        .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() { break; }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + ?, etag = ?, mtime = ? WHERE fileid = ?"
        )
        .bind(delta).bind(&new_etag).bind(mtime).bind(id)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
        if anc.is_root() { break; }
        cur = anc.parent();
    }
    Ok(())
}

async fn propagate_ancestors_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    storage_pk: i64,
    leaf: &StoragePath,
    delta: i64,
    mtime: i64,
) -> FileCacheResult<()> {
    let mut cur = leaf.parent();
    while let Some(anc) = cur {
        let ph = path_hash(&anc);
        let fileid: Option<u64> = sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
        )
        .bind(storage_pk as u32).bind(&ph)
        .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() { break; }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + ?, etag = ?, mtime = ? WHERE fileid = ?"
        )
        .bind(delta).bind(&new_etag).bind(mtime).bind(id)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
        if anc.is_root() { break; }
        cur = anc.parent();
    }
    Ok(())
}

async fn propagate_ancestors_postgres(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    storage_pk: i64,
    leaf: &StoragePath,
    delta: i64,
    mtime: i64,
) -> FileCacheResult<()> {
    let mut cur = leaf.parent();
    while let Some(anc) = cur {
        let ph = path_hash(&anc);
        let fileid: Option<i64> = sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = $1 AND path_hash = $2"
        )
        .bind(storage_pk as i32).bind(&ph)
        .fetch_optional(&mut **tx).await.map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() { break; }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + $1, etag = $2, mtime = $3 WHERE fileid = $4"
        )
        .bind(delta).bind(&new_etag).bind(mtime as i32).bind(id)
        .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
        if anc.is_root() { break; }
        cur = anc.parent();
    }
    Ok(())
}

async fn rewrite_descendant_paths_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = format!("{}/", from.as_str());
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT fileid, path FROM oc_filecache WHERE storage = ? AND path LIKE ?",
    )
    .bind(storage_pk).bind(format!("{}%", from_prefix))
    .fetch_all(&mut **tx).await.map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() { suffix.to_string() } else { format!("{}/{}", to.as_str(), suffix) };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = ?, path_hash = ? WHERE fileid = ?")
            .bind(&new_path).bind(&new_hash).bind(fileid)
            .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok(())
}

async fn rewrite_descendant_paths_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = format!("{}/", from.as_str());
    let rows: Vec<(u64, String)> = sqlx::query_as(
        "SELECT fileid, path FROM oc_filecache WHERE storage = ? AND path LIKE ?",
    )
    .bind(storage_pk as u32).bind(format!("{}%", from_prefix))
    .fetch_all(&mut **tx).await.map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() { suffix.to_string() } else { format!("{}/{}", to.as_str(), suffix) };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = ?, path_hash = ? WHERE fileid = ?")
            .bind(&new_path).bind(&new_hash).bind(fileid)
            .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok(())
}

async fn rewrite_descendant_paths_postgres(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = format!("{}/", from.as_str());
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT fileid, path FROM oc_filecache WHERE storage = $1 AND path LIKE $2",
    )
    .bind(storage_pk as i32).bind(format!("{}%", from_prefix))
    .fetch_all(&mut **tx).await.map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() { suffix.to_string() } else { format!("{}/{}", to.as_str(), suffix) };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = $1, path_hash = $2 WHERE fileid = $3")
            .bind(&new_path).bind(&new_hash).bind(fileid)
            .execute(&mut **tx).await.map_err(FileCacheError::Db)?;
    }
    Ok(())
}

async fn resolve_parent_fileid(
    cache: &FileCache,
    storage_pk: i64,
    path: &StoragePath,
) -> FileCacheResult<Option<i64>> {
    let Some(parent) = path.parent() else { return Ok(None) };
    if parent.is_root() {
        // Special case: parent of "foo" is the empty-path root row, which
        // may or may not be present. If absent, return None — the root row
        // is optional and inserted lazily by directory operations.
        let ph = path_hash(&parent);
        let fileid: Option<i64> = match cache.pool() {
            DbPool::Sqlite(p) => sqlx::query_scalar(
                "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            ).bind(storage_pk).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?,
            DbPool::MySql(p) => sqlx::query_scalar::<_, u64>(
                "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
            ).bind(storage_pk as u32).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?.map(|x| x as i64),
            DbPool::Postgres(p) => sqlx::query_scalar(
                "SELECT fileid FROM oc_filecache WHERE storage = $1 AND path_hash = $2"
            ).bind(storage_pk as i32).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?,
        };
        return Ok(fileid);
    }
    let ph = path_hash(&parent);
    let fileid: Option<i64> = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
        ).bind(storage_pk).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?,
        DbPool::MySql(p) => sqlx::query_scalar::<_, u64>(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?"
        ).bind(storage_pk as u32).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?.map(|x| x as i64),
        DbPool::Postgres(p) => sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = $1 AND path_hash = $2"
        ).bind(storage_pk as i32).bind(&ph).fetch_optional(p).await.map_err(FileCacheError::Db)?,
    };
    match fileid {
        Some(id) => Ok(Some(id)),
        None => Err(FileCacheError::AncestorMissing(parent)),
    }
}

fn sys_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn unix_now() -> i64 {
    sys_to_unix(SystemTime::now())
}
```

### Step 3: Create `crates/crabcloud-filecache/tests/support/mod.rs`

```rust
//! Shared test fixtures: SQLite pool with migrations applied, fixtures for
//! constructing FileMetadata.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_storage::{ETag, FileKind, FileMetadata, Mimetype, Permissions, StoragePath};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

pub fn make_metadata(path: &str, size: u64, mimetype: &str) -> FileMetadata {
    FileMetadata {
        path: StoragePath::new(path).unwrap(),
        kind: FileKind::File,
        size,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(mimetype).unwrap(),
        permissions: Permissions::full(),
    }
}

pub fn make_dir_metadata(path: &str) -> FileMetadata {
    FileMetadata {
        path: StoragePath::new(path).unwrap(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::octet_stream(),
        permissions: Permissions::full(),
    }
}

pub struct Harness {
    pub pool: DbPool,
    pub _tempdir: TempDir,
}

pub async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("h.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    Harness { pool, _tempdir: dir }
}
```

### Step 4: Create `crates/crabcloud-filecache/tests/apply_events.rs`

The 6 integration tests. Each builds a `Harness`, constructs a `FileCache`, applies events, asserts cache state.

```rust
mod support;

use crabcloud_filecache::{FileCache, FileCacheError};
use crabcloud_storage::{StorageEvent, StoragePath};
use support::{harness, make_dir_metadata, make_metadata};

const SID: &str = "local::/test";

#[tokio::test]
async fn apply_written_event_inserts_leaf_with_metadata() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    // Need to seed the root directory first; otherwise the leaf has no
    // resolvable parent. Apply DirCreated on root.
    let root_md = make_dir_metadata("");
    cache
        .apply(&StorageEvent::DirCreated {
            storage_id: SID.into(),
            path: StoragePath::root(),
            metadata: root_md,
        })
        .await
        .unwrap();

    let md = make_metadata("hello.txt", 5, "text/plain");
    cache
        .apply(&StorageEvent::Written {
            storage_id: SID.into(),
            path: StoragePath::new("hello.txt").unwrap(),
            metadata: md.clone(),
        })
        .await
        .unwrap();

    let row = cache
        .lookup(SID, &StoragePath::new("hello.txt").unwrap())
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.name, "hello.txt");
    assert_eq!(row.size, 5);
    assert_eq!(row.mimetype.as_str(), "text/plain");
    assert_eq!(row.etag, md.etag);
}

#[tokio::test]
async fn apply_propagates_size_and_etag_up_chain() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::root(),
        metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::new("a").unwrap(),
        metadata: make_dir_metadata("a"),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::new("a/b").unwrap(),
        metadata: make_dir_metadata("a/b"),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::new("a/b/c").unwrap(),
        metadata: make_dir_metadata("a/b/c"),
    }).await.unwrap();

    let root_before = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();
    let a_before = cache.lookup(SID, &StoragePath::new("a").unwrap()).await.unwrap().unwrap();
    let ab_before = cache.lookup(SID, &StoragePath::new("a/b").unwrap()).await.unwrap().unwrap();
    let abc_before = cache.lookup(SID, &StoragePath::new("a/b/c").unwrap()).await.unwrap().unwrap();

    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("a/b/c/file.txt").unwrap(),
        metadata: make_metadata("a/b/c/file.txt", 100, "text/plain"),
    }).await.unwrap();

    let root_after = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();
    let a_after = cache.lookup(SID, &StoragePath::new("a").unwrap()).await.unwrap().unwrap();
    let ab_after = cache.lookup(SID, &StoragePath::new("a/b").unwrap()).await.unwrap().unwrap();
    let abc_after = cache.lookup(SID, &StoragePath::new("a/b/c").unwrap()).await.unwrap().unwrap();

    assert_eq!(root_after.size, root_before.size + 100);
    assert_eq!(a_after.size,    a_before.size    + 100);
    assert_eq!(ab_after.size,   ab_before.size   + 100);
    assert_eq!(abc_after.size,  abc_before.size  + 100);

    assert_ne!(root_after.etag, root_before.etag);
    assert_ne!(a_after.etag,    a_before.etag);
    assert_ne!(ab_after.etag,   ab_before.etag);
    assert_ne!(abc_after.etag,  abc_before.etag);
}

#[tokio::test]
async fn apply_dir_created_inserts_directory_with_zero_size() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::root(),
        metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(),
        path: StoragePath::new("d").unwrap(),
        metadata: make_dir_metadata("d"),
    }).await.unwrap();

    let row = cache.lookup(SID, &StoragePath::new("d").unwrap()).await.unwrap().unwrap();
    assert_eq!(row.size, 0);
    assert_eq!(row.mimetype.as_str(), "httpd/unix-directory");
}

#[tokio::test]
async fn apply_deleted_cascades_descendants_and_decrements_size() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());

    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::root(), metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::new("d").unwrap(), metadata: make_dir_metadata("d"),
    }).await.unwrap();
    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("d/inner.txt").unwrap(),
        metadata: make_metadata("d/inner.txt", 50, "text/plain"),
    }).await.unwrap();

    let root_pre = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();

    cache.apply(&StorageEvent::Deleted {
        storage_id: SID.into(),
        path: StoragePath::new("d").unwrap(),
    }).await.unwrap();

    assert!(cache.lookup(SID, &StoragePath::new("d").unwrap()).await.unwrap().is_none());
    assert!(cache.lookup(SID, &StoragePath::new("d/inner.txt").unwrap()).await.unwrap().is_none());

    let root_post = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();
    // Root size dropped by `d`'s size (which was 50 after the inner write).
    assert_eq!(root_post.size, root_pre.size - 50);
}

#[tokio::test]
async fn apply_moved_within_same_parent_bumps_etag_only() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::root(), metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("from.txt").unwrap(),
        metadata: make_metadata("from.txt", 10, "text/plain"),
    }).await.unwrap();

    let root_pre = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();

    cache.apply(&StorageEvent::Moved {
        storage_id: SID.into(),
        from: StoragePath::new("from.txt").unwrap(),
        to: StoragePath::new("to.txt").unwrap(),
    }).await.unwrap();

    assert!(cache.lookup(SID, &StoragePath::new("from.txt").unwrap()).await.unwrap().is_none());
    let to_row = cache.lookup(SID, &StoragePath::new("to.txt").unwrap()).await.unwrap().unwrap();
    assert_eq!(to_row.size, 10);

    let root_post = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();
    assert_eq!(root_post.size, root_pre.size); // same parent → no size change
    assert_ne!(root_post.etag, root_pre.etag); // but etag bumped
}

#[tokio::test]
async fn apply_moved_across_parents_shifts_size_and_bumps_both_etags() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::root(), metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::new("a").unwrap(), metadata: make_dir_metadata("a"),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::new("b").unwrap(), metadata: make_dir_metadata("b"),
    }).await.unwrap();
    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("a/file.txt").unwrap(),
        metadata: make_metadata("a/file.txt", 25, "text/plain"),
    }).await.unwrap();

    let a_pre = cache.lookup(SID, &StoragePath::new("a").unwrap()).await.unwrap().unwrap();
    let b_pre = cache.lookup(SID, &StoragePath::new("b").unwrap()).await.unwrap().unwrap();

    cache.apply(&StorageEvent::Moved {
        storage_id: SID.into(),
        from: StoragePath::new("a/file.txt").unwrap(),
        to: StoragePath::new("b/file.txt").unwrap(),
    }).await.unwrap();

    let a_post = cache.lookup(SID, &StoragePath::new("a").unwrap()).await.unwrap().unwrap();
    let b_post = cache.lookup(SID, &StoragePath::new("b").unwrap()).await.unwrap().unwrap();
    assert_eq!(a_post.size, a_pre.size - 25);
    assert_eq!(b_post.size, b_pre.size + 25);
    assert_ne!(a_post.etag, a_pre.etag);
    assert_ne!(b_post.etag, b_pre.etag);
}

#[tokio::test]
async fn apply_moved_directory_rewrites_descendant_paths() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::root(), metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::new("a").unwrap(), metadata: make_dir_metadata("a"),
    }).await.unwrap();
    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("a/inner.txt").unwrap(),
        metadata: make_metadata("a/inner.txt", 7, "text/plain"),
    }).await.unwrap();

    // Move directory "a" -> "b". Descendant path "a/inner.txt" should be
    // rewritten to "b/inner.txt".
    cache.apply(&StorageEvent::Moved {
        storage_id: SID.into(),
        from: StoragePath::new("a").unwrap(),
        to: StoragePath::new("b").unwrap(),
    }).await.unwrap();

    assert!(cache.lookup(SID, &StoragePath::new("a/inner.txt").unwrap()).await.unwrap().is_none());
    let inner = cache.lookup(SID, &StoragePath::new("b/inner.txt").unwrap()).await.unwrap().unwrap();
    assert_eq!(inner.name, "inner.txt");
    assert_eq!(inner.size, 7);
}

#[tokio::test]
async fn apply_copied_inserts_dest_with_fresh_etag() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    cache.apply(&StorageEvent::DirCreated {
        storage_id: SID.into(), path: StoragePath::root(), metadata: make_dir_metadata(""),
    }).await.unwrap();
    cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("src.txt").unwrap(),
        metadata: make_metadata("src.txt", 12, "text/plain"),
    }).await.unwrap();

    let src_pre = cache.lookup(SID, &StoragePath::new("src.txt").unwrap()).await.unwrap().unwrap();
    let root_pre = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();

    cache.apply(&StorageEvent::Copied {
        storage_id: SID.into(),
        from: StoragePath::new("src.txt").unwrap(),
        to: StoragePath::new("dst.txt").unwrap(),
    }).await.unwrap();

    let dst = cache.lookup(SID, &StoragePath::new("dst.txt").unwrap()).await.unwrap().unwrap();
    assert_eq!(dst.size, 12);
    assert_ne!(dst.etag, src_pre.etag);

    let root_post = cache.lookup(SID, &StoragePath::root()).await.unwrap().unwrap();
    assert_eq!(root_post.size, root_pre.size + 12);
    assert_ne!(root_post.etag, root_pre.etag);
}

#[tokio::test]
async fn apply_missing_ancestor_errors() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    // No root row + no "a" row — direct write to "a/file" must fail.
    let res = cache.apply(&StorageEvent::Written {
        storage_id: SID.into(),
        path: StoragePath::new("a/file.txt").unwrap(),
        metadata: make_metadata("a/file.txt", 5, "text/plain"),
    }).await;
    assert!(matches!(res, Err(FileCacheError::AncestorMissing(_))));
}
```

### Step 5: Run + commit + push + open Batch B PR

```
cargo test -p crabcloud-filecache
cargo xtask check-all
```

Expected: 9 new integration tests pass + previous unit tests still pass.

```
git add crates/crabcloud-filecache
git commit -m "feat(filecache): FileCache::apply + ancestor propagation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-b
gh pr create --base master --head filecache-batch-b \
  --title "filecache: batch B — apply event handlers + ancestor propagation" \
  --body "Sub-project 4b, batch B: FileCache facade + apply() for all 5 StorageEvent variants. Each handler runs leaf mutation + ancestor walk in one DB transaction. Directory moves rewrite all descendant paths. 9 integration tests cover the full event surface including missing-ancestor errors."
```

**STOP.**

---

## Task 3: Cache-miss populate (Batch C)

**Files:**
- Create: `crates/crabcloud-filecache/src/populate.rs`
- Modify: `crates/crabcloud-filecache/src/lib.rs` (add `stat`/`list`/`stamp_last_checked` + `populate_locks` field)
- Create: `crates/crabcloud-filecache/tests/populate.rs`

### Step 1: Branch

```
git checkout -b filecache-batch-c origin/master
```

### Step 2: Update `crates/crabcloud-filecache/src/lib.rs`

Add to the imports + add the field + add the three methods. Replace the lib.rs from Batch B:

```rust
//! `crabcloud-filecache` — DB-backed cache for storage state.

pub mod error;
pub mod mimetypes;
pub mod populate;
pub mod propagate;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

use crabcloud_db::DbPool;
use crabcloud_storage::{DirEntry, FileMetadata, Storage, StorageEvent, StoragePath};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct FileCache {
    pool: DbPool,
    pub(crate) storage_ids: DashMap<String, i64>,
    pub(crate) mimetypes: DashMap<String, i64>,
    pub(crate) populate_locks: DashMap<(String, StoragePath), Arc<Mutex<()>>>,
}

impl FileCache {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            storage_ids: DashMap::new(),
            mimetypes: DashMap::new(),
            populate_locks: DashMap::new(),
        }
    }

    pub(crate) fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Cached stat. On miss, calls `storage.stat(path)` under a per-path
    /// lock so concurrent callers for the same path produce one backend
    /// stat. Distinct paths populate in parallel.
    pub async fn stat(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<FileMetadata> {
        populate::stat(self, storage, path).await
    }

    /// Cached directory listing. On miss, populates the directory itself
    /// + every immediate child (one level). Returns the cache rows shaped
    /// as `DirEntry`.
    pub async fn list(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<Vec<DirEntry>> {
        populate::list(self, storage, path).await
    }

    pub async fn apply(&self, event: &StorageEvent) -> FileCacheResult<()> {
        propagate::apply_event(self, event).await
    }

    pub async fn lookup(
        &self,
        storage_id: &str,
        path: &StoragePath,
    ) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row(self, storage_id, path).await
    }

    pub async fn lookup_by_id(
        &self,
        fileid: i64,
    ) -> FileCacheResult<Option<FilecacheRow>> {
        propagate::lookup_row_by_id(self, fileid).await
    }

    /// Update `oc_storages.last_checked` for `storage_id`. Called by the
    /// scanner at the end of `full_scan`.
    pub async fn stamp_last_checked(&self, storage_id: &str) -> FileCacheResult<()> {
        storages::stamp_last_checked(&self.pool, storage_id).await
    }
}
```

### Step 3: Create `crates/crabcloud-filecache/src/populate.rs`

```rust
//! Cache-miss populate path. Per-`(storage_id, path)` mutex serializes
//! concurrent stat-on-miss for the same path; distinct paths run in
//! parallel.

use crabcloud_storage::{
    DirEntry, FileKind, FileMetadata, Mimetype, Permissions, Storage, StorageEvent,
    StoragePath, StorageError,
};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{FileCacheError, FileCacheResult};
use crate::FileCache;

pub async fn stat(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<FileMetadata> {
    // Fast path: cache hit.
    if let Some(row) = cache.lookup(storage.id(), path).await? {
        return Ok(row_to_metadata(row));
    }

    // Acquire per-path lock.
    let key = (storage.id().to_string(), path.clone());
    let lock = cache
        .populate_locks
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Re-check cache under the lock.
    if let Some(row) = cache.lookup(storage.id(), path).await? {
        drop(_guard);
        opportunistic_cleanup(cache, &key, &lock);
        return Ok(row_to_metadata(row));
    }

    // Backend stat. NotFound propagates as-is.
    let meta = match storage.stat(path).await {
        Ok(m) => m,
        Err(StorageError::NotFound) => {
            drop(_guard);
            opportunistic_cleanup(cache, &key, &lock);
            return Err(FileCacheError::NotFound);
        }
        Err(e) => {
            drop(_guard);
            opportunistic_cleanup(cache, &key, &lock);
            return Err(FileCacheError::Storage(e));
        }
    };

    // Recurse parent so its row exists before we INSERT.
    if let Some(parent) = path.parent() {
        if !parent.is_root() {
            stat(cache, storage, &parent).await?;
        } else {
            // Ensure root row exists. If the storage's root isn't cached
            // yet, populate it via a root stat. Storage trait guarantees
            // stat(root) returns kind=Directory.
            if cache.lookup(storage.id(), &parent).await?.is_none() {
                let root_meta = storage.stat(&parent).await?;
                cache
                    .apply(&StorageEvent::DirCreated {
                        storage_id: storage.id().to_string(),
                        path: parent.clone(),
                        metadata: root_meta,
                    })
                    .await?;
            }
        }
    }

    // Materialize the row through `apply` so it goes through the same
    // intern + propagation sequence as event-driven writes.
    let event = if matches!(meta.kind, FileKind::Directory) {
        StorageEvent::DirCreated {
            storage_id: storage.id().to_string(),
            path: path.clone(),
            metadata: meta.clone(),
        }
    } else {
        StorageEvent::Written {
            storage_id: storage.id().to_string(),
            path: path.clone(),
            metadata: meta.clone(),
        }
    };
    cache.apply(&event).await?;

    drop(_guard);
    opportunistic_cleanup(cache, &key, &lock);
    Ok(meta)
}

pub async fn list(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FileCacheResult<Vec<DirEntry>> {
    // Ensure directory is populated.
    let _ = stat(cache, storage, path).await?;
    // List from backend (we don't yet trust the cache as authoritative
    // for "all children present" — sub-project 5 can layer that on).
    let entries = storage.list(path).await?;
    // Populate each child.
    for child in &entries {
        let child_path = if path.is_root() {
            StoragePath::new(child.name.clone())?
        } else {
            path.join(&child.name)?
        };
        stat(cache, storage, &child_path).await?;
    }
    Ok(entries)
}

fn row_to_metadata(row: crate::schema::FilecacheRow) -> FileMetadata {
    use std::time::{Duration, UNIX_EPOCH};
    FileMetadata {
        path: row.path,
        kind: row.kind,
        size: row.size,
        mtime: UNIX_EPOCH + Duration::from_secs(row.mtime),
        etag: row.etag,
        mimetype: row.mimetype,
        permissions: row.permissions,
    }
}

fn opportunistic_cleanup(
    cache: &FileCache,
    key: &(String, StoragePath),
    lock: &Arc<Mutex<()>>,
) {
    // If we hold the only Arc (besides the DashMap's), remove the entry.
    // Racy but bounded — the next populate just re-creates an Arc.
    if Arc::strong_count(lock) <= 2 {
        // 2 = our local Arc + the one held by the DashMap entry.
        cache.populate_locks.remove(key);
    }
}
```

### Step 4: Create `crates/crabcloud-filecache/tests/populate.rs`

```rust
mod support;

use async_trait::async_trait;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{
    memory::MemoryStorage, DirEntry, EventSink, FileMetadata, MultipartHandle, NoopEventSink,
    PartTag, Storage, StorageError, StorageEvent, StoragePath, StorageResult,
};
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use support::harness;
use tokio::io::AsyncRead;

/// Storage wrapper that counts stat() calls.
struct CountingStorage {
    inner: Arc<dyn Storage>,
    stat_count: Arc<AtomicU32>,
}

#[async_trait]
impl Storage for CountingStorage {
    fn id(&self) -> &str { self.inner.id() }
    async fn stat(&self, p: &StoragePath) -> StorageResult<FileMetadata> {
        self.stat_count.fetch_add(1, Ordering::SeqCst);
        self.inner.stat(p).await
    }
    async fn exists(&self, p: &StoragePath) -> StorageResult<bool> { self.inner.exists(p).await }
    async fn list(&self, p: &StoragePath) -> StorageResult<Vec<DirEntry>> { self.inner.list(p).await }
    async fn read(&self, p: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> { self.inner.read(p).await }
    async fn read_range(&self, p: &StoragePath, r: Range<u64>) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> { self.inner.read_range(p, r).await }
    async fn put_file(&self, p: &StoragePath, b: Pin<Box<dyn AsyncRead + Send>>, s: &dyn EventSink) -> StorageResult<FileMetadata> { self.inner.put_file(p, b, s).await }
    async fn mkdir(&self, p: &StoragePath, s: &dyn EventSink) -> StorageResult<FileMetadata> { self.inner.mkdir(p, s).await }
    async fn delete(&self, p: &StoragePath, s: &dyn EventSink) -> StorageResult<()> { self.inner.delete(p, s).await }
    async fn rename(&self, f: &StoragePath, t: &StoragePath, s: &dyn EventSink) -> StorageResult<()> { self.inner.rename(f, t, s).await }
    async fn copy(&self, f: &StoragePath, t: &StoragePath, s: &dyn EventSink) -> StorageResult<()> { self.inner.copy(f, t, s).await }
    async fn begin_multipart(&self, t: &StoragePath, s: &dyn EventSink) -> StorageResult<MultipartHandle> { self.inner.begin_multipart(t, s).await }
    async fn put_part(&self, h: &MultipartHandle, n: u32, b: Pin<Box<dyn AsyncRead + Send>>) -> StorageResult<PartTag> { self.inner.put_part(h, n, b).await }
    async fn commit_multipart(&self, h: MultipartHandle, p: Vec<PartTag>, s: &dyn EventSink) -> StorageResult<FileMetadata> { self.inner.commit_multipart(h, p, s).await }
    async fn abort_multipart(&self, h: MultipartHandle) -> StorageResult<()> { self.inner.abort_multipart(h).await }
}

fn body(bytes: Vec<u8>) -> Pin<Box<dyn AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

async fn seed_one_file(storage: &Arc<dyn Storage>, path: &str, bytes: &[u8]) {
    storage.put_file(&StoragePath::new(path).unwrap(), body(bytes.to_vec()), &NoopEventSink).await.unwrap();
}

#[tokio::test]
async fn stat_cache_miss_populates_then_uses_cache() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate1"));
    seed_one_file(&inner, "hello.txt", b"hi").await;

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("hello.txt").unwrap();
    let _meta1 = cache.stat(&counting, &p).await.unwrap();
    let after_first = count.load(Ordering::SeqCst);
    let _meta2 = cache.stat(&counting, &p).await.unwrap();
    let after_second = count.load(Ordering::SeqCst);

    // First call may stat the leaf + the root; subsequent call should add 0.
    assert!(after_first >= 1);
    assert_eq!(after_first, after_second, "second stat should be cached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stat_cache_miss_concurrent_populates_once() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate2"));
    seed_one_file(&inner, "f.txt", b"x").await;

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("f.txt").unwrap();
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let cache = cache.clone();
        let counting = counting.clone();
        let p = p.clone();
        tasks.push(tokio::spawn(async move {
            cache.stat(&counting, &p).await.unwrap();
        }));
    }
    for t in tasks { t.await.unwrap(); }

    // Backend stat hit at most: 1 leaf + 1 root = 2 total. NOT 100.
    let n = count.load(Ordering::SeqCst);
    assert!(n <= 2, "expected <=2 backend stats (leaf + root), got {n}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stat_cache_miss_distinct_paths_run_in_parallel() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate3"));
    for i in 0..50 {
        seed_one_file(&inner, &format!("f-{i:03}.txt"), b"x").await;
    }

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let mut tasks = Vec::new();
    for i in 0..50 {
        let cache = cache.clone();
        let counting = counting.clone();
        tasks.push(tokio::spawn(async move {
            let p = StoragePath::new(format!("f-{i:03}.txt")).unwrap();
            cache.stat(&counting, &p).await.unwrap();
        }));
    }
    for t in tasks { t.await.unwrap(); }

    // 50 leaf stats + root stats (≤1 because of cache reuse on root).
    let n = count.load(Ordering::SeqCst);
    assert!(n >= 50, "expected at least 50 distinct backend stats, got {n}");
    assert!(n <= 60, "expected at most ~50 + a few root re-stats, got {n}");
}

#[tokio::test]
async fn stat_cache_miss_not_found_propagates_without_negative_caching() {
    let h = harness().await;
    let cache = FileCache::new(h.pool.clone());
    let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("populate4"));

    let count = Arc::new(AtomicU32::new(0));
    let counting: Arc<dyn Storage> = Arc::new(CountingStorage {
        inner: inner.clone(),
        stat_count: count.clone(),
    });

    let p = StoragePath::new("ghost.txt").unwrap();
    let r = cache.stat(&counting, &p).await;
    assert!(matches!(r, Err(crabcloud_filecache::FileCacheError::NotFound)));

    let r2 = cache.stat(&counting, &p).await;
    assert!(matches!(r2, Err(crabcloud_filecache::FileCacheError::NotFound)));

    // Both calls hit the backend (no negative caching).
    let n = count.load(Ordering::SeqCst);
    assert!(n >= 2, "expected at least 2 backend stats on repeat NotFound, got {n}");
}
```

### Step 5: Run + commit + push + open Batch C PR

```
cargo test -p crabcloud-filecache
cargo xtask check-all
```

Expected: 4 new populate tests pass; all earlier tests still pass.

```
git add crates/crabcloud-filecache
git commit -m "feat(filecache): cache-miss populate with per-path lock

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-c
gh pr create --base master --head filecache-batch-c \
  --title "filecache: batch C — cache-miss populate" \
  --body "Sub-project 4b, batch C: FileCache::stat/list populate on cache miss via per-path mutex serialization. 100 concurrent stats for the same path produce 1 backend stat; 50 distinct paths run in parallel. NotFound propagates without negative caching."
```

**STOP.**

---

## Task 4: ChannelEventSink + Scanner (Batch D)

**Files:**
- Modify: `crates/crabcloud-storage/src/lib.rs` (add `ChannelEventSink`)
- Create: `crates/crabcloud-filecache/src/scanner/mod.rs`
- Create: `crates/crabcloud-filecache/src/scanner/apply.rs`
- Create: `crates/crabcloud-filecache/src/scanner/full_scan.rs`
- Modify: `crates/crabcloud-filecache/src/lib.rs` (add `pub mod scanner;`)
- Create: `crates/crabcloud-filecache/tests/scanner.rs`

### Step 1: Branch

```
git checkout -b filecache-batch-d origin/master
```

### Step 2: Add `ChannelEventSink` to `crates/crabcloud-storage/src/lib.rs`

Find the `pub struct NoopEventSink;` declaration. After its `impl EventSink for NoopEventSink`, append:

```rust
/// Broadcast-channel-backed `EventSink`. Wraps `tokio::sync::broadcast`.
/// `emit` is non-blocking and best-effort (a send with zero receivers is
/// dropped silently). Consumers subscribe via [`ChannelEventSink::subscribe`].
pub struct ChannelEventSink {
    tx: tokio::sync::broadcast::Sender<StorageEvent>,
}

impl ChannelEventSink {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StorageEvent> {
        self.tx.subscribe()
    }
}

#[async_trait]
impl EventSink for ChannelEventSink {
    async fn emit(&self, event: StorageEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod channel_sink_tests {
    use super::*;

    #[tokio::test]
    async fn emit_with_subscriber_delivers() {
        let sink = ChannelEventSink::new(4);
        let mut rx = sink.subscribe();
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
        let got = rx.recv().await.unwrap();
        assert!(matches!(got, StorageEvent::Deleted { .. }));
    }

    #[tokio::test]
    async fn emit_without_subscriber_does_not_panic() {
        let sink = ChannelEventSink::new(4);
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
    }
}
```

Also ensure `StorageEvent` exposes a `storage_id()` accessor (added to `lib.rs`):

```rust
impl StorageEvent {
    /// Returns the `storage_id` of the event.
    pub fn storage_id(&self) -> &str {
        match self {
            StorageEvent::Written { storage_id, .. }
            | StorageEvent::DirCreated { storage_id, .. }
            | StorageEvent::Deleted { storage_id, .. }
            | StorageEvent::Moved { storage_id, .. }
            | StorageEvent::Copied { storage_id, .. } => storage_id,
        }
    }
}
```

### Step 3: Create `crates/crabcloud-filecache/src/scanner/mod.rs`

```rust
//! Scanner: continuous consumer of `ChannelEventSink` events + on-demand
//! full-scan + drift recovery via `RecvError::Lagged`.

pub mod apply;
pub mod full_scan;

use crabcloud_storage::{ChannelEventSink, Storage};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::FileCacheResult;
use crate::FileCache;

pub struct Scanner {
    cache: Arc<FileCache>,
    storages: DashMap<String, Arc<dyn Storage>>,
    sink: Arc<ChannelEventSink>,
}

impl Scanner {
    pub fn new(cache: Arc<FileCache>, sink: Arc<ChannelEventSink>) -> Self {
        Self {
            cache,
            storages: DashMap::new(),
            sink,
        }
    }

    pub fn register_storage(&self, storage: Arc<dyn Storage>) {
        self.storages.insert(storage.id().to_string(), storage);
    }

    pub fn storage_for(&self, id: &str) -> Option<Arc<dyn Storage>> {
        self.storages.get(id).map(|s| s.clone())
    }

    pub async fn full_scan(&self, storage: &Arc<dyn Storage>) -> FileCacheResult<u64> {
        full_scan::full_scan(&self.cache, storage).await
    }

    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = self.sink.subscribe();
            info!("scanner consumer started");
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Err(e) = self.cache.apply(&event).await {
                            warn!(?event, error = %e, "filecache apply failed; scheduling re-scan");
                            if let Some(storage) = self.storages.get(event.storage_id()) {
                                let _ = full_scan::full_scan(&self.cache, &storage).await;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "scanner lagged; full-scanning all storages");
                        for entry in self.storages.iter() {
                            let _ = full_scan::full_scan(&self.cache, entry.value()).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("scanner channel closed; consumer exiting");
                        return;
                    }
                }
            }
        })
    }
}
```

### Step 4: Create `crates/crabcloud-filecache/src/scanner/apply.rs`

Empty for now (the apply logic lives in `propagate.rs`; this module exists for future per-event customization):

```rust
//! Scanner-side event mutation hooks. Currently a placeholder — all
//! per-event logic lives in `crate::propagate::apply_event`. Reserved for
//! future scanner-specific instrumentation (metrics, dlq, etc.).
```

### Step 5: Create `crates/crabcloud-filecache/src/scanner/full_scan.rs`

```rust
//! BFS walk of a storage; populates every cache row top-down.

use crabcloud_storage::{FileKind, Storage, StoragePath};
use std::collections::VecDeque;
use std::sync::Arc;

use crate::error::FileCacheResult;
use crate::FileCache;

pub async fn full_scan(
    cache: &FileCache,
    storage: &Arc<dyn Storage>,
) -> FileCacheResult<u64> {
    let mut queue: VecDeque<StoragePath> = VecDeque::new();
    queue.push_back(StoragePath::root());
    let mut count = 0u64;

    while let Some(path) = queue.pop_front() {
        // Populate this row through the cache's stat path.
        let _ = cache.stat(storage, &path).await?;
        count += 1;

        let meta = storage.stat(&path).await?;
        if matches!(meta.kind, FileKind::Directory) {
            let children = storage.list(&path).await?;
            for child in children {
                let child_path = if path.is_root() {
                    StoragePath::new(child.name.clone())?
                } else {
                    path.join(&child.name)?
                };
                queue.push_back(child_path);
            }
        }
    }

    cache.stamp_last_checked(storage.id()).await?;
    Ok(count)
}
```

### Step 6: Wire `scanner` into `crabcloud-filecache/src/lib.rs`

Add `pub mod scanner;` to the module list:

```rust
pub mod error;
pub mod mimetypes;
pub mod populate;
pub mod propagate;
pub mod scanner;
pub mod schema;
pub mod storages;

pub use scanner::Scanner;
```

### Step 7: Create `crates/crabcloud-filecache/tests/scanner.rs`

```rust
mod support;

use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_storage::{
    memory::MemoryStorage, ChannelEventSink, EventSink, NoopEventSink, Storage, StoragePath,
};
use std::sync::Arc;
use std::time::Duration;
use support::harness;
use tokio::io::AsyncRead;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_consumes_written_events_into_cache() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner1"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Seed the root row in the cache so the scanner's apply doesn't fail
    // with AncestorMissing.
    let root_meta = storage.stat(&StoragePath::root()).await.unwrap();
    cache.apply(&crabcloud_storage::StorageEvent::DirCreated {
        storage_id: storage.id().to_string(),
        path: StoragePath::root(),
        metadata: root_meta,
    }).await.unwrap();

    // Spawn the consumer.
    let _handle = scanner.clone().spawn();

    // Now emit a real write through the sink-bound storage.
    storage.put_file(&StoragePath::new("scanned.txt").unwrap(), body(b"hello".to_vec()), &*sink).await.unwrap();

    // Wait for the scanner to catch up. Poll-loop the cache.
    let mut attempts = 0;
    loop {
        let row = cache.lookup(storage.id(), &StoragePath::new("scanned.txt").unwrap()).await.unwrap();
        if row.is_some() { break; }
        attempts += 1;
        if attempts > 50 { panic!("scanner didn't catch up in time"); }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_full_scan_reconciles_external_writes() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(64));
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner2"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Write files DIRECTLY through the storage (no sink), then full-scan.
    storage.put_file(&StoragePath::new("a.txt").unwrap(), body(b"x".to_vec()), &NoopEventSink).await.unwrap();
    storage.put_file(&StoragePath::new("b.txt").unwrap(), body(b"y".to_vec()), &NoopEventSink).await.unwrap();
    storage.put_file(&StoragePath::new("c.txt").unwrap(), body(b"z".to_vec()), &NoopEventSink).await.unwrap();

    let count = scanner.full_scan(&storage).await.unwrap();
    assert!(count >= 4, "expected at least 4 entries (root + 3 files), got {count}");
    assert!(cache.lookup(storage.id(), &StoragePath::new("a.txt").unwrap()).await.unwrap().is_some());
    assert!(cache.lookup(storage.id(), &StoragePath::new("b.txt").unwrap()).await.unwrap().is_some());
    assert!(cache.lookup(storage.id(), &StoragePath::new("c.txt").unwrap()).await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scanner_lagged_triggers_full_scan_recovery() {
    let h = harness().await;
    let cache = Arc::new(FileCache::new(h.pool.clone()));
    let sink = Arc::new(ChannelEventSink::new(4)); // tiny capacity
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("scanner3"));
    let scanner = Arc::new(Scanner::new(cache.clone(), sink.clone()));
    scanner.register_storage(storage.clone());

    // Seed root.
    let root_meta = storage.stat(&StoragePath::root()).await.unwrap();
    cache.apply(&crabcloud_storage::StorageEvent::DirCreated {
        storage_id: storage.id().to_string(),
        path: StoragePath::root(),
        metadata: root_meta,
    }).await.unwrap();

    // Don't subscribe; spawn the scanner; quickly emit more events than
    // capacity so the consumer Lags on receive.
    let _handle = scanner.clone().spawn();
    // Give the consumer time to subscribe.
    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0..20u32 {
        storage.put_file(
            &StoragePath::new(format!("f{i:02}.txt")).unwrap(),
            body(vec![b'x'; 1]),
            &*sink,
        ).await.unwrap();
    }

    // Wait for the scanner to catch up via full-scan.
    let mut attempts = 0;
    loop {
        let row = cache.lookup(storage.id(), &StoragePath::new("f19.txt").unwrap()).await.unwrap();
        if row.is_some() { break; }
        attempts += 1;
        if attempts > 100 { panic!("scanner didn't recover from lag"); }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
```

### Step 8: Run + commit + push + open Batch D PR

```
cargo test -p crabcloud-storage
cargo test -p crabcloud-filecache
cargo xtask check-all
```

Expected: ChannelEventSink unit tests pass in `crabcloud-storage`; 3 new scanner integration tests pass.

```
git add crates/crabcloud-storage crates/crabcloud-filecache
git commit -m "feat(filecache,storage): ChannelEventSink + Scanner

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-d
gh pr create --base master --head filecache-batch-d \
  --title "filecache: batch D — ChannelEventSink + Scanner" \
  --body "Sub-project 4b, batch D: ChannelEventSink in crabcloud-storage (broadcast::Sender wrapper). Scanner with continuous consumer + full_scan + RecvError::Lagged recovery. 3 integration tests cover live event consumption, drift reconciliation via full-scan, and lag recovery."
```

**STOP.**

---

## Task 5: AppState wiring + config + CLI (Batch E)

**Files:**
- Modify: `crates/crabcloud-config/src/lib.rs` (add `FilecacheConfig`)
- Modify: `crates/crabcloud-core/src/state.rs` (extend `AppState` + `AppStateBuilder::build`)
- Modify: `crates/crabcloud-core/Cargo.toml` (add `crabcloud-filecache` workspace dep)
- Modify: `Cargo.toml` workspace (add `crabcloud-filecache` to `[workspace.dependencies]`)
- Modify: `crates/crabcloud-server/src/cli.rs` (or equivalent — add `files:scan` subcommand)

### Step 1: Branch

```
git checkout -b filecache-batch-e origin/master
```

### Step 2: Add `crabcloud-filecache` as a workspace dep

In root `Cargo.toml` `[workspace.dependencies]`, add:

```toml
crabcloud-filecache = { path = "crates/crabcloud-filecache" }
```

(Match the style of other `crabcloud-*` entries.)

### Step 3: `FilecacheConfig` in `crates/crabcloud-config/src/lib.rs`

Find the `Config` struct definition. Add a new field:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilecacheConfig {
    #[serde(default = "default_filecache_enabled")]
    pub enabled: bool,
    #[serde(default = "default_event_channel_capacity")]
    pub event_channel_capacity: usize,
}

fn default_filecache_enabled() -> bool { true }
fn default_event_channel_capacity() -> usize { 1024 }

impl Default for FilecacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_filecache_enabled(),
            event_channel_capacity: default_event_channel_capacity(),
        }
    }
}
```

Then on the main `Config` struct, add:

```rust
#[serde(default)]
pub filecache: FilecacheConfig,
```

In `test_support::minimal_sqlite_config`, ensure `filecache: FilecacheConfig::default()` is set on the constructed `Config` so existing tests still work.

### Step 4: Extend `AppState` in `crates/crabcloud-core/src/state.rs`

Find the `AppState` struct. Add three fields:

```rust
pub storage_sink: Arc<crabcloud_storage::ChannelEventSink>,
pub filecache: Arc<crabcloud_filecache::FileCache>,
pub scanner: Arc<crabcloud_filecache::Scanner>,
```

In `AppStateBuilder::build`, before the existing `let state = AppState { ... };` construction, add:

```rust
let storage_sink = Arc::new(crabcloud_storage::ChannelEventSink::new(
    self.config.filecache.event_channel_capacity,
));
let filecache = Arc::new(crabcloud_filecache::FileCache::new(pool.clone()));
let scanner = Arc::new(crabcloud_filecache::Scanner::new(
    filecache.clone(),
    storage_sink.clone(),
));
if self.config.filecache.enabled {
    scanner.clone().spawn();
}
```

…and add the three fields into the `AppState { ... }` literal.

Update the existing `crabcloud-core` `Cargo.toml`: add `crabcloud-filecache.workspace = true` (this dependency is new for batch E).

### Step 5: Add `files:scan` CLI subcommand in `crabcloud-server`

Find the existing clap setup in `crabcloud-server/src/cli.rs` (or wherever the binary's argument parsing lives — grep for `clap::Parser` or `Subcommand`).

Add a new `Files` subcommand group with a `Scan` variant:

```rust
#[derive(clap::Subcommand)]
pub enum Command {
    // … existing variants …

    /// File-cache scanner commands.
    #[command(subcommand)]
    Files(FilesCmd),
}

#[derive(clap::Subcommand)]
pub enum FilesCmd {
    /// Walk a registered storage from root, reconciling cache state.
    Scan { storage_id: String },
}
```

In the dispatch:

```rust
match cmd {
    // … existing arms …
    Command::Files(FilesCmd::Scan { storage_id }) => {
        let state = AppStateBuilder::new(config).build().await?;
        let storage = state
            .scanner
            .storage_for(&storage_id)
            .ok_or_else(|| anyhow::anyhow!("unknown storage_id: {storage_id}"))?;
        let count = state.scanner.full_scan(&storage).await?;
        tracing::info!(count, "files:scan complete");
        println!("scanned {count} entries for storage '{storage_id}'");
    }
}
```

Adapt to the actual binary structure (the exact subcommand wiring may differ — find the closest existing pattern and match it).

### Step 6: Run + commit + push + open Batch E PR

```
cargo test -p crabcloud-config -p crabcloud-core -p crabcloud-server -p crabcloud-filecache
cargo xtask check-all
```

Expected: existing tests still pass; CLI subcommand is reachable but not yet exercised by an integration test (4c's mount/View will pass a registered storage; the smoke test is invoking `crabcloud files:scan` with an unknown id → clean error).

```
git add Cargo.toml crates/crabcloud-config crates/crabcloud-core crates/crabcloud-server
git commit -m "feat(filecache): AppState wiring + [filecache] config + files:scan CLI

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-e
gh pr create --base master --head filecache-batch-e \
  --title "filecache: batch E — AppState + config + CLI" \
  --body "Sub-project 4b, batch E: storage_sink/filecache/scanner fields on AppState. [filecache] block in crabcloud-config (enabled, event_channel_capacity defaults). \`crabcloud files:scan <storage_id>\` CLI subcommand resolves a registered storage and runs full_scan."
```

**STOP.**

---

## Task 6: Acceptance docs (Batch F)

**Files:**
- Create: `docs/superpowers/plans/2026-05-12-filecache-and-scanner-implementation.changelog.md`
- Create: `docs/superpowers/specs/2026-05-12-filecache-and-scanner-design.followup-4b-s3.md`
- Modify: `README.md`

### Step 1: Branch

```
git checkout -b filecache-batch-f origin/master
```

### Step 2: Write the changelog

Create `docs/superpowers/plans/2026-05-12-filecache-and-scanner-implementation.changelog.md`:

```markdown
# Sub-project 4b — Changelog

Completed: 2026-05-12

## What works

- New `crabcloud-filecache` crate (DB-backed cache + scanner).
- Migration `0004_filecache` creates `oc_storages` + `oc_mimetypes` + `oc_filecache` on sqlite/mysql/postgres (3 tables, 5 indexes, 4 FKs).
- `ChannelEventSink` in `crabcloud-storage` wraps `tokio::sync::broadcast` (default capacity 1024).
- `FileCache::apply` handles `Written`/`DirCreated`/`Deleted`/`Moved`/`Copied`. Each handler runs leaf mutation + ancestor walk in one DB transaction. Directory moves rewrite all descendant paths.
- Cache-miss `stat`/`list` populate through real-backend stat under per-path mutex: 100 concurrent stats for one path → 1 backend hit; distinct paths parallelize.
- `Scanner` continuous consumer applies events; `full_scan` walks a storage top-down for drift recovery; `RecvError::Lagged` triggers full-scan of every registered storage.
- `files:scan <storage_id>` CLI subcommand in `crabcloud-server`.
- `[filecache] enabled = true, event_channel_capacity = 1024` block in `crabcloud-config`.
- `AppState` gains `storage_sink`/`filecache`/`scanner` fields; `AppStateBuilder` spawns the scanner when `enabled = true`.

## What's deferred

- **S3 backend** — sub-project **4b-S3** (separate brainstorming). Prep notes at `docs/superpowers/specs/2026-05-12-filecache-and-scanner-design.followup-4b-s3.md`.
- **Mount composition / View layer** — sub-project **4c**.
- **Chunked-upload protocol translation** — sub-project **4c**.
- **WebDAV / HTTP routes** — sub-project **5**.
- **Trash, versions, WebDAV LOCK/UNLOCK** — separate later sub-projects.
- **Server-side encryption hooks** — separate later sub-project.
- **Sharing-aware permissions composition** — 4c + sharing sub-project.
- **Negative caching** — 4b doesn't remember NotFound results.
- **Parallel apply** — single-consumer; events apply in order.
- **`oc_filecache.parent` integrity audit** — there's no scrubber to detect orphan rows (parent points to a non-existent fileid).

## Known limitations

- **`oc_filecache.path` is capped at 4000 chars** (MySQL/Postgres VARCHAR limit before index-width concerns). `StoragePath::new` caps at 4096; gap is 96 chars. Operators with deeper paths must wait for a future VARCHAR widening or switch to TEXT-typed columns.
- **External edits between scans** are not visible until next `files:scan`. Documented for operators.
- **Migration version is 4** (not 3 as the spec said — `0003_auth_tokens` already exists on master).
- **Cross-storage moves** require `Storage::rename` to be on the same storage; 4c's View layer will add cross-storage copy+delete.
- **Per-path lock map** grows monotonically with opportunistic cleanup; bounded eviction is a future hardening.
- **Test suites use SQLite-only fixtures**; multi-dialect coverage runs in CI via `cargo xtask check-all`.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean (sqlite/mysql/postgres) | OK (CI) |
| 2 | Migration `0004_filecache` creates 3 tables + 5 indexes + FKs | OK (`tests/apply_events.rs` + `core_migration_applies_against_sqlite`) |
| 3 | `ChannelEventSink` is `EventSink`; capacity 1024 default | OK (`crates/crabcloud-storage/src/lib.rs::channel_sink_tests`) |
| 4 | Written event inserts leaf with correct mimetype/size/etag/permissions | OK (`tests/apply_events.rs::apply_written_event_inserts_leaf_with_metadata`) |
| 5 | Ancestor size + etag propagation atomic | OK (`tests/apply_events.rs::apply_propagates_size_and_etag_up_chain`) |
| 6 | Cache-miss populate serializes per-path (100 → 1 backend hit) | OK (`tests/populate.rs::stat_cache_miss_concurrent_populates_once`) |
| 7 | Cache-miss populate parallelizes across paths | OK (`tests/populate.rs::stat_cache_miss_distinct_paths_run_in_parallel`) |
| 8 | Scanner consumes broadcast events | OK (`tests/scanner.rs::scanner_consumes_written_events_into_cache`) |
| 9 | Full-scan reconciles external drift | OK (`tests/scanner.rs::scanner_full_scan_reconciles_external_writes`) |
| 10 | `RecvError::Lagged` triggers full-scan recovery | OK (`tests/scanner.rs::scanner_lagged_triggers_full_scan_recovery`) |
| 11 | `files:scan` CLI runs full-scan | OK (smoke + Batch E wiring) |
| 12 | Deleted directory cascades descendants via FK | OK (`tests/apply_events.rs::apply_deleted_cascades_descendants_and_decrements_size`) |
| 13 | Moved row updates fields + descendant paths + propagates ETag both chains | OK (`tests/apply_events.rs::apply_moved_directory_rewrites_descendant_paths` + `apply_moved_across_parents_shifts_size_and_bumps_both_etags`) |
| 14 | Workspace `-D warnings` clean | OK (CI) |
| 15 | `git grep -i rustcloud` empty | OK |
```

### Step 3: Write the 4b-S3 prep notes

Create `docs/superpowers/specs/2026-05-12-filecache-and-scanner-design.followup-4b-s3.md`:

```markdown
# Sub-project 4b-S3 prep — S3 backend

Notes captured during 4b implementation that should inform the 4b-S3 spec when we brainstorm it. **These are prep notes, not a spec** — the actual 4b-S3 spec will be authored via the brainstorming skill before implementation begins.

## Scope sketch

Add `S3Storage` to `crabcloud-storage`. Plug it into the existing `Storage` trait without changes (4a's trait was designed for this). Multipart semantics map cleanly to S3's UploadPart/CompleteMultipartUpload. `Scanner::register_storage` accepts the new backend without modification.

## Crate + dep choices

Use `aws-sdk-s3` (official, async, multipart-first). Workspace deps to add:

- `aws-sdk-s3 = "1"`
- `aws-config = "1"`
- `aws-credential-types = "1"`

S3 backend lives in `crabcloud-storage/src/s3/` alongside `local/` and `memory/`.

## Operation mapping

| Storage method | S3 op |
|---|---|
| `id()` | `format!("s3::{bucket}/{prefix}")` |
| `stat(path)` | `HeadObject` (or `ListObjectsV2` with prefix for directories) |
| `exists(path)` | `HeadObject` with 404 catch |
| `list(path)` | `ListObjectsV2` with `Delimiter=/` |
| `read(path)` | `GetObject` |
| `read_range(path, range)` | `GetObject` with `Range` header |
| `put_file(path, body)` | `PutObject` for small bodies; `CreateMultipartUpload` + chunked PUT for large |
| `mkdir(path)` | `PutObject` with `Key=prefix/` and empty body (S3 directory convention) |
| `delete(path)` | `DeleteObject` (single) or `DeleteObjects` (batched for directories) |
| `rename(from, to)` | `CopyObject` + `DeleteObject` (S3 has no native rename) |
| `copy(from, to)` | `CopyObject` |
| `begin_multipart(target)` | `CreateMultipartUpload`; `upload_id` ← S3's UploadId |
| `put_part(handle, n, body)` | `UploadPart`; `PartTag.etag` ← S3 part ETag |
| `commit_multipart(handle, parts)` | `CompleteMultipartUpload` |
| `abort_multipart(handle)` | `AbortMultipartUpload` |

## ETag normalization

S3 returns ETags as quoted strings, sometimes with a `-<part_count>` suffix for multipart uploads. The 4a contract expects 40-char lowercase hex. Two options:

1. **Mint a synthetic ETag** (random hex via `ETag::new()`) on every PUT/multipart commit. Stash it in object metadata (`x-amz-meta-crabcloud-etag`). Reads pull the metadata. Decouples our ETag from S3's. Cost: one extra metadata read per stat.
2. **Use S3's ETag as-is** (strip quotes; hash-md5-strip-suffix transformation). Cheaper but couples our cache to S3's hashing.

Recommend option 1 — matches LocalStorage's xattr persistence pattern.

## Mimetype + permissions

- Mimetype on PUT: set `ContentType` from the same detection logic LocalStorage uses (extension table + magic-byte sniff). Stat reads `ContentType`.
- Permissions: bucket-policy-mediated, not per-object. Map to `Permissions::full()` for owned objects. Future: integrate with S3 bucket ACLs.

## Directories

S3 has no real directories. Two patterns coexist in the wild:

- **Empty-object marker** (`prefix/`): explicit; visible via `ListObjectsV2`.
- **Prefix-only**: derived from the existence of child objects.

Recommend the empty-object marker for parity with how filecache rows track directories; emit one on `mkdir`. `list(prefix)` then sees both real markers and synthetic derivations via the `CommonPrefixes` response field.

## Drift recovery

S3 console writes won't fire `StorageEvent`s. `Scanner::full_scan` is the only reconciler. Operators should be told to `crabcloud files:scan s3::<bucket>/<prefix>` after out-of-band uploads.

## Open questions for 4b-S3 brainstorming

- **Region + credentials config:** static config block, env vars, or AWS SDK default chain (recommended)?
- **Object size limits:** S3 supports 5 TiB objects; multipart-part minimum is 5 MiB. Do we enforce the 5 MiB minimum in the storage trait, or push it to the backend?
- **Presigned URLs:** offer a method for clients to upload directly to S3 without proxying through our server? Bypasses the event sink — the scanner reconciles via `files:scan`.
- **`ListObjectsV2` pagination:** 1000-objects-per-call default. `list()` should paginate transparently.
- **Eventual consistency:** read-after-write is now strong on S3, but cross-region consistency lags. Document the assumption.
- **Test strategy:** use `minio` testcontainer or LocalStack? Both work; minio is simpler.
```

### Step 4: Update README.md

Read `README.md` and find the workspace-layout block where `crabcloud-filecache` should appear in alphabetical order. Insert between `-db` and `-http`:

```
crates/crabcloud-filecache         DB-backed file cache + scanner consuming Storage events
```

(Match the bullet style of the existing entries — likely a dash + backtick + description.)

### Step 5: Run + commit + push + open Batch F PR

```
cargo xtask check-all
git add docs/superpowers README.md
git commit -m "docs(filecache): sub-project 4b acceptance — changelog + README + 4b-S3 prep notes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin filecache-batch-f
gh pr create --base master --head filecache-batch-f \
  --title "filecache: batch F — sub-project 4b acceptance docs" \
  --body "Sub-project 4b final batch: changelog (15-row acceptance table), README workspace-layout bullet for crabcloud-filecache, prep notes for the eventual 4b-S3 brainstorming session (operation mapping, ETag normalization, directory conventions, drift recovery, open questions)."
```

**STOP.**

---

## Final acceptance

After all 6 PRs land:

1. `git pull --ff-only origin master`.
2. `cargo xtask check-all` green.
3. Update program memory: mark 4b complete, point to 4b-S3 prep notes.
4. Brainstorm 4b-S3 (S3 backend) when ready.

## Open questions deferred

- See changelog "What's deferred" + "Known limitations".
- See `4b-S3` prep doc for backend-design decisions.
- See spec §16 for cache-strategy open questions (negative caching, scanner parallelism, etc.).
