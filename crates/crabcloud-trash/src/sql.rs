//! Multidialect SQL constants for the trash service.
//!
//! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres
//! (`$N`). Dispatch in `service.rs` via `match self.pool.as_ref()`.

// -- INSERT a new trash row. Returns id via RETURNING (pg) or
//    last_insert_rowid/last_insert_id (sqlite/mysql).
pub const INSERT_QM: &str = "\
    INSERT INTO oc_files_trash \
    (\"user\", basename, suffix, location, deleted_at, type, fileid_legacy) \
    VALUES (?, ?, ?, ?, ?, ?, ?)";

pub const INSERT_PG: &str = "\
    INSERT INTO oc_files_trash \
    (\"user\", basename, suffix, location, deleted_at, type, fileid_legacy) \
    VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id";

// -- LIST all entries for one user, most-recent-first.
pub const LIST_QM: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE \"user\" = ? ORDER BY deleted_at DESC";

pub const LIST_PG: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE \"user\" = $1 ORDER BY deleted_at DESC";

// -- GET one entry by id (used by restore + purge by-id).
pub const GET_BY_ID_QM: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE id = ?";

pub const GET_BY_ID_PG: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE id = $1";

// -- GET one entry by (user, basename, suffix) — used by DAV handlers
//    which receive the suffix-encoded filename.
pub const GET_BY_NAME_QM: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE \"user\" = ? AND basename = ? AND suffix = ?";

pub const GET_BY_NAME_PG: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE \"user\" = $1 AND basename = $2 AND suffix = $3";

// -- DELETE one row.
pub const DELETE_QM: &str = "DELETE FROM oc_files_trash WHERE id = ?";
pub const DELETE_PG: &str = "DELETE FROM oc_files_trash WHERE id = $1";

// -- DELETE all rows for a user (empty-trash). Reserved for Batch C
//    when the OCS `DELETE /trash` empty-bin handler lands; gated behind
//    `#[allow(dead_code)]` so clippy stays clean today.
#[allow(dead_code)]
pub const DELETE_ALL_QM: &str = "DELETE FROM oc_files_trash WHERE \"user\" = ?";
#[allow(dead_code)]
pub const DELETE_ALL_PG: &str = "DELETE FROM oc_files_trash WHERE \"user\" = $1";

// -- SELECT a batch of expired rows for sweeping.
pub const SELECT_EXPIRED_QM: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE deleted_at < ? LIMIT ?";

pub const SELECT_EXPIRED_PG: &str = "\
    SELECT id, \"user\", basename, suffix, location, deleted_at, type, fileid_legacy \
    FROM oc_files_trash WHERE deleted_at < $1 LIMIT $2";

// -- Sub-second collision probe: count suffixes matching prefix for a user.
//    Used when we need to bump `_2`, `_3`, ... on the same `dN` second.
pub const COUNT_SUFFIX_PREFIX_QM: &str = "\
    SELECT COUNT(*) AS n FROM oc_files_trash \
    WHERE \"user\" = ? AND basename = ? AND suffix LIKE ?";

pub const COUNT_SUFFIX_PREFIX_PG: &str = "\
    SELECT COUNT(*) AS n FROM oc_files_trash \
    WHERE \"user\" = $1 AND basename = $2 AND suffix LIKE $3";
