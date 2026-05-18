//! Multidialect SQL constants for the search service.
//!
//! Per-dialect because the full-text mechanism differs substantially:
//!   - sqlite: FTS5 virtual table; `MATCH ?` syntax with FTS5 query string
//!   - mysql: `MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE)`
//!   - postgres: `tsv @@ plainto_tsquery('simple', ?)` with `ts_rank_cd`
//!
//! Query templates here are BASE templates; `service.rs` appends optional
//! filter / cursor AND clauses dynamically before binding.

// -- DELETE one (viewer, fileid) row.
pub const DELETE_VIEWER_FILE_QM: &str = "DELETE FROM oc_search WHERE viewer_uid = ? AND fileid = ?";
pub const DELETE_VIEWER_FILE_PG: &str =
    "DELETE FROM oc_search WHERE viewer_uid = $1 AND fileid = $2";

// -- INSERT (used by sqlite DELETE-then-INSERT; FTS5 has no UPSERT).
pub const INSERT_QM: &str = "INSERT INTO oc_search \
     (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

pub const INSERT_MYSQL_UPSERT: &str = "INSERT INTO oc_search \
     (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
     ON DUPLICATE KEY UPDATE \
       storage_id = VALUES(storage_id), basename = VALUES(basename), \
       path = VALUES(path), mime = VALUES(mime), \
       mtime = VALUES(mtime), size = VALUES(size)";

pub const INSERT_PG_UPSERT: &str = "INSERT INTO oc_search \
     (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
     ON CONFLICT (viewer_uid, fileid) DO UPDATE SET \
       storage_id = EXCLUDED.storage_id, basename = EXCLUDED.basename, \
       path = EXCLUDED.path, mime = EXCLUDED.mime, \
       mtime = EXCLUDED.mtime, size = EXCLUDED.size";

// -- DELETE all rows for one fileid (every viewer). Used on hard-delete /
//    soft-delete-to-trash.
pub const DELETE_FILEID_QM: &str = "DELETE FROM oc_search WHERE fileid = ?";
pub const DELETE_FILEID_PG: &str = "DELETE FROM oc_search WHERE fileid = $1";

// -- Lookup any fileid for a (storage_id, path) — used by the indexer's
//    Deleted handler when StorageEvent doesn't carry the fileid.
//    LIMIT 1: we just need ONE row to discover the fileid; the caller
//    follows up with DELETE_FILEID_* to cascade across all viewers.
pub const LOOKUP_FILEID_BY_STORAGE_PATH_QM: &str =
    "SELECT fileid FROM oc_search WHERE storage_id = ? AND path = ? LIMIT 1";
pub const LOOKUP_FILEID_BY_STORAGE_PATH_PG: &str =
    "SELECT fileid FROM oc_search WHERE storage_id = $1 AND path = $2 LIMIT 1";

// -- QUERY base templates.
// sqlite (FTS5): `MATCH ?` predicate on the virtual table; rank via
// `bm25(oc_search)` (lower = better; ORDER BY rank ASC).
pub const QUERY_BASE_SQLITE: &str =
    "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
            bm25(oc_search) AS rank \
     FROM oc_search \
     WHERE viewer_uid = ? AND oc_search MATCH ?";

// mysql: NATURAL LANGUAGE MODE. Rank via the MATCH score (higher = better);
// ORDER BY rank DESC.
pub const QUERY_BASE_MYSQL: &str = "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
            MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) AS rank \
     FROM oc_search \
     WHERE viewer_uid = ? \
       AND MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE)";

// postgres: `@@` with `plainto_tsquery` + `ts_rank_cd`.
pub const QUERY_BASE_PG: &str = "SELECT fileid, storage_id, basename, path, mime, mtime, size, \
            ts_rank_cd(tsv, plainto_tsquery('simple', $1)) AS rank \
     FROM oc_search \
     WHERE viewer_uid = $2 \
       AND tsv @@ plainto_tsquery('simple', $1)";
