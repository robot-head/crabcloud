//! `Versions` — file version snapshot + list + restore + delete + sweep.
//!
//! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
//! On-disk layout: `<datadir>/<uid>/files_versions/<relative>.v<mtime>`.
//!
//! Multidialect SQL is dispatched via `match self.pool.as_ref()` and each
//! dialect decodes its row into a shared `RowParts` struct (mirroring the
//! `crabcloud-trash` pattern).

use crate::error::VersionsError;
use crate::sql;
use crate::types::VersionEntry;
use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct Versions {
    pool: Arc<DbPool>,
    /// Filesystem root that contains `<uid>/files/...` and
    /// `<uid>/files_versions/...`. Same value as `FileConfig::datadirectory`.
    datadir: PathBuf,
}

impl Versions {
    pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
        Self { pool, datadir }
    }

    /// Filesystem root the service operates on. Same value as
    /// `FileConfig::datadirectory`.
    pub fn datadir(&self) -> &Path {
        &self.datadir
    }

    // -------- snapshot_if_needed --------

    /// Snapshot the current bytes at `<datadir>/<uid>/files/<src_path>` to
    /// the versions tree, recording an `oc_files_versions` row. Returns
    /// `Ok(Some(id))` on a successful snapshot, `Ok(None)` if the
    /// snapshot was skipped (zero-byte, oversize, throttled), or `Err` on
    /// real failure.
    ///
    /// The caller passes the pre-write current size (cheap to compute
    /// from the filecache row) plus `now_secs`, `throttle_secs`, and
    /// `max_bytes` drawn from config. Decoupling these from `Versions`
    /// itself keeps the service free of clock + config dependencies and
    /// makes tests deterministic.
    #[allow(clippy::too_many_arguments)]
    pub async fn snapshot_if_needed(
        &self,
        uid: &str,
        storage_id: i64,
        fileid: i64,
        src_path: &str,
        current_size: i64,
        now_secs: i64,
        throttle_secs: i64,
        max_bytes: u64,
    ) -> Result<Option<i64>, VersionsError> {
        if current_size <= 0 {
            return Ok(None);
        }
        if (current_size as u64) > max_bytes {
            tracing::warn!(
                uid,
                fileid,
                current_size,
                max_bytes,
                "versions: skipping snapshot, size exceeds max_bytes"
            );
            return Ok(None);
        }
        if throttle_secs > 0 {
            if let Some(latest) = self.get_latest_for(storage_id, fileid).await? {
                if now_secs - latest.version_mtime < throttle_secs {
                    return Ok(None);
                }
            }
        }

        let rel = src_path.trim_start_matches('/');
        let src_abs = self.datadir.join(uid).join("files").join(rel);
        if !tokio::fs::try_exists(&src_abs).await? {
            return Err(VersionsError::SourceMissing);
        }
        let parent = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
        let dst_dir = self.datadir.join(uid).join("files_versions").join(parent);
        tokio::fs::create_dir_all(&dst_dir).await?;
        let basename = Path::new(rel)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(VersionsError::SourceMissing)?;
        let dst_abs = dst_dir.join(format!("{basename}.v{now_secs}"));
        if let Err(e) = tokio::fs::copy(&src_abs, &dst_abs).await {
            // Best-effort cleanup of any partial copy; `.ok()` so the
            // cleanup error can't mask the original error.
            tokio::fs::remove_file(&dst_abs).await.ok();
            return Err(e.into());
        }

        let path_for_row = format!("/{rel}");
        let id = match self
            .insert_row(
                storage_id,
                fileid,
                uid,
                &path_for_row,
                now_secs,
                current_size,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    orphan_path = %dst_abs.display(),
                    uid,
                    "versions snapshot_if_needed: INSERT failed after copy; bytes stranded"
                );
                return Err(e);
            }
        };
        Ok(Some(id))
    }

    async fn insert_row(
        &self,
        storage_id: i64,
        fileid: i64,
        uid: &str,
        path: &str,
        version_mtime: i64,
        size: i64,
    ) -> Result<i64, VersionsError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .bind(uid)
                    .bind(path)
                    .bind(version_mtime)
                    .bind(size)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .bind(uid)
                    .bind(path)
                    .bind(version_mtime)
                    .bind(size)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(storage_id)
                    .bind(fileid)
                    .bind(uid)
                    .bind(path)
                    .bind(version_mtime)
                    .bind(size)
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }

    // -------- list_for / get_by_id / get_latest_for --------

    /// All versions for `(uid, fileid)`, newest-first.
    pub async fn list_for(
        &self,
        uid: &str,
        fileid: i64,
    ) -> Result<Vec<VersionEntry>, VersionsError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_FOR_QM)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_FOR_QM)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_FOR_PG)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    /// Fetch a single version row by primary key.
    pub async fn get_by_id(&self, id: i64) -> Result<VersionEntry, VersionsError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::GET_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite)
                    .transpose()?
                    .ok_or(VersionsError::NotFound)
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::GET_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql)
                    .transpose()?
                    .ok_or(VersionsError::NotFound)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::GET_BY_ID_PG)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres)
                    .transpose()?
                    .ok_or(VersionsError::NotFound)
            }
        }
    }

    async fn get_latest_for(
        &self,
        storage_id: i64,
        fileid: i64,
    ) -> Result<Option<VersionEntry>, VersionsError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::GET_LATEST_FOR_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::GET_LATEST_FOR_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::GET_LATEST_FOR_PG)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres).transpose()
            }
        }
    }

    // -------- restore --------

    /// Snapshot the current file at `<uid>/files/<entry.path>` (so the
    /// pre-restore state is not lost), then copy the version's bytes
    /// over current. Caller passes `current_size_for_snapshot`,
    /// `now_secs`, and the throttle / size_cap config — same shape as
    /// `snapshot_if_needed`. Lossless.
    pub async fn restore(
        &self,
        uid: &str,
        version_id: i64,
        current_size_for_snapshot: i64,
        now_secs: i64,
        throttle_secs: i64,
        max_bytes: u64,
    ) -> Result<(), VersionsError> {
        let entry = self.get_by_id(version_id).await?;
        if entry.user != uid {
            return Err(VersionsError::WrongUser);
        }
        // Snapshot current first. A `None` here means current was zero or
        // throttled; restore still proceeds.
        let _ = self
            .snapshot_if_needed(
                uid,
                entry.storage_id,
                entry.fileid,
                &entry.path,
                current_size_for_snapshot,
                now_secs,
                throttle_secs,
                max_bytes,
            )
            .await?;

        let rel = entry.path.trim_start_matches('/');
        let parent = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
        let basename = Path::new(rel)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(VersionsError::SourceMissing)?;
        let src_abs = self
            .datadir
            .join(uid)
            .join("files_versions")
            .join(parent)
            .join(format!("{basename}.v{}", entry.version_mtime));
        if !tokio::fs::try_exists(&src_abs).await? {
            return Err(VersionsError::SourceMissing);
        }
        let dst_dir = self.datadir.join(uid).join("files").join(parent);
        tokio::fs::create_dir_all(&dst_dir).await?;
        let dst_abs = dst_dir.join(basename);
        tokio::fs::copy(&src_abs, &dst_abs).await?;
        Ok(())
    }

    // -------- delete --------

    /// Hard-delete one version row + its on-disk file. Validates that
    /// `uid` owns the row.
    pub async fn delete(&self, uid: &str, id: i64) -> Result<(), VersionsError> {
        let entry = self.get_by_id(id).await?;
        if entry.user != uid {
            return Err(VersionsError::WrongUser);
        }
        self.delete_entry(&entry).await
    }

    async fn delete_entry(&self, entry: &VersionEntry) -> Result<(), VersionsError> {
        let rel = entry.path.trim_start_matches('/');
        let basename = Path::new(rel)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(VersionsError::SourceMissing)?;
        let parent = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
        let on_disk = self
            .datadir
            .join(&entry.user)
            .join("files_versions")
            .join(parent)
            .join(format!("{basename}.v{}", entry.version_mtime));
        if tokio::fs::try_exists(&on_disk).await? {
            tokio::fs::remove_file(&on_disk).await?;
        }
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::DELETE_QM)
                    .bind(entry.id)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::DELETE_QM)
                    .bind(entry.id)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::DELETE_PG)
                    .bind(entry.id)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    // -------- purge_for_fileid --------

    /// Remove every version row + on-disk file for `(storage_id, fileid)`.
    /// Invoked by `Trash::purge_entry` on hard-delete cascade.
    /// Best-effort per row: individual file-remove failures are logged
    /// but don't abort the cascade.
    pub async fn purge_for_fileid(
        &self,
        storage_id: i64,
        fileid: i64,
    ) -> Result<u64, VersionsError> {
        let entries: Vec<VersionEntry> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_FOR_FILEID_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_sqlite)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_FOR_FILEID_QM)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_mysql)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_FOR_FILEID_PG)
                    .bind(storage_id)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_postgres)
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        let mut n = 0u64;
        for entry in entries {
            if let Err(e) = self.delete_entry(&entry).await {
                tracing::warn!(
                    error = %e,
                    version_id = entry.id,
                    "versions purge_for_fileid: delete_entry failed"
                );
                continue;
            }
            n += 1;
        }
        Ok(n)
    }

    /// Remove every version row + on-disk file for `(uid, fileid)`. Used
    /// by the trash cascade when the caller knows the owner uid (from
    /// the trash row's `user` column) but not the owner-home numeric
    /// storage_id. Best-effort same shape as `purge_for_fileid`.
    pub async fn purge_for_user_fileid(
        &self,
        uid: &str,
        fileid: i64,
    ) -> Result<u64, VersionsError> {
        let entries: Vec<VersionEntry> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_FOR_USER_FILEID_QM)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_sqlite)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_FOR_USER_FILEID_QM)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_mysql)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_FOR_USER_FILEID_PG)
                    .bind(uid)
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter()
                    .map(row_from_postgres)
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        let mut n = 0u64;
        for entry in entries {
            if let Err(e) = self.delete_entry(&entry).await {
                tracing::warn!(
                    error = %e,
                    version_id = entry.id,
                    "versions purge_for_user_fileid: delete_entry failed"
                );
                continue;
            }
            n += 1;
        }
        Ok(n)
    }

    // -------- sweep_tiered --------

    /// Apply the tiered retention rule per `(user, fileid)` group.
    /// Returns the number of rows purged. Bucket schedule (relative to
    /// `now_secs`):
    ///   0-24h: keep every version
    ///   24h-30d: keep one per hour bucket (newest in each)
    ///   30d-180d: keep one per day bucket
    ///   180d+: keep one per week bucket
    pub async fn sweep_tiered(&self, now_secs: i64) -> Result<u64, VersionsError> {
        let groups: Vec<(String, i64)> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_GROUPS_QM).fetch_all(p).await?;
                rows.into_iter()
                    .map(|r| -> Result<_, VersionsError> {
                        Ok((r.try_get::<String, _>("user")?, r.try_get::<i64, _>("fileid")?))
                    })
                    .collect::<Result<_, _>>()?
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_GROUPS_QM).fetch_all(p).await?;
                rows.into_iter()
                    .map(|r| -> Result<_, VersionsError> {
                        Ok((r.try_get::<String, _>("user")?, r.try_get::<i64, _>("fileid")?))
                    })
                    .collect::<Result<_, _>>()?
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_GROUPS_PG).fetch_all(p).await?;
                rows.into_iter()
                    .map(|r| -> Result<_, VersionsError> {
                        Ok((r.try_get::<String, _>("user")?, r.try_get::<i64, _>("fileid")?))
                    })
                    .collect::<Result<_, _>>()?
            }
        };

        let mut purged_total = 0u64;
        for (uid, fileid) in groups {
            // list_for returns newest-first.
            let entries = self.list_for(&uid, fileid).await?;
            let mut seen_slots: std::collections::HashSet<(u8, i64)> =
                std::collections::HashSet::new();
            for entry in entries {
                let age_secs = now_secs - entry.version_mtime;
                let slot = bucket_slot(age_secs, entry.version_mtime);
                // tag 0 = "keep every version" — don't dedupe.
                let keep = if slot.0 == 0 {
                    true
                } else {
                    seen_slots.insert(slot)
                };
                if !keep {
                    if let Err(e) = self.delete_entry(&entry).await {
                        tracing::warn!(
                            error = %e,
                            id = entry.id,
                            "versions sweep: delete_entry failed"
                        );
                        continue;
                    }
                    purged_total += 1;
                }
            }
        }
        Ok(purged_total)
    }
}

/// Bucket classifier. Returns `(bucket_tag, slot_key)`. The sweeper
/// keeps the newest version per `(bucket_tag, slot_key)` pair, except
/// for `bucket_tag == 0` (the 0-24h "keep every" tier) where the slot
/// is ignored.
///
/// Tag values:
///   0 — within 24h (keep every)
///   1 — 24h-30d (one per hour bucket)
///   2 — 30d-180d (one per day bucket)
///   3 — 180d+ (one per week bucket)
fn bucket_slot(age_secs: i64, version_mtime: i64) -> (u8, i64) {
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;
    const WEEK: i64 = 7 * DAY;
    if age_secs < DAY {
        (0, 0) // ignored
    } else if age_secs < 30 * DAY {
        (1, version_mtime / HOUR)
    } else if age_secs < 180 * DAY {
        (2, version_mtime / DAY)
    } else {
        (3, version_mtime / WEEK)
    }
}

/// Decoded slice of a versions row that the per-dialect decoders all
/// agree on. Assembled by `assemble_row` into a typed `VersionEntry`.
struct RowParts {
    id: i64,
    storage_id: i64,
    fileid: i64,
    user: String,
    path: String,
    version_mtime: i64,
    size: i64,
}

fn assemble_row(parts: RowParts) -> Result<VersionEntry, VersionsError> {
    Ok(VersionEntry {
        id: parts.id,
        storage_id: parts.storage_id,
        fileid: parts.fileid,
        user: parts.user,
        path: parts.path,
        version_mtime: parts.version_mtime,
        size: parts.size,
    })
}

fn row_from_sqlite(row: sqlx::sqlite::SqliteRow) -> Result<VersionEntry, VersionsError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        storage_id: row.try_get("storage_id")?,
        fileid: row.try_get("fileid")?,
        user: row.try_get("user")?,
        path: row.try_get("path")?,
        version_mtime: row.try_get("version_mtime")?,
        size: row.try_get("size")?,
    })
}

fn row_from_mysql(row: sqlx::mysql::MySqlRow) -> Result<VersionEntry, VersionsError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        storage_id: row.try_get("storage_id")?,
        fileid: row.try_get("fileid")?,
        user: row.try_get("user")?,
        path: row.try_get("path")?,
        version_mtime: row.try_get("version_mtime")?,
        size: row.try_get("size")?,
    })
}

fn row_from_postgres(row: sqlx::postgres::PgRow) -> Result<VersionEntry, VersionsError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        storage_id: row.try_get("storage_id")?,
        fileid: row.try_get("fileid")?,
        user: row.try_get("user")?,
        path: row.try_get("path")?,
        version_mtime: row.try_get("version_mtime")?,
        size: row.try_get("size")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The dev-dependencies `crabcloud-config` and `tempfile` are used by
    // the integration test at `tests/versions_e2e.rs`; the lib-test
    // target doesn't see them, so anchor them here to keep the
    // `unused_crate_dependencies` lint quiet.
    use crabcloud_config as _;
    use tempfile as _;

    #[test]
    fn bucket_slot_within_24h_is_tag_zero() {
        assert_eq!(bucket_slot(60, 1_000), (0, 0));
        assert_eq!(bucket_slot(86_399, 1_000), (0, 0));
    }

    #[test]
    fn bucket_slot_24h_to_30d_uses_hour_buckets() {
        assert_eq!(bucket_slot(86_400, 1_000), (1, 1_000 / 3_600));
        assert_eq!(bucket_slot(30 * 86_400 - 1, 1_000), (1, 1_000 / 3_600));
    }

    #[test]
    fn bucket_slot_30d_to_180d_uses_day_buckets() {
        assert_eq!(
            bucket_slot(30 * 86_400, 1_000_000),
            (2, 1_000_000 / 86_400)
        );
        assert_eq!(
            bucket_slot(180 * 86_400 - 1, 1_000_000),
            (2, 1_000_000 / 86_400)
        );
    }

    #[test]
    fn bucket_slot_over_180d_uses_week_buckets() {
        assert_eq!(
            bucket_slot(180 * 86_400, 1_000_000),
            (3, 1_000_000 / (7 * 86_400))
        );
    }
}
