//! SQL constants for the `Shares` service. Per-dialect pairs follow the
//! `crabcloud-filecache::propagate` convention: `_QM` is sqlite + mysql
//! (positional `?`), `_PG` is postgres (`$1..$N`).

/// Documentation-only constant. Every SELECT below names these 17 columns
/// in this order; row decoders rely on `try_get` by name, so the constant
/// is not interpolated, but the listed names are the contract.
pub(crate) const SELECT_COLUMNS: &str = "id, share_type, share_with, uid_owner, uid_initiator, \
    parent, item_type, item_source, file_source, file_target, permissions, stime, accepted, \
    expiration, token, password, mail_send";

/// Documentation-only constant for the INSERT column list. `INSERT_QM` and
/// `INSERT_PG` bind in this order. `id` is omitted: the database mints it
/// (AUTOINCREMENT / AUTO_INCREMENT / BIGSERIAL).
pub(crate) const INSERT_BIND_LIST: &str = "share_type, share_with, uid_owner, uid_initiator, \
    parent, item_type, item_source, file_source, file_target, permissions, stime, accepted, \
    expiration, token, password, mail_send";

pub(crate) const SELECT_BY_ID_QM: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE id = ?";

pub(crate) const SELECT_BY_ID_PG: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE id = $1";

pub(crate) const SELECT_OUTGOING_QM: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE uid_owner = ? ORDER BY id";

pub(crate) const SELECT_OUTGOING_PG: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE uid_owner = $1 ORDER BY id";

pub(crate) const SELECT_FOR_OWNER_AND_SOURCE_QM: &str = "SELECT id, share_type, share_with, \
    uid_owner, uid_initiator, parent, item_type, item_source, file_source, file_target, \
    permissions, stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE uid_owner = ? AND file_source = ? ORDER BY id";

pub(crate) const SELECT_FOR_OWNER_AND_SOURCE_PG: &str = "SELECT id, share_type, share_with, \
    uid_owner, uid_initiator, parent, item_type, item_source, file_source, file_target, \
    permissions, stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE uid_owner = $1 AND file_source = $2 ORDER BY id";

pub(crate) const DELETE_BY_ID_QM: &str = "DELETE FROM oc_share WHERE id = ?";
pub(crate) const DELETE_BY_ID_PG: &str = "DELETE FROM oc_share WHERE id = $1";

pub(crate) const UNACCEPT_BY_ID_QM: &str = "UPDATE oc_share SET accepted = 0 WHERE id = ?";
pub(crate) const UNACCEPT_BY_ID_PG: &str = "UPDATE oc_share SET accepted = 0 WHERE id = $1";

pub(crate) const INSERT_QM: &str = "INSERT INTO oc_share \
    (share_type, share_with, uid_owner, uid_initiator, parent, item_type, item_source, \
     file_source, file_target, permissions, stime, accepted, expiration, token, password, \
     mail_send) \
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_PG: &str = "INSERT INTO oc_share \
    (share_type, share_with, uid_owner, uid_initiator, parent, item_type, item_source, \
     file_source, file_target, permissions, stime, accepted, expiration, token, password, \
     mail_send) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
    RETURNING id";

pub(crate) const UPDATE_PERMISSIONS_QM: &str = "UPDATE oc_share SET permissions = ? WHERE id = ?";
pub(crate) const UPDATE_PERMISSIONS_PG: &str = "UPDATE oc_share SET permissions = $1 WHERE id = $2";

pub(crate) const UPDATE_EXPIRATION_QM: &str = "UPDATE oc_share SET expiration = ? WHERE id = ?";
pub(crate) const UPDATE_EXPIRATION_PG: &str = "UPDATE oc_share SET expiration = $1 WHERE id = $2";

/// Reference each constant so unused-const warnings stay quiet across batches
/// (some are only consumed once Batch B's CRUD impls land below).
const _: &str = SELECT_COLUMNS;
const _: &str = INSERT_BIND_LIST;

/// Build the dynamic `share_counts_for` query for an owner with `fileid_count`
/// candidate fileids. Returns `(file_source, count)` rows for every fileid in
/// the input list that has at least one outgoing share owned by the requester.
/// Caller must guarantee `fileid_count > 0`; empty input short-circuits before
/// reaching the DB. Placeholder layout: owner uid is the first bind; the
/// remaining `fileid_count` binds fill the `IN (…)` list in order.
pub(crate) fn share_counts_for(fileid_count: usize, dialect: Dialect) -> String {
    debug_assert!(fileid_count > 0);
    let mut q = String::with_capacity(160 + fileid_count * 4);
    q.push_str("SELECT file_source, COUNT(*) AS cnt FROM oc_share WHERE uid_owner = ");
    match dialect {
        Dialect::Qm => q.push('?'),
        Dialect::Pg => q.push_str("$1"),
    }
    q.push_str(" AND file_source IN (");
    match dialect {
        Dialect::Qm => {
            for i in 0..fileid_count {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('?');
            }
        }
        Dialect::Pg => {
            for i in 0..fileid_count {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('$');
                q.push_str(&(i + 2).to_string());
            }
        }
    }
    q.push_str(") GROUP BY file_source");
    q
}

/// Build the dynamic `list_incoming` query for a recipient with `group_count`
/// group memberships. Placeholder style follows `dialect`: `qm` (sqlite + mysql)
/// uses `?`, `pg` uses `$N` numbering starting at `$1` for the recipient uid
/// and continuing through `$2..$(group_count+1)` for the group names.
pub(crate) fn select_incoming(group_count: usize, dialect: Dialect) -> String {
    let mut q = String::with_capacity(512);
    q.push_str(
        "SELECT id, share_type, share_with, uid_owner, uid_initiator, parent, item_type, \
         item_source, file_source, file_target, permissions, stime, accepted, expiration, \
         token, password, mail_send FROM oc_share WHERE accepted = 1 AND share_type IN (0, 1) \
         AND (",
    );
    match dialect {
        Dialect::Qm => {
            q.push_str("(share_type = 0 AND share_with = ?)");
            if group_count > 0 {
                q.push_str(" OR (share_type = 1 AND share_with IN (");
                for i in 0..group_count {
                    if i > 0 {
                        q.push_str(", ");
                    }
                    q.push('?');
                }
                q.push_str("))");
            }
        }
        Dialect::Pg => {
            q.push_str("(share_type = 0 AND share_with = $1)");
            if group_count > 0 {
                q.push_str(" OR (share_type = 1 AND share_with IN (");
                for i in 0..group_count {
                    if i > 0 {
                        q.push_str(", ");
                    }
                    q.push('$');
                    q.push_str(&(i + 2).to_string());
                }
                q.push_str("))");
            }
        }
    }
    q.push_str(") ORDER BY id");
    q
}

/// SQL placeholder style. `Qm` covers sqlite + mysql (positional `?`); `Pg`
/// covers postgres (numbered `$N`).
///
/// Intentionally distinct from `crabcloud_db::Dialect` — that enum models the
/// engine (three variants); this one models placeholder syntax (two), since
/// the only thing `select_incoming` cares about is how to write `?` vs `$2`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Dialect {
    Qm,
    Pg,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_incoming_qm_no_groups() {
        let q = select_incoming(0, Dialect::Qm);
        assert!(q.contains("share_with = ?"));
        assert!(!q.contains("share_type = 1 AND share_with IN"));
        assert!(q.ends_with("ORDER BY id"));
    }

    #[test]
    fn select_incoming_qm_with_groups() {
        let q = select_incoming(3, Dialect::Qm);
        assert!(q.contains("share_type = 0 AND share_with = ?"));
        assert!(q.contains("share_type = 1 AND share_with IN (?, ?, ?)"));
    }

    #[test]
    fn select_incoming_pg_no_groups() {
        let q = select_incoming(0, Dialect::Pg);
        assert!(q.contains("share_with = $1"));
        assert!(!q.contains("$2"));
    }

    #[test]
    fn select_incoming_pg_with_groups() {
        let q = select_incoming(2, Dialect::Pg);
        assert!(q.contains("share_with = $1"));
        assert!(q.contains("share_with IN ($2, $3)"));
    }

    #[test]
    fn share_counts_for_qm_single() {
        let q = share_counts_for(1, Dialect::Qm);
        assert!(q.contains("uid_owner = ?"));
        assert!(q.contains("file_source IN (?)"));
        assert!(q.ends_with("GROUP BY file_source"));
    }

    #[test]
    fn share_counts_for_qm_many() {
        let q = share_counts_for(3, Dialect::Qm);
        assert!(q.contains("file_source IN (?, ?, ?)"));
    }

    #[test]
    fn share_counts_for_pg_single() {
        let q = share_counts_for(1, Dialect::Pg);
        assert!(q.contains("uid_owner = $1"));
        assert!(q.contains("file_source IN ($2)"));
    }

    #[test]
    fn share_counts_for_pg_many() {
        let q = share_counts_for(3, Dialect::Pg);
        assert!(q.contains("file_source IN ($2, $3, $4)"));
    }
}
