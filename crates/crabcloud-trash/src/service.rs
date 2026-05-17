//! `Trash` — soft-delete + list + restore + purge + sweep.
//!
//! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
//! Trashbin layout on disk: `<datadir>/<uid>/files_trashbin/files/<basename>.<suffix>`.
//! Restored files go back to `<datadir>/<uid>/files/<location>/<basename>`,
//! creating missing parents and suffixing the basename with ` (restored)`
//! on collision.

use crate::error::TrashError;
use crate::sql;
use crate::types::{RestoredTo, TrashEntry, TrashType};
use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cap on restore-collision suffix attempts before giving up.
const RESTORE_COLLISION_CAP: u32 = 99;

#[derive(Clone)]
pub struct Trash {
    pool: Arc<DbPool>,
    /// Filesystem root that contains `<uid>/files/...` and `<uid>/files_trashbin/...`.
    /// Same value as `FileConfig::datadirectory`.
    datadir: PathBuf,
}

impl Trash {
    pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
        Self { pool, datadir }
    }

    /// Filesystem root the service operates on. Same value as
    /// `FileConfig::datadirectory`.
    pub fn datadir(&self) -> &Path {
        &self.datadir
    }

    // -------- soft_delete --------

    /// Move `<datadir>/<uid>/files/<src_path>` to the trashbin and write
    /// the metadata row. Returns the new trash row id.
    pub async fn soft_delete(
        &self,
        uid: &str,
        src_path: &str,
        kind: TrashType,
        fileid_legacy: Option<i64>,
    ) -> Result<i64, TrashError> {
        let src_path = src_path.trim_start_matches('/').to_string();
        validate_relative_path(&src_path)?;
        let basename = Path::new(&src_path)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(TrashError::SourceMissing)?
            .to_string();
        let location = match Path::new(&src_path).parent().and_then(|p| p.to_str()) {
            Some("") | None => "/".to_string(),
            Some(parent) => format!("/{parent}"),
        };

        let now = chrono::Utc::now().timestamp();
        let suffix = self.resolve_unique_suffix(uid, &basename, now).await?;
        let trash_dir = self.datadir.join(uid).join("files_trashbin").join("files");
        tokio::fs::create_dir_all(&trash_dir).await?;
        let src = self.datadir.join(uid).join("files").join(&src_path);
        let dst = trash_dir.join(format!("{basename}.{suffix}"));
        if !tokio::fs::try_exists(&src).await? {
            return Err(TrashError::SourceMissing);
        }
        tokio::fs::rename(&src, &dst).await?;

        let id = match self
            .insert_row(uid, &basename, &suffix, &location, now, kind, fileid_legacy)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    orphan_path = %dst.display(),
                    uid,
                    "trash soft_delete: INSERT failed after rename; bytes stranded at orphan_path"
                );
                return Err(e);
            }
        };
        Ok(id)
    }

    /// Compute a unique on-disk suffix for `basename` at `now_secs`. The
    /// returned suffix is `d<secs>` for the common case, or `d<secs>_N`
    /// if a prior delete in the same second already used the bare suffix.
    ///
    /// **TOCTOU note:** two concurrent soft-deletes of the same basename
    /// within the same second can both observe `n == 0` and race to insert
    /// the same `(user, basename, suffix)` — the second client surfaces
    /// a `Db(sqlx::Error)` from the `idx_trash_user_name` unique-index
    /// violation. Single-writer deployments don't hit this; multi-writer
    /// ones rarely do. Revisit with a real transaction-bounded probe if
    /// it ever becomes common.
    async fn resolve_unique_suffix(
        &self,
        uid: &str,
        basename: &str,
        now_secs: i64,
    ) -> Result<String, TrashError> {
        let base = format!("d{now_secs}");
        let like = format!("{base}%");
        let n: i64 = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_QM)
                .bind(uid)
                .bind(basename)
                .bind(&like)
                .fetch_one(p)
                .await?
                .try_get("n")?,
            DbPool::MySql(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_QM)
                .bind(uid)
                .bind(basename)
                .bind(&like)
                .fetch_one(p)
                .await?
                .try_get("n")?,
            DbPool::Postgres(p) => sqlx::query(sql::COUNT_SUFFIX_PREFIX_PG)
                .bind(uid)
                .bind(basename)
                .bind(&like)
                .fetch_one(p)
                .await?
                .try_get("n")?,
        };
        Ok(if n == 0 {
            base
        } else {
            format!("{base}_{}", n + 1)
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_row(
        &self,
        uid: &str,
        basename: &str,
        suffix: &str,
        location: &str,
        deleted_at: i64,
        kind: TrashType,
        fileid_legacy: Option<i64>,
    ) -> Result<i64, TrashError> {
        let ty = kind.as_str();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .bind(location)
                    .bind(deleted_at)
                    .bind(ty)
                    .bind(fileid_legacy)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let r = sqlx::query(sql::INSERT_QM)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .bind(location)
                    .bind(deleted_at)
                    .bind(ty)
                    .bind(fileid_legacy)
                    .execute(p)
                    .await?;
                Ok(r.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .bind(location)
                    .bind(deleted_at)
                    .bind(ty)
                    .bind(fileid_legacy)
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }

    /// Soft-delete an entry whose bytes live OUTSIDE
    /// `<datadir>/<deleter_uid>/files/...` (i.e. a share-mount target
    /// in another user's storage). Streams the bytes through `reader`
    /// into the deleter's `files_trashbin/files/` and writes the
    /// trash-row metadata under the deleter. Source removal is the
    /// caller's responsibility — the caller already holds the share-
    /// mount storage handle and is in the best position to honor that
    /// backend's `delete` semantics (and emit the right storage event
    /// for the filecache scanner).
    ///
    /// Use this for the spec §2 decision #7 path: "shared-with-me
    /// delete lands in the DELETER's bin". For ordinary home deletes
    /// (single-user) use [`Self::soft_delete`] which does the cheaper
    /// same-filesystem rename.
    ///
    /// `location_for_row` is the path the trash row should record as
    /// the original location; for share-mount deletes it's the
    /// deleter's view path's parent (e.g. `/Shared/Vacation`), NOT the
    /// owner-relative storage path — restoring back to that location
    /// keeps the deleter's mental model intact.
    ///
    /// Partial-copy failures roll back the destination file before
    /// returning Err (so a half-written file at `dst` isn't orphaned —
    /// no trash row references it, and the sweeper can't reclaim it).
    /// INSERT failures after a successful copy log a `tracing::warn!`
    /// with the orphan path (caller can grep).
    ///
    /// Same suffix-collision TOCTOU caveat as [`Self::soft_delete`] —
    /// see that fn's `resolve_unique_suffix` note.
    pub async fn soft_delete_from_reader(
        &self,
        deleter_uid: &str,
        location_for_row: &str,
        basename: &str,
        kind: TrashType,
        fileid_legacy: Option<i64>,
        mut reader: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
    ) -> Result<i64, TrashError> {
        if basename.is_empty()
            || basename.contains('/')
            || basename.contains('\\')
            || basename.contains('\0')
            || basename == ".."
            || basename == "."
        {
            return Err(TrashError::SourceMissing);
        }
        let now = chrono::Utc::now().timestamp();
        let suffix = self
            .resolve_unique_suffix(deleter_uid, basename, now)
            .await?;
        let trash_dir = self
            .datadir
            .join(deleter_uid)
            .join("files_trashbin")
            .join("files");
        tokio::fs::create_dir_all(&trash_dir).await?;
        let dst = trash_dir.join(format!("{basename}.{suffix}"));
        // Copy reader → dst. Use create_new to avoid clobbering on the
        // off-chance a stale file with the same suffix already lives
        // there. On any error after create_new succeeds, unlink the
        // partial file so we don't leak bytes nobody references — .ok()
        // because the cleanup error must not mask the original error.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dst)
            .await?;
        if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
            drop(file);
            tokio::fs::remove_file(&dst).await.ok();
            return Err(e.into());
        }
        if let Err(e) = file.sync_all().await {
            drop(file);
            tokio::fs::remove_file(&dst).await.ok();
            return Err(e.into());
        }
        drop(file);

        let id = match self
            .insert_row(
                deleter_uid,
                basename,
                &suffix,
                location_for_row,
                now,
                kind,
                fileid_legacy,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    orphan_path = %dst.display(),
                    deleter_uid,
                    "trash soft_delete_from_reader: INSERT failed after copy; bytes stranded"
                );
                return Err(e);
            }
        };
        Ok(id)
    }

    /// Recursively soft-delete a directory whose bytes live OUTSIDE
    /// `<datadir>/<deleter_uid>/files/...` (i.e. a share-mount subtree
    /// in another user's storage) by copying the entire tree into the
    /// deleter's trashbin under a single trash row of type `Dir`. The
    /// on-disk shape mirrors home-storage dir soft-delete: one row,
    /// one `<basename>.<suffix>/` directory at the top of the bin, full
    /// subtree preserved underneath.
    ///
    /// Source removal is the caller's responsibility — like
    /// [`Self::soft_delete_from_reader`], the caller already holds the
    /// share-mount storage handle and is the right place to honor that
    /// backend's `delete` semantics (and emit the storage event the
    /// filecache scanner expects).
    ///
    /// `src_abs` is the absolute path to the source directory on local
    /// disk. For SP12 MVP the spec is local-first so a `&Path` argument
    /// is sufficient; cross-storage trash for non-local backends is
    /// explicitly out of scope and stays a future concern.
    ///
    /// Partial-copy failures roll back the destination tree (best-effort
    /// `remove_dir_all`) before returning Err so we don't end up with
    /// half-trash + half-source. INSERT failures after a successful copy
    /// emit a `tracing::warn!` with the orphan path, same as the file
    /// variant.
    ///
    /// Same suffix-collision TOCTOU caveat as [`Self::soft_delete`] —
    /// see that fn's `resolve_unique_suffix` note.
    pub async fn soft_delete_directory_from_path(
        &self,
        deleter_uid: &str,
        location_for_row: &str,
        basename: &str,
        src_abs: &Path,
        fileid_legacy: Option<i64>,
    ) -> Result<i64, TrashError> {
        if basename.is_empty()
            || basename.contains('/')
            || basename.contains('\\')
            || basename.contains('\0')
            || basename == ".."
            || basename == "."
        {
            return Err(TrashError::SourceMissing);
        }
        // Surface a clean SourceMissing if the source doesn't exist or
        // isn't a directory — same shape `soft_delete` uses for missing
        // file sources. Anything more exotic (broken symlink, permission
        // denied stat) will surface as Io below.
        let md = match tokio::fs::metadata(src_abs).await {
            Ok(md) => md,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TrashError::SourceMissing)
            }
            Err(e) => return Err(e.into()),
        };
        if !md.is_dir() {
            return Err(TrashError::SourceMissing);
        }

        let now = chrono::Utc::now().timestamp();
        let suffix = self
            .resolve_unique_suffix(deleter_uid, basename, now)
            .await?;
        let trash_dir = self
            .datadir
            .join(deleter_uid)
            .join("files_trashbin")
            .join("files");
        tokio::fs::create_dir_all(&trash_dir).await?;
        let dst_root = trash_dir.join(format!("{basename}.{suffix}"));
        // `create_dir` (not `create_dir_all`) fails if dst_root already
        // exists — defense in depth against an unexpected stale entry
        // colliding with our freshly resolved suffix.
        if let Err(e) = tokio::fs::create_dir(&dst_root).await {
            return Err(e.into());
        }

        if let Err(e) = copy_dir_recursive(src_abs, &dst_root).await {
            // Best-effort rollback of the partial destination tree so
            // we never leave bytes nobody references. `.ok()` so the
            // cleanup error can't mask the original error.
            tokio::fs::remove_dir_all(&dst_root).await.ok();
            return Err(e);
        }

        let id = match self
            .insert_row(
                deleter_uid,
                basename,
                &suffix,
                location_for_row,
                now,
                crate::types::TrashType::Dir,
                fileid_legacy,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    orphan_path = %dst_root.display(),
                    deleter_uid,
                    "trash soft_delete_directory_from_path: INSERT failed after copy; bytes stranded"
                );
                return Err(e);
            }
        };
        Ok(id)
    }

    // -------- list --------

    pub async fn list(&self, uid: &str) -> Result<Vec<TrashEntry>, TrashError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::LIST_QM).bind(uid).fetch_all(p).await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::LIST_QM).bind(uid).fetch_all(p).await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::LIST_PG).bind(uid).fetch_all(p).await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    pub async fn get_by_id(&self, id: i64) -> Result<TrashEntry, TrashError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::GET_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::GET_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::GET_BY_ID_PG)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
        }
    }

    pub async fn get_by_name(
        &self,
        uid: &str,
        basename: &str,
        suffix: &str,
    ) -> Result<TrashEntry, TrashError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::GET_BY_NAME_QM)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::GET_BY_NAME_QM)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::GET_BY_NAME_PG)
                    .bind(uid)
                    .bind(basename)
                    .bind(suffix)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres)
                    .transpose()?
                    .ok_or(TrashError::NotFound)
            }
        }
    }

    // -------- restore --------

    /// Restore `id`. If `dest_override` is None, restore to the row's
    /// original `location/basename`. Caller (DAV MOVE) may pass an
    /// explicit destination ("/dav/files/<uid>/foo/bar" reduced to
    /// "/foo/bar").
    pub async fn restore(
        &self,
        uid: &str,
        id: i64,
        dest_override: Option<&str>,
    ) -> Result<RestoredTo, TrashError> {
        let entry = self.get_by_id(id).await?;
        if entry.user != uid {
            return Err(TrashError::WrongUser);
        }
        let (target_dir_rel, target_basename) = match dest_override {
            Some(d) => {
                let trimmed = d.trim_start_matches('/');
                validate_relative_path(trimmed)?;
                let basename = Path::new(trimmed)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(entry.basename.as_str())
                    .to_string();
                (parent_of(trimmed).to_string(), basename)
            }
            None => (
                entry.location.trim_start_matches('/').to_string(),
                entry.basename.clone(),
            ),
        };

        let target_dir_abs = self.datadir.join(uid).join("files").join(&target_dir_rel);
        tokio::fs::create_dir_all(&target_dir_abs).await?;

        // Collision-resolved final filename inside target_dir_abs.
        let (final_name, final_rel) =
            pick_non_colliding_name(&target_dir_abs, &target_dir_rel, &target_basename).await?;

        let src = self
            .datadir
            .join(uid)
            .join("files_trashbin")
            .join("files")
            .join(format!("{}.{}", entry.basename, entry.suffix));
        if !tokio::fs::try_exists(&src).await? {
            return Err(TrashError::SourceMissing);
        }
        let dst = target_dir_abs.join(&final_name);
        tokio::fs::rename(&src, &dst).await?;

        if let Err(e) = self.delete_row(id).await {
            tracing::warn!(
                error = %e,
                row_id = id,
                gone_from = %src.display(),
                "trash restore: delete_row failed after rename; row points at missing trash file"
            );
            return Err(e);
        }
        Ok(RestoredTo { path: final_rel })
    }

    // -------- purge --------

    pub async fn purge(&self, uid: &str, id: i64) -> Result<(), TrashError> {
        let entry = self.get_by_id(id).await?;
        if entry.user != uid {
            return Err(TrashError::WrongUser);
        }
        self.purge_entry(&entry).await
    }

    /// Empty the user's bin. Returns count of rows removed.
    pub async fn purge_all(&self, uid: &str) -> Result<u64, TrashError> {
        let rows = self.list(uid).await?;
        let mut n = 0u64;
        for e in rows {
            self.purge_entry(&e).await?;
            n += 1;
        }
        Ok(n)
    }

    async fn purge_entry(&self, entry: &TrashEntry) -> Result<(), TrashError> {
        let src = self
            .datadir
            .join(&entry.user)
            .join("files_trashbin")
            .join("files")
            .join(format!("{}.{}", entry.basename, entry.suffix));
        if tokio::fs::try_exists(&src).await? {
            // Files: remove_file. Directories: remove_dir_all.
            if entry.r#type == TrashType::Dir {
                tokio::fs::remove_dir_all(&src).await?;
            } else {
                tokio::fs::remove_file(&src).await?;
            }
        }
        self.delete_row(entry.id).await?;
        Ok(())
    }

    async fn delete_row(&self, id: i64) -> Result<(), TrashError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::DELETE_QM).bind(id).execute(p).await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::DELETE_QM).bind(id).execute(p).await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::DELETE_PG).bind(id).execute(p).await?;
            }
        }
        Ok(())
    }

    // -------- sweep_expired --------

    /// Delete rows with `deleted_at < cutoff`. Returns the count
    /// deleted. Best-effort: file-removal errors on individual entries
    /// are logged but don't abort the sweep.
    pub async fn sweep_expired(&self, cutoff: i64, batch: i64) -> Result<u64, TrashError> {
        let rows: Vec<TrashEntry> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let raw = sqlx::query(sql::SELECT_EXPIRED_QM)
                    .bind(cutoff)
                    .bind(batch)
                    .fetch_all(p)
                    .await?;
                raw.into_iter()
                    .map(row_from_sqlite)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::MySql(p) => {
                let raw = sqlx::query(sql::SELECT_EXPIRED_QM)
                    .bind(cutoff)
                    .bind(batch)
                    .fetch_all(p)
                    .await?;
                raw.into_iter()
                    .map(row_from_mysql)
                    .collect::<Result<Vec<_>, _>>()?
            }
            DbPool::Postgres(p) => {
                let raw = sqlx::query(sql::SELECT_EXPIRED_PG)
                    .bind(cutoff)
                    .bind(batch)
                    .fetch_all(p)
                    .await?;
                raw.into_iter()
                    .map(row_from_postgres)
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        let mut n = 0u64;
        for entry in rows {
            if let Err(e) = self.purge_entry(&entry).await {
                tracing::warn!(error = %e, id = entry.id, "trash sweep: purge failed");
                continue;
            }
            n += 1;
        }
        Ok(n)
    }
}

/// Decoded slice of a trash row that the dialect-specific decoders all
/// agree on. Assembled by `assemble_row` into a typed `TrashEntry`.
struct RowParts {
    id: i64,
    user: String,
    basename: String,
    suffix: String,
    location: String,
    deleted_at: i64,
    type_str: String,
    fileid_legacy: Option<i64>,
}

fn assemble_row(parts: RowParts) -> Result<TrashEntry, TrashError> {
    let ty = TrashType::parse(&parts.type_str).ok_or_else(|| {
        TrashError::Db(sqlx::Error::Decode(
            format!("unknown trash type {:?}", parts.type_str).into(),
        ))
    })?;
    Ok(TrashEntry {
        id: parts.id,
        user: parts.user,
        basename: parts.basename,
        suffix: parts.suffix,
        location: parts.location,
        deleted_at: parts.deleted_at,
        r#type: ty,
        fileid_legacy: parts.fileid_legacy,
    })
}

fn row_from_sqlite(row: sqlx::sqlite::SqliteRow) -> Result<TrashEntry, TrashError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        user: row.try_get("user")?,
        basename: row.try_get("basename")?,
        suffix: row.try_get("suffix")?,
        location: row.try_get("location")?,
        deleted_at: row.try_get("deleted_at")?,
        type_str: row.try_get("type")?,
        fileid_legacy: row.try_get("fileid_legacy")?,
    })
}

fn row_from_mysql(row: sqlx::mysql::MySqlRow) -> Result<TrashEntry, TrashError> {
    // The table is created with `DEFAULT CHARSET=utf8mb4` (no `_bin`
    // collation), so string columns arrive as VARCHAR; plain `try_get`
    // is sufficient here.
    assemble_row(RowParts {
        id: row.try_get("id")?,
        user: row.try_get("user")?,
        basename: row.try_get("basename")?,
        suffix: row.try_get("suffix")?,
        location: row.try_get("location")?,
        deleted_at: row.try_get("deleted_at")?,
        type_str: row.try_get("type")?,
        fileid_legacy: row.try_get("fileid_legacy")?,
    })
}

fn row_from_postgres(row: sqlx::postgres::PgRow) -> Result<TrashEntry, TrashError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        user: row.try_get("user")?,
        basename: row.try_get("basename")?,
        suffix: row.try_get("suffix")?,
        location: row.try_get("location")?,
        deleted_at: row.try_get("deleted_at")?,
        type_str: row.try_get("type")?,
        fileid_legacy: row.try_get("fileid_legacy")?,
    })
}

/// Defense-in-depth check that a user-relative path is free of traversal
/// tricks before it gets joined with the user's data directory. The
/// current sole caller (`View::delete`) hands us validated `UserPath`
/// strings, but a future DAV/OCS handler might pass raw client input.
///
/// Rejects:
/// - any `..` path segment
/// - any backslash (Windows separator that `Path::join` honors)
/// - any NUL byte
/// - any absolute path (a leading `/` that survived `trim_start_matches`)
///
/// The empty string is accepted (means "root"). On failure we re-use
/// `TrashError::SourceMissing` rather than minting a new variant — the
/// caller can't usefully distinguish "this segment was `..`" from "this
/// file doesn't exist", and the check is defense-in-depth.
fn validate_relative_path(p: &str) -> Result<(), TrashError> {
    if p.contains('\\') || p.contains('\0') || p.starts_with('/') {
        return Err(TrashError::SourceMissing);
    }
    for seg in p.split('/') {
        if seg == ".." {
            return Err(TrashError::SourceMissing);
        }
    }
    Ok(())
}

/// Strip the last path segment. "a/b/c" -> "a/b", "a" -> "", "" -> "".
fn parent_of(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}

/// Find a free filename inside `target_dir_abs` starting with `basename`,
/// then `<basename> (restored)`, then `<basename> (restored 2)`, etc.
/// The ` (restored)` suffix is appended to the whole basename (after the
/// extension) so users see `report.pdf (restored)` rather than
/// `report (restored).pdf` — matches the spec §6 example.
/// Returns `(final_name, final_rel)` where `final_rel` is the full
/// user-relative path (e.g. "/foo/bar.txt (restored)").
async fn pick_non_colliding_name(
    target_dir_abs: &Path,
    target_dir_rel: &str,
    basename: &str,
) -> Result<(String, String), TrashError> {
    for n in 0..=RESTORE_COLLISION_CAP {
        let candidate = match n {
            0 => basename.to_string(),
            1 => format!("{basename} (restored)"),
            k => format!("{basename} (restored {k})"),
        };
        if !tokio::fs::try_exists(target_dir_abs.join(&candidate)).await? {
            let rel = if target_dir_rel.is_empty() {
                format!("/{candidate}")
            } else {
                format!("/{target_dir_rel}/{candidate}")
            };
            return Ok((candidate, rel));
        }
    }
    Err(TrashError::RestoreCollision)
}

/// Recursively copy the contents of `src` into `dst` (which must already
/// exist as an empty directory). Files are streamed via `tokio::io::copy`
/// and `sync_all`-ed before the next entry; subdirectories are
/// `create_dir`-ed and recursed into. Symlinks are intentionally NOT
/// followed — the spec is local-first and crabcloud's storage layer
/// doesn't expose symlinks as first-class entries, so any symlink we
/// encounter while walking is preserved as a regular file copy of its
/// target if the underlying fs follows it, or surfaces as an Io error
/// otherwise. Tested behavior is the no-symlinks case (the common one).
///
/// Recursion uses an explicit stack to avoid the boxed-future dance
/// that direct async recursion needs. Each frame holds one open
/// `ReadDir` iterator; we drain entries depth-first.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), TrashError> {
    // Frame: (src_dir, dst_dir, open ReadDir at src_dir).
    let mut stack: Vec<(PathBuf, PathBuf, tokio::fs::ReadDir)> = Vec::new();
    let initial = tokio::fs::read_dir(src).await?;
    stack.push((src.to_path_buf(), dst.to_path_buf(), initial));

    while let Some((src_dir, dst_dir, mut iter)) = stack.pop() {
        // Drain one entry at a time. On a subdirectory we push the
        // current frame back (with its updated iterator) before
        // descending so the depth-first traversal resumes correctly
        // after the child frame finishes.
        loop {
            let Some(entry) = iter.next_entry().await? else {
                break;
            };
            let file_type = entry.file_type().await?;
            let src_path = entry.path();
            let name = entry.file_name();
            let dst_path = dst_dir.join(&name);
            if file_type.is_dir() {
                tokio::fs::create_dir(&dst_path).await?;
                // Suspend the current frame, descend.
                stack.push((src_dir.clone(), dst_dir.clone(), iter));
                let child_iter = tokio::fs::read_dir(&src_path).await?;
                stack.push((src_path, dst_path, child_iter));
                break;
            } else if file_type.is_file() {
                let mut reader = tokio::fs::File::open(&src_path).await?;
                let mut writer = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dst_path)
                    .await?;
                tokio::io::copy(&mut reader, &mut writer).await?;
                writer.sync_all().await?;
            } else {
                // Symlinks / sockets / fifos: out of scope. The spec
                // assumes regular files + directories. Treat anything
                // else as a hard error so we don't silently lose data
                // by skipping it.
                return Err(TrashError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "unsupported file type at {} (not a regular file or directory)",
                        src_path.display()
                    ),
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The dev-dependencies `crabcloud-config` and `tempfile` are used by
    // the integration test at `tests/trash_e2e.rs`; the lib-test target
    // doesn't see them, so anchor them here to keep the
    // `unused_crate_dependencies` lint quiet.
    use crabcloud_config as _;
    use tempfile as _;

    #[test]
    fn parent_of_handles_root_and_nested() {
        assert_eq!(parent_of("foo.txt"), "");
        assert_eq!(parent_of("a/foo.txt"), "a");
        assert_eq!(parent_of("a/b/c/foo.txt"), "a/b/c");
        assert_eq!(parent_of(""), "");
    }

    #[test]
    fn validate_relative_path_rejects_traversal() {
        assert!(validate_relative_path("../etc/passwd").is_err());
        assert!(validate_relative_path("a/../b").is_err());
        assert!(validate_relative_path("a/b\\..").is_err());
        assert!(validate_relative_path("a\0b").is_err());
    }

    #[test]
    fn validate_relative_path_accepts_normal() {
        assert!(validate_relative_path("notes/todo.txt").is_ok());
        assert!(validate_relative_path("a/b/c").is_ok());
        assert!(validate_relative_path("file.txt").is_ok());
        assert!(validate_relative_path("").is_ok()); // root
    }
}
