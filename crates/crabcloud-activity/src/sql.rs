//! Multidialect SQL constants for the activity service.
//!
//! `_QM` is sqlite + mysql (`?` placeholders); `_PG` is postgres (`$N`).

// -- INSERT a new activity row. Returns id via RETURNING (pg) or
//    last_insert_{rowid,id} (sqlite/mysql).
pub const INSERT_QM: &str = "\
    INSERT INTO oc_activity \
    (affected_user, actor, event_type, subject_id, subject_params, \
     object_type, object_id, occurred_at, last_seen_at, count) \
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)";

pub const INSERT_PG: &str = "\
    INSERT INTO oc_activity \
    (affected_user, actor, event_type, subject_id, subject_params, \
     object_type, object_id, occurred_at, last_seen_at, count) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1) RETURNING id";

// -- LIST rows for one user, descending id, with optional `since` cursor
//    (exclusive: id < since). Pass `since = 0` to disable the cursor.
pub const LIST_QM: &str = "\
    SELECT id, affected_user, actor, event_type, subject_id, subject_params, \
           object_type, object_id, occurred_at, last_seen_at, count \
    FROM oc_activity \
    WHERE affected_user = ? AND (? = 0 OR id < ?) \
    ORDER BY id DESC LIMIT ?";

pub const LIST_PG: &str = "\
    SELECT id, affected_user, actor, event_type, subject_id, subject_params, \
           object_type, object_id, occurred_at, last_seen_at, count \
    FROM oc_activity \
    WHERE affected_user = $1 AND ($2 = 0 OR id < $2) \
    ORDER BY id DESC LIMIT $3";

// -- COALESCE probe: most recent row matching (recipient, actor, event,
//    object_id) within last_seen_at >= cutoff. Used to decide INSERT vs
//    UPDATE in `emit`.
pub const COALESCE_PROBE_QM: &str = "\
    SELECT id FROM oc_activity \
    WHERE affected_user = ? AND actor = ? AND event_type = ? \
      AND ((object_id IS NULL AND ? IS NULL) OR object_id = ?) \
      AND last_seen_at >= ? \
    ORDER BY last_seen_at DESC LIMIT 1";

pub const COALESCE_PROBE_PG: &str = "\
    SELECT id FROM oc_activity \
    WHERE affected_user = $1 AND actor = $2 AND event_type = $3 \
      AND ((object_id IS NULL AND $4::BIGINT IS NULL) OR object_id = $4) \
      AND last_seen_at >= $5 \
    ORDER BY last_seen_at DESC LIMIT 1";

// -- COALESCE update: bump count + last_seen_at + subject_params.
pub const COALESCE_UPDATE_QM: &str = "\
    UPDATE oc_activity SET count = count + 1, last_seen_at = ?, subject_params = ? \
    WHERE id = ?";

pub const COALESCE_UPDATE_PG: &str = "\
    UPDATE oc_activity SET count = count + 1, last_seen_at = $1, subject_params = $2 \
    WHERE id = $3";

// -- DELETE expired rows.
pub const DELETE_EXPIRED_QM: &str = "DELETE FROM oc_activity WHERE occurred_at < ?";
pub const DELETE_EXPIRED_PG: &str = "DELETE FROM oc_activity WHERE occurred_at < $1";

// -- Settings: GET single toggle, GET all for user, UPSERT toggle.
pub const SETTINGS_GET_QM: &str = "\
    SELECT stream FROM oc_activity_settings WHERE user_id = ? AND event_type = ?";

pub const SETTINGS_GET_PG: &str = "\
    SELECT stream FROM oc_activity_settings WHERE user_id = $1 AND event_type = $2";

pub const SETTINGS_GET_ALL_QM: &str = "\
    SELECT event_type, stream FROM oc_activity_settings WHERE user_id = ?";

pub const SETTINGS_GET_ALL_PG: &str = "\
    SELECT event_type, stream FROM oc_activity_settings WHERE user_id = $1";

// -- UPSERT — per-dialect; sqlite uses ON CONFLICT, mysql uses
//    ON DUPLICATE KEY UPDATE, postgres uses ON CONFLICT.
pub const SETTINGS_UPSERT_SQLITE: &str = "\
    INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES (?, ?, ?) \
    ON CONFLICT (user_id, event_type) DO UPDATE SET stream = excluded.stream";

pub const SETTINGS_UPSERT_MYSQL: &str = "\
    INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES (?, ?, ?) \
    ON DUPLICATE KEY UPDATE stream = VALUES(stream)";

pub const SETTINGS_UPSERT_PG: &str = "\
    INSERT INTO oc_activity_settings (user_id, event_type, stream) VALUES ($1, $2, $3) \
    ON CONFLICT (user_id, event_type) DO UPDATE SET stream = excluded.stream";
