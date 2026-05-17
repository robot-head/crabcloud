//! Multidialect SQL constants for the versions service.
//!
//! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres (`$N`).
//! Dispatch in `service.rs` via `match self.pool.as_ref()`.

// -- INSERT a new version row. Returns id via RETURNING (pg) or
//    last_insert_rowid/last_insert_id (sqlite/mysql).
pub const INSERT_QM: &str = "\
    INSERT INTO oc_files_versions \
    (storage_id, fileid, \"user\", path, version_mtime, size) \
    VALUES (?, ?, ?, ?, ?, ?)";

pub const INSERT_PG: &str = "\
    INSERT INTO oc_files_versions \
    (storage_id, fileid, \"user\", path, version_mtime, size) \
    VALUES ($1, $2, $3, $4, $5, $6) RETURNING id";

// -- LIST all versions for a (user, fileid), newest-first.
pub const LIST_FOR_QM: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE \"user\" = ? AND fileid = ? \
    ORDER BY version_mtime DESC";

pub const LIST_FOR_PG: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE \"user\" = $1 AND fileid = $2 \
    ORDER BY version_mtime DESC";

// -- GET one by id (restore + delete + cascade lookup).
pub const GET_BY_ID_QM: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE id = ?";

pub const GET_BY_ID_PG: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE id = $1";

// -- GET most-recent version for throttle check.
pub const GET_LATEST_FOR_QM: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions \
    WHERE storage_id = ? AND fileid = ? \
    ORDER BY version_mtime DESC LIMIT 1";

pub const GET_LATEST_FOR_PG: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions \
    WHERE storage_id = $1 AND fileid = $2 \
    ORDER BY version_mtime DESC LIMIT 1";

// -- DELETE one row by id.
pub const DELETE_QM: &str = "DELETE FROM oc_files_versions WHERE id = ?";
pub const DELETE_PG: &str = "DELETE FROM oc_files_versions WHERE id = $1";

// -- LIST distinct (user, fileid) pairs for the tiered sweeper. Used to
//    drive per-file bucket classification.
pub const LIST_GROUPS_QM: &str = "\
    SELECT DISTINCT \"user\", fileid FROM oc_files_versions \
    ORDER BY \"user\", fileid";

pub const LIST_GROUPS_PG: &str = "\
    SELECT DISTINCT \"user\", fileid FROM oc_files_versions \
    ORDER BY \"user\", fileid";

// -- LIST all version rows for a (storage_id, fileid). Used for
//    purge_for_fileid (storage_id-keyed cascade).
pub const LIST_FOR_FILEID_QM: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE storage_id = ? AND fileid = ?";

pub const LIST_FOR_FILEID_PG: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE storage_id = $1 AND fileid = $2";

// -- LIST all version rows for a (user, fileid). Used by the trash
//    cascade path where we know the uid (from the trash row) but not the
//    owner home's numeric storage_id.
pub const LIST_FOR_USER_FILEID_QM: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE \"user\" = ? AND fileid = ?";

pub const LIST_FOR_USER_FILEID_PG: &str = "\
    SELECT id, storage_id, fileid, \"user\", path, version_mtime, size \
    FROM oc_files_versions WHERE \"user\" = $1 AND fileid = $2";
