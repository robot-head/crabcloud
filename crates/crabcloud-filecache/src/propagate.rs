//! Single transactional apply path for each `StorageEvent` variant. Walks
//! the ancestor chain, propagates size delta + fresh ETag, commits.
//!
//! Per-dialect helpers (`upsert_leaf_*`, `propagate_ancestors_*`,
//! `rewrite_descendant_paths_*`) are required because `sqlx::Transaction<'_, T>`
//! is generic in `T` (the dialect type) and Rust async functions can't return
//! an existentially-quantified transaction handle. So we duplicate per
//! `Sqlite`, `MySql`, `Postgres`.

use crabcloud_db::DbPool;
use crabcloud_storage::{ETag, FileMetadata, StorageEvent, StoragePath};
use sqlx::Row as _;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{FileCacheError, FileCacheResult};
use crate::mimetypes::intern_mimetype;
use crate::schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw};
use crate::storages::intern_storage;
use crate::FileCache;

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
        StorageEvent::Deleted { storage_id, path } => apply_deleted(cache, storage_id, path).await,
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

// --- lookup helpers ---

/// Lookup `(storage_id, path)` -> row.
pub async fn lookup_row(
    cache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
) -> FileCacheResult<Option<FilecacheRow>> {
    let ph = path_hash(path);
    let raw = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query(SQL_SELECT_BY_STORAGE_PATH_QM)
            .bind(storage_id)
            .bind(&ph)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_sqlite_row)
            .transpose()?,
        DbPool::MySql(p) => sqlx::query(SQL_SELECT_BY_STORAGE_PATH_QM)
            .bind(storage_id)
            .bind(&ph)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_mysql_row)
            .transpose()?,
        DbPool::Postgres(p) => sqlx::query(SQL_SELECT_BY_STORAGE_PATH_PG)
            .bind(storage_id)
            .bind(&ph)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_postgres_row)
            .transpose()?,
    };
    raw.map(FilecacheRowRaw::into_row).transpose()
}

/// Lookup by `fileid`.
pub async fn lookup_row_by_id(
    cache: &FileCache,
    fileid: i64,
) -> FileCacheResult<Option<FilecacheRow>> {
    let raw = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query(SQL_SELECT_BY_FILEID_QM)
            .bind(fileid)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_sqlite_row)
            .transpose()?,
        DbPool::MySql(p) => sqlx::query(SQL_SELECT_BY_FILEID_QM)
            .bind(fileid as u64)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_mysql_row)
            .transpose()?,
        DbPool::Postgres(p) => sqlx::query(SQL_SELECT_BY_FILEID_PG)
            .bind(fileid)
            .fetch_optional(p)
            .await
            .map_err(FileCacheError::Db)?
            .map(decode_postgres_row)
            .transpose()?,
    };
    raw.map(FilecacheRowRaw::into_row).transpose()
}

const SQL_SELECT_COLUMNS: &str = "f.fileid, s.id AS storage_id, f.path, f.parent, f.name, \
    m.mimetype AS mimetype, f.size, f.mtime, f.storage_mtime, f.etag, f.permissions";

const SQL_SELECT_BY_STORAGE_PATH_QM: &str = "SELECT f.fileid, s.id AS storage_id, f.path, \
    f.parent, f.name, m.mimetype AS mimetype, f.size, f.mtime, f.storage_mtime, f.etag, \
    f.permissions \
    FROM oc_filecache f \
    JOIN oc_storages s ON s.numeric_id = f.storage \
    JOIN oc_mimetypes m ON m.id = f.mimetype \
    WHERE s.id = ? AND f.path_hash = ?";

const SQL_SELECT_BY_STORAGE_PATH_PG: &str = "SELECT f.fileid, s.id AS storage_id, f.path, \
    f.parent, f.name, m.mimetype AS mimetype, f.size, f.mtime, f.storage_mtime, f.etag, \
    f.permissions \
    FROM oc_filecache f \
    JOIN oc_storages s ON s.numeric_id = f.storage \
    JOIN oc_mimetypes m ON m.id = f.mimetype \
    WHERE s.id = $1 AND f.path_hash = $2";

const SQL_SELECT_BY_FILEID_QM: &str = "SELECT f.fileid, s.id AS storage_id, f.path, \
    f.parent, f.name, m.mimetype AS mimetype, f.size, f.mtime, f.storage_mtime, f.etag, \
    f.permissions \
    FROM oc_filecache f \
    JOIN oc_storages s ON s.numeric_id = f.storage \
    JOIN oc_mimetypes m ON m.id = f.mimetype \
    WHERE f.fileid = ?";

const SQL_SELECT_BY_FILEID_PG: &str = "SELECT f.fileid, s.id AS storage_id, f.path, \
    f.parent, f.name, m.mimetype AS mimetype, f.size, f.mtime, f.storage_mtime, f.etag, \
    f.permissions \
    FROM oc_filecache f \
    JOIN oc_storages s ON s.numeric_id = f.storage \
    JOIN oc_mimetypes m ON m.id = f.mimetype \
    WHERE f.fileid = $1";

// Reference the constant so unused-const warnings stay quiet across batches.
const _: &str = SQL_SELECT_COLUMNS;

fn decode_sqlite_row(row: sqlx::sqlite::SqliteRow) -> FileCacheResult<FilecacheRowRaw> {
    Ok(FilecacheRowRaw {
        fileid: row
            .try_get::<i64, _>("fileid")
            .map_err(FileCacheError::Db)?,
        storage_id: row
            .try_get::<String, _>("storage_id")
            .map_err(FileCacheError::Db)?,
        path: row
            .try_get::<String, _>("path")
            .map_err(FileCacheError::Db)?,
        parent: row
            .try_get::<Option<i64>, _>("parent")
            .map_err(FileCacheError::Db)?,
        name: row
            .try_get::<String, _>("name")
            .map_err(FileCacheError::Db)?,
        mimetype: row
            .try_get::<String, _>("mimetype")
            .map_err(FileCacheError::Db)?,
        size: row.try_get::<i64, _>("size").map_err(FileCacheError::Db)?,
        mtime: row.try_get::<i64, _>("mtime").map_err(FileCacheError::Db)?,
        storage_mtime: row
            .try_get::<i64, _>("storage_mtime")
            .map_err(FileCacheError::Db)?,
        etag: row
            .try_get::<String, _>("etag")
            .map_err(FileCacheError::Db)?,
        permissions: row
            .try_get::<i64, _>("permissions")
            .map_err(FileCacheError::Db)?,
    })
}

fn decode_mysql_row(row: sqlx::mysql::MySqlRow) -> FileCacheResult<FilecacheRowRaw> {
    // The migrated mysql tables use `COLLATE=utf8mb4_bin`, which the wire
    // protocol surfaces as VARBINARY instead of VARCHAR. `try_get_unchecked`
    // bypasses the sqlx type check and decodes from raw bytes via the
    // `MySqlValue` impl — which for `String` is `String::from_utf8_lossy`.
    // No data-loss risk: storage IDs, paths, and names are written by the
    // application and constrained to valid UTF-8 by `StoragePath::new`.
    Ok(FilecacheRowRaw {
        fileid: row
            .try_get::<u64, _>("fileid")
            .map_err(FileCacheError::Db)? as i64,
        storage_id: row
            .try_get_unchecked::<String, _>("storage_id")
            .map_err(FileCacheError::Db)?,
        path: row
            .try_get_unchecked::<String, _>("path")
            .map_err(FileCacheError::Db)?,
        parent: row
            .try_get::<Option<u64>, _>("parent")
            .map_err(FileCacheError::Db)?
            .map(|v| v as i64),
        name: row
            .try_get_unchecked::<String, _>("name")
            .map_err(FileCacheError::Db)?,
        mimetype: row
            .try_get_unchecked::<String, _>("mimetype")
            .map_err(FileCacheError::Db)?,
        size: row.try_get::<i64, _>("size").map_err(FileCacheError::Db)?,
        mtime: row.try_get::<u32, _>("mtime").map_err(FileCacheError::Db)? as i64,
        storage_mtime: row
            .try_get::<u32, _>("storage_mtime")
            .map_err(FileCacheError::Db)? as i64,
        etag: row
            .try_get_unchecked::<String, _>("etag")
            .map_err(FileCacheError::Db)?,
        permissions: row
            .try_get::<u32, _>("permissions")
            .map_err(FileCacheError::Db)? as i64,
    })
}

fn decode_postgres_row(row: sqlx::postgres::PgRow) -> FileCacheResult<FilecacheRowRaw> {
    Ok(FilecacheRowRaw {
        fileid: row
            .try_get::<i64, _>("fileid")
            .map_err(FileCacheError::Db)?,
        storage_id: row
            .try_get::<String, _>("storage_id")
            .map_err(FileCacheError::Db)?,
        path: row
            .try_get::<String, _>("path")
            .map_err(FileCacheError::Db)?,
        parent: row
            .try_get::<Option<i64>, _>("parent")
            .map_err(FileCacheError::Db)?,
        name: row
            .try_get::<String, _>("name")
            .map_err(FileCacheError::Db)?,
        mimetype: row
            .try_get::<String, _>("mimetype")
            .map_err(FileCacheError::Db)?,
        size: row.try_get::<i64, _>("size").map_err(FileCacheError::Db)?,
        mtime: row.try_get::<i32, _>("mtime").map_err(FileCacheError::Db)? as i64,
        storage_mtime: row
            .try_get::<i32, _>("storage_mtime")
            .map_err(FileCacheError::Db)? as i64,
        etag: row
            .try_get::<String, _>("etag")
            .map_err(FileCacheError::Db)?,
        permissions: row
            .try_get::<i32, _>("permissions")
            .map_err(FileCacheError::Db)? as i64,
    })
}

// --- event handlers ---

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
        crate::schema::DIRECTORY_MIMETYPE.to_string()
    } else {
        metadata.mimetype.as_str().to_string()
    };
    let mimepart_str = type_half(&mimetype_str).to_string();
    let mimetype_pk = intern_mimetype(cache.pool(), &cache.mimetypes, &mimetype_str).await?;
    let mimepart_pk = intern_mimetype(cache.pool(), &cache.mimetypes, &mimepart_str).await?;

    let new_size = if is_dir { 0i64 } else { metadata.size as i64 };
    let new_etag = metadata.etag.as_str().to_string();
    let mtime = sys_to_unix(metadata.mtime);
    let permissions = metadata.permissions.bits() as i64;

    // Resolve parent fileid (if any) — must already exist or AncestorMissing,
    // except for the root row whose parent is `None`.
    let parent_fileid = resolve_parent_fileid(cache, storage_pk, path).await?;

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) = upsert_leaf_sqlite(
                &mut tx,
                storage_pk,
                path,
                parent_fileid,
                mimetype_pk,
                mimepart_pk,
                new_size,
                mtime,
                mtime,
                &new_etag,
                permissions,
            )
            .await?;
            let delta = new_size - old_size;
            propagate_ancestors_sqlite(&mut tx, storage_pk, path, delta, mtime).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) = upsert_leaf_mysql(
                &mut tx,
                storage_pk,
                path,
                parent_fileid,
                mimetype_pk,
                mimepart_pk,
                new_size,
                mtime,
                mtime,
                &new_etag,
                permissions,
            )
            .await?;
            let delta = new_size - old_size;
            propagate_ancestors_mysql(&mut tx, storage_pk, path, delta, mtime).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let (old_size, _existing_id) = upsert_leaf_postgres(
                &mut tx,
                storage_pk,
                path,
                parent_fileid,
                mimetype_pk,
                mimepart_pk,
                new_size,
                mtime,
                mtime,
                &new_etag,
                permissions,
            )
            .await?;
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
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk)
            .bind(&ph)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            // ON DELETE CASCADE on the parent FK takes care of descendants.
            sqlx::query("DELETE FROM oc_filecache WHERE storage = ? AND path_hash = ?")
                .bind(storage_pk)
                .bind(&ph)
                .execute(&mut *tx)
                .await
                .map_err(FileCacheError::Db)?;
            let now = unix_now();
            propagate_ancestors_sqlite(&mut tx, storage_pk, path, -old_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(u64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk as u32)
            .bind(&ph)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            sqlx::query("DELETE FROM oc_filecache WHERE storage = ? AND path_hash = ?")
                .bind(storage_pk as u32)
                .bind(&ph)
                .execute(&mut *tx)
                .await
                .map_err(FileCacheError::Db)?;
            let now = unix_now();
            propagate_ancestors_mysql(&mut tx, storage_pk, path, -old_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = $1 AND path_hash = $2",
            )
            .bind(storage_pk as i32)
            .bind(&ph)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((_fileid, old_size)) = row else {
                tx.commit().await.map_err(FileCacheError::Db)?;
                return Ok(());
            };
            sqlx::query("DELETE FROM oc_filecache WHERE storage = $1 AND path_hash = $2")
                .bind(storage_pk as i32)
                .bind(&ph)
                .execute(&mut *tx)
                .await
                .map_err(FileCacheError::Db)?;
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
    let now = unix_now();

    match cache.pool() {
        DbPool::Sqlite(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "UPDATE oc_filecache SET path = ?, path_hash = ?, parent = ?, name = ? \
                 WHERE fileid = ?",
            )
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk)
            .bind(&new_name)
            .bind(leaf_id)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            // If the leaf is a directory, rewrite every descendant's path.
            rewrite_descendant_paths_sqlite(&mut tx, storage_pk, from, to).await?;
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
            .bind(storage_pk as u32)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "UPDATE oc_filecache SET path = ?, path_hash = ?, parent = ?, name = ? \
                 WHERE fileid = ?",
            )
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk.map(|x| x as u64))
            .bind(&new_name)
            .bind(leaf_id)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            rewrite_descendant_paths_mysql(&mut tx, storage_pk, from, to).await?;
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
            .bind(storage_pk as i32)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((leaf_id, old_size)) = row else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "UPDATE oc_filecache SET path = $1, path_hash = $2, parent = $3, name = $4 \
                 WHERE fileid = $5",
            )
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk)
            .bind(&new_name)
            .bind(leaf_id)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            rewrite_descendant_paths_postgres(&mut tx, storage_pk, from, to).await?;
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
            let src: Option<(i64, i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size, mtime, storage_mtime, permissions \
                 FROM oc_filecache \
                 WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((
                _src_id,
                mimetype_pk,
                mimepart_pk,
                src_size,
                _src_mtime,
                src_storage_mtime,
                src_permissions,
            )) = src
            else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
                 mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(storage_pk)
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk)
            .bind(&new_name)
            .bind(mimetype_pk)
            .bind(mimepart_pk)
            .bind(src_size)
            .bind(now)
            .bind(src_storage_mtime)
            .bind(&new_etag)
            .bind(src_permissions)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            propagate_ancestors_sqlite(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::MySql(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let src: Option<(u64, u32, u32, i64, u32, u32, u32)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size, mtime, storage_mtime, permissions \
                 FROM oc_filecache \
                 WHERE storage = ? AND path_hash = ?",
            )
            .bind(storage_pk as u32)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((
                _src_id,
                mimetype_pk,
                mimepart_pk,
                src_size,
                _src_mtime,
                src_storage_mtime,
                src_permissions,
            )) = src
            else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
                 mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(storage_pk as u32)
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk.map(|x| x as u64))
            .bind(&new_name)
            .bind(mimetype_pk)
            .bind(mimepart_pk)
            .bind(src_size)
            .bind(now as u32)
            .bind(src_storage_mtime)
            .bind(&new_etag)
            .bind(src_permissions)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            propagate_ancestors_mysql(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
        DbPool::Postgres(p) => {
            let mut tx = p.begin().await.map_err(FileCacheError::Db)?;
            let src: Option<(i64, i32, i32, i64, i32, i32, i32)> = sqlx::query_as(
                "SELECT fileid, mimetype, mimepart, size, mtime, storage_mtime, permissions \
                 FROM oc_filecache \
                 WHERE storage = $1 AND path_hash = $2",
            )
            .bind(storage_pk as i32)
            .bind(&from_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            let Some((
                _src_id,
                mimetype_pk,
                mimepart_pk,
                src_size,
                _src_mtime,
                src_storage_mtime,
                src_permissions,
            )) = src
            else {
                return Err(FileCacheError::NotFound);
            };
            sqlx::query(
                "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
                 mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(storage_pk as i32)
            .bind(to.as_str())
            .bind(&to_hash)
            .bind(to_parent_pk)
            .bind(&new_name)
            .bind(mimetype_pk)
            .bind(mimepart_pk)
            .bind(src_size)
            .bind(now as i32)
            .bind(src_storage_mtime)
            .bind(&new_etag)
            .bind(src_permissions)
            .execute(&mut *tx)
            .await
            .map_err(FileCacheError::Db)?;
            propagate_ancestors_postgres(&mut tx, storage_pk, to, src_size, now).await?;
            tx.commit().await.map_err(FileCacheError::Db)?;
        }
    }
    Ok(())
}

// --- per-dialect leaf upsert ---

#[allow(clippy::too_many_arguments)]
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
    let existing: Option<(i64, i64)> =
        sqlx::query_as("SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?")
            .bind(storage_pk)
            .bind(&ph)
            .fetch_optional(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = ?, name = ?, mimetype = ?, mimepart = ?, \
             size = ?, mtime = ?, storage_mtime = ?, etag = ?, permissions = ? \
             WHERE fileid = ?",
        )
        .bind(parent_pk)
        .bind(path.basename())
        .bind(mimetype_pk)
        .bind(mimepart_pk)
        .bind(new_size)
        .bind(mtime)
        .bind(storage_mtime)
        .bind(etag)
        .bind(permissions)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
             mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(storage_pk)
        .bind(path.as_str())
        .bind(&ph)
        .bind(parent_pk)
        .bind(path.basename())
        .bind(mimetype_pk)
        .bind(mimepart_pk)
        .bind(new_size)
        .bind(mtime)
        .bind(storage_mtime)
        .bind(etag)
        .bind(permissions)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

#[allow(clippy::too_many_arguments)]
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
    let existing: Option<(u64, i64)> =
        sqlx::query_as("SELECT fileid, size FROM oc_filecache WHERE storage = ? AND path_hash = ?")
            .bind(storage_pk as u32)
            .bind(&ph)
            .fetch_optional(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id as i64)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = ?, name = ?, mimetype = ?, mimepart = ?, \
             size = ?, mtime = ?, storage_mtime = ?, etag = ?, permissions = ? \
             WHERE fileid = ?",
        )
        .bind(parent_pk.map(|x| x as u64))
        .bind(path.basename())
        .bind(mimetype_pk as u32)
        .bind(mimepart_pk as u32)
        .bind(new_size)
        .bind(mtime as u32)
        .bind(storage_mtime as u32)
        .bind(etag)
        .bind(permissions as u32)
        .bind(id as u64)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
             mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(storage_pk as u32)
        .bind(path.as_str())
        .bind(&ph)
        .bind(parent_pk.map(|x| x as u64))
        .bind(path.basename())
        .bind(mimetype_pk as u32)
        .bind(mimepart_pk as u32)
        .bind(new_size)
        .bind(mtime as u32)
        .bind(storage_mtime as u32)
        .bind(etag)
        .bind(permissions as u32)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

#[allow(clippy::too_many_arguments)]
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
    .bind(storage_pk as i32)
    .bind(&ph)
    .fetch_optional(&mut **tx)
    .await
    .map_err(FileCacheError::Db)?;
    let (old_size, existing_id) = match existing {
        Some((id, sz)) => (sz, Some(id)),
        None => (0, None),
    };
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE oc_filecache SET parent = $1, name = $2, mimetype = $3, mimepart = $4, \
             size = $5, mtime = $6, storage_mtime = $7, etag = $8, permissions = $9 \
             WHERE fileid = $10",
        )
        .bind(parent_pk)
        .bind(path.basename())
        .bind(mimetype_pk as i32)
        .bind(mimepart_pk as i32)
        .bind(new_size)
        .bind(mtime as i32)
        .bind(storage_mtime as i32)
        .bind(etag)
        .bind(permissions as i32)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    } else {
        sqlx::query(
            "INSERT INTO oc_filecache (storage, path, path_hash, parent, name, \
             mimetype, mimepart, size, mtime, storage_mtime, etag, permissions) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(storage_pk as i32)
        .bind(path.as_str())
        .bind(&ph)
        .bind(parent_pk)
        .bind(path.basename())
        .bind(mimetype_pk as i32)
        .bind(mimepart_pk as i32)
        .bind(new_size)
        .bind(mtime as i32)
        .bind(storage_mtime as i32)
        .bind(etag)
        .bind(permissions as i32)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
    }
    Ok((old_size, existing_id))
}

// --- per-dialect ancestor walk ---

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
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?",
        )
        .bind(storage_pk)
        .bind(&ph)
        .fetch_optional(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() {
                break;
            }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + ?, etag = ?, mtime = ? WHERE fileid = ?",
        )
        .bind(delta)
        .bind(&new_etag)
        .bind(mtime)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        if anc.is_root() {
            break;
        }
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
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?",
        )
        .bind(storage_pk as u32)
        .bind(&ph)
        .fetch_optional(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() {
                break;
            }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + ?, etag = ?, mtime = ? WHERE fileid = ?",
        )
        .bind(delta)
        .bind(&new_etag)
        .bind(mtime as u32)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        if anc.is_root() {
            break;
        }
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
            "SELECT fileid FROM oc_filecache WHERE storage = $1 AND path_hash = $2",
        )
        .bind(storage_pk as i32)
        .bind(&ph)
        .fetch_optional(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        let Some(id) = fileid else {
            if anc.is_root() {
                break;
            }
            return Err(FileCacheError::AncestorMissing(anc));
        };
        let new_etag = ETag::new().as_str().to_string();
        sqlx::query(
            "UPDATE oc_filecache SET size = size + $1, etag = $2, mtime = $3 WHERE fileid = $4",
        )
        .bind(delta)
        .bind(&new_etag)
        .bind(mtime as i32)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(FileCacheError::Db)?;
        if anc.is_root() {
            break;
        }
        cur = anc.parent();
    }
    Ok(())
}

// --- per-dialect descendant-path rewrite ---

async fn rewrite_descendant_paths_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = if from.is_root() {
        String::new()
    } else {
        format!("{}/", from.as_str())
    };
    let like_pattern = format!("{from_prefix}%");
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT fileid, path FROM oc_filecache WHERE storage = ? AND path LIKE ?")
            .bind(storage_pk)
            .bind(&like_pattern)
            .fetch_all(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        if old_path.len() < from_prefix.len() {
            continue;
        }
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() {
            suffix.to_string()
        } else {
            format!("{}/{}", to.as_str(), suffix)
        };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = ?, path_hash = ? WHERE fileid = ?")
            .bind(&new_path)
            .bind(&new_hash)
            .bind(fileid)
            .execute(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    }
    Ok(())
}

async fn rewrite_descendant_paths_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = if from.is_root() {
        String::new()
    } else {
        format!("{}/", from.as_str())
    };
    let like_pattern = format!("{from_prefix}%");
    let rows: Vec<(u64, String)> =
        sqlx::query_as("SELECT fileid, path FROM oc_filecache WHERE storage = ? AND path LIKE ?")
            .bind(storage_pk as u32)
            .bind(&like_pattern)
            .fetch_all(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        if old_path.len() < from_prefix.len() {
            continue;
        }
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() {
            suffix.to_string()
        } else {
            format!("{}/{}", to.as_str(), suffix)
        };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = ?, path_hash = ? WHERE fileid = ?")
            .bind(&new_path)
            .bind(&new_hash)
            .bind(fileid)
            .execute(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    }
    Ok(())
}

async fn rewrite_descendant_paths_postgres(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    storage_pk: i64,
    from: &StoragePath,
    to: &StoragePath,
) -> FileCacheResult<()> {
    let from_prefix = if from.is_root() {
        String::new()
    } else {
        format!("{}/", from.as_str())
    };
    let like_pattern = format!("{from_prefix}%");
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT fileid, path FROM oc_filecache WHERE storage = $1 AND path LIKE $2")
            .bind(storage_pk as i32)
            .bind(&like_pattern)
            .fetch_all(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    for (fileid, old_path) in rows {
        if old_path.len() < from_prefix.len() {
            continue;
        }
        let suffix = &old_path[from_prefix.len()..];
        let new_path = if to.is_root() {
            suffix.to_string()
        } else {
            format!("{}/{}", to.as_str(), suffix)
        };
        let new_path_obj = StoragePath::new(new_path.clone())
            .map_err(|e| FileCacheError::Invalid(format!("rewrite produced invalid path: {e}")))?;
        let new_hash = path_hash(&new_path_obj);
        sqlx::query("UPDATE oc_filecache SET path = $1, path_hash = $2 WHERE fileid = $3")
            .bind(&new_path)
            .bind(&new_hash)
            .bind(fileid)
            .execute(&mut **tx)
            .await
            .map_err(FileCacheError::Db)?;
    }
    Ok(())
}

/// Resolve `path.parent()` to a `fileid`. Returns:
/// - `Ok(None)` if `path` is the root (no parent).
/// - `Ok(Some(id))` if the parent row exists.
/// - `Ok(None)` if the parent is the root row and the root row is absent
///   (root insertion is lazy).
/// - `Err(AncestorMissing(parent))` if a non-root parent is absent.
async fn resolve_parent_fileid(
    cache: &FileCache,
    storage_pk: i64,
    path: &StoragePath,
) -> FileCacheResult<Option<i64>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let ph = path_hash(&parent);
    let fileid: Option<i64> = match cache.pool() {
        DbPool::Sqlite(p) => sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?",
        )
        .bind(storage_pk)
        .bind(&ph)
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?,
        DbPool::MySql(p) => sqlx::query_scalar::<_, u64>(
            "SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?",
        )
        .bind(storage_pk as u32)
        .bind(&ph)
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?
        .map(|x| x as i64),
        DbPool::Postgres(p) => sqlx::query_scalar(
            "SELECT fileid FROM oc_filecache WHERE storage = $1 AND path_hash = $2",
        )
        .bind(storage_pk as i32)
        .bind(&ph)
        .fetch_optional(p)
        .await
        .map_err(FileCacheError::Db)?,
    };
    match fileid {
        Some(id) => Ok(Some(id)),
        None if parent.is_root() => Ok(None),
        None => Err(FileCacheError::AncestorMissing(parent)),
    }
}

fn sys_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_now() -> i64 {
    sys_to_unix(SystemTime::now())
}
