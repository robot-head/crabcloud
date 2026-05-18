//! `Shares` — sharing CRUD service. SP7 spec sections §5 (surface) and §9
//! (auth/permissions). Schema lives in migration `0006_shares`.
//!
//! Sibling modules host the bits that aren't core CRUD/token-resolve so
//! this file stays focused:
//! - `notifications` — best-effort mail hooks (`share_created`,
//!   `link_emailed`) invoked post-insert.
//! - `sweeper_support` — `find_expiring_links` + `stamp_last_warned`
//!   used by `ExpirationWarningSweeper`.

mod notifications;
mod sweeper_support;

use chrono::{DateTime, NaiveDateTime, Utc};
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_storage::StoragePath;
use crabcloud_users::{GroupId, NotificationPrefs, UserId, UsersService};
use sqlx::Row as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ShareError;
use crate::mail::MailEnqueuer;
use crate::permissions::SharePermissions;
use crate::sql::{self, Dialect};
use crate::types::{
    CreateShareRequest, ItemType, ShareFanoutContext, ShareRow, ShareType, UpdateShareFields,
};

#[derive(Clone)]
pub struct Shares {
    pub(crate) pool: Arc<DbPool>,
    pub(crate) users: Arc<UsersService>,
    pub(crate) filecache: Arc<FileCache>,
    pub(crate) mail: Arc<dyn MailEnqueuer>,
    pub(crate) prefs: NotificationPrefs,
    /// Base URL the templates use to build absolute links
    /// (e.g. `https://crabcloud.example`). Falls back to `/s/<token>`
    /// relative when not set.
    pub(crate) instance_url: String,
    /// Activity emitter. Best-effort; failures are logged + swallowed
    /// because the user-visible share create/delete must not be rolled
    /// back by a down activity log (SP14 spec §6 emit-failure row).
    pub(crate) activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
    /// Search-index fan-out. Best-effort; failures are logged + swallowed
    /// so a misbehaving search index can't block a share create / delete
    /// (SP15 spec §6 — same eventually-consistent contract as activity).
    pub(crate) search: Arc<dyn crabcloud_search::SearchFanout>,
}

/// Construction parameters for `Shares::new`.
///
/// Workspace-internal callers are expected to use struct-literal form so each
/// field is named at the call site (avoids silent positional swaps among the
/// several `Arc<...>` parameters). Add new fields with care — every call site
/// will need updating.
pub struct SharesConfig {
    pub pool: Arc<DbPool>,
    pub users: Arc<UsersService>,
    pub filecache: Arc<FileCache>,
    pub mail: Arc<dyn MailEnqueuer>,
    pub prefs: NotificationPrefs,
    /// Base URL inserted into mail templates' `{{ link_url }}` (so callers
    /// see `https://host/s/<token>`). Empty string is tolerated — the
    /// templates degrade to relative `/s/<token>` URLs.
    pub instance_url: String,
    /// Activity emitter for `share_created` / `share_deleted` events.
    /// Tests can pass `Arc::new(crabcloud_activity::NoopEmitter)` to
    /// skip activity logging.
    pub activity: Arc<dyn crabcloud_activity::ActivityEmitter>,
    /// Search-index fan-out. Tests can pass
    /// `Arc::new(crabcloud_search::NoopSearchFanout)` to skip search
    /// indexing.
    pub search: Arc<dyn crabcloud_search::SearchFanout>,
}

impl Shares {
    /// Construct a `Shares` service from a `SharesConfig`.
    pub fn new(cfg: SharesConfig) -> Self {
        Self {
            pool: cfg.pool,
            users: cfg.users,
            filecache: cfg.filecache,
            mail: cfg.mail,
            prefs: cfg.prefs,
            instance_url: cfg.instance_url,
            activity: cfg.activity,
            search: cfg.search,
        }
    }

    pub async fn create(&self, req: CreateShareRequest) -> Result<ShareRow, ShareError> {
        // Link + Email rows take a different code path: no user/group
        // recipient lookup, password and expiration handled, token
        // generated. Email additionally enqueues a `link_emailed` mail
        // post-insert (Task C4). SP8 §5.
        if matches!(req.share_type, ShareType::Link | ShareType::Email) {
            return self.create_link(req).await;
        }
        if req.permissions & 1 == 0 {
            return Err(ShareError::BadPermissions);
        }
        let perms = SharePermissions::from_wire(req.permissions);

        let storage_path = parse_wire_path(&req.path)?;
        let row = self
            .filecache
            .lookup(&req.home_storage_id, &storage_path)
            .await
            .map_err(map_filecache)?
            .ok_or(ShareError::PathNotOwned)?;
        if row.storage_id != req.home_storage_id {
            return Err(ShareError::ReshareRejected);
        }

        match req.share_type {
            ShareType::User => {
                let uid = UserId::new(req.share_with.clone())
                    .map_err(|_| ShareError::RecipientUnknown)?;
                if self
                    .users
                    .user_store()
                    .lookup(&uid)
                    .await
                    .map_err(map_users)?
                    .is_none()
                {
                    return Err(ShareError::RecipientUnknown);
                }
            }
            ShareType::Group => {
                let gid = GroupId::new(req.share_with.clone())
                    .map_err(|_| ShareError::RecipientUnknown)?;
                if self
                    .users
                    .group_store()
                    .lookup(&gid)
                    .await
                    .map_err(map_users)?
                    .is_none()
                {
                    return Err(ShareError::RecipientUnknown);
                }
            }
            ShareType::Link | ShareType::Email => unreachable!(),
        }

        let item_type = if row.mimetype.as_str() == crabcloud_filecache::DIRECTORY_MIMETYPE {
            ItemType::Folder
        } else {
            ItemType::File
        };
        let basename = storage_path.basename();
        let file_target = format!("/{basename}");
        let stime = unix_now();
        let share_type_db: i16 = req.share_type.into();
        let perms_db = perms.as_u32() as i32;
        let fileid = row.fileid;

        let id = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind(&req.share_with)
                    .bind(&req.requester)
                    .bind(&req.requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(&file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .bind::<Option<String>>(None)
                    .bind::<Option<String>>(None)
                    .bind(0_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .execute(p)
                    .await?;
                res.last_insert_rowid()
            }
            DbPool::MySql(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind(&req.share_with)
                    .bind(&req.requester)
                    .bind(&req.requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(&file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .bind::<Option<String>>(None)
                    .bind::<Option<String>>(None)
                    .bind(0_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .execute(p)
                    .await?;
                res.last_insert_id() as i64
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(share_type_db)
                    .bind(&req.share_with)
                    .bind(&req.requester)
                    .bind(&req.requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(&file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .bind::<Option<String>>(None)
                    .bind::<Option<String>>(None)
                    .bind(0_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .fetch_one(p)
                    .await?;
                row.try_get::<i64, _>("id")?
            }
        };

        // Post-insert notification hook. Only User shares mail the
        // recipient; group shares would require fan-out to N members
        // and are deferred. Failures are logged + dropped — the share
        // itself succeeded.
        if matches!(req.share_type, ShareType::User) {
            self.try_enqueue_share_created_mail(
                req.share_with.as_str(),
                req.requester.as_str(),
                &storage_path,
            )
            .await;
        }

        // Best-effort activity emit (SP14). Failures are logged.
        self.emit_share_activity(
            crabcloud_activity::EventType::ShareCreated,
            "share_created_you",
            "share_created_by",
            id,
            req.requester.as_str(),
            req.share_type,
            Some(req.share_with.as_str()),
            req.path.as_str(),
        )
        .await;

        // Best-effort search fan-out (SP15). Failures are logged + dropped
        // so the user-visible share-create still succeeds. The recipient
        // list mirrors `share_activity_recipients` minus the actor.
        let recipients = self
            .search_fanout_recipients(req.share_type, Some(req.share_with.as_str()))
            .await;
        if !recipients.is_empty() {
            let recipient_prefix = format!("/{}", file_target.trim_start_matches('/'));
            let owner_path = format!("/{}", req.path.trim_start_matches('/'));
            if let Err(e) = self
                .search
                .fan_out_for_share(
                    &self.filecache,
                    recipients,
                    &req.home_storage_id,
                    &owner_path,
                    &recipient_prefix,
                )
                .await
            {
                tracing::warn!(error = %e, share_id = id, "sharing: search fan_out_for_share failed");
            }
        }

        Ok(ShareRow {
            id,
            share_type: req.share_type,
            share_with: Some(req.share_with),
            uid_owner: req.requester.clone(),
            uid_initiator: req.requester,
            parent: None,
            item_type,
            item_source: fileid,
            file_source: fileid,
            file_target,
            permissions: perms,
            stime,
            accepted: true,
            expiration: None,
            token: None,
            password_hash: None,
            last_warned: None,
        })
    }

    pub async fn get(&self, id: i64) -> Result<Option<ShareRow>, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::SELECT_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::SELECT_BY_ID_QM)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::SELECT_BY_ID_PG)
                    .bind(id)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres).transpose()
            }
        }
    }

    /// Look up a public-link share row by its token. Filters on
    /// `share_type IN (3, 4)` so both Link and Email rows are returned —
    /// email-link recipients open the share via `/s/<token>` exactly like
    /// regular link recipients. Returns `None` for unknown / non-link
    /// rows. Does NOT enforce expiration — the caller must compare
    /// `expiration` to `now()` and treat past-expired as missing. SP8 §5.
    pub async fn resolve_by_token(&self, token: &str) -> Result<Option<ShareRow>, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_QM)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_QM)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_PG)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres).transpose()
            }
        }
    }

    pub async fn list_outgoing(&self, owner: &UserId) -> Result<Vec<ShareRow>, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::SELECT_OUTGOING_QM)
                    .bind(owner.as_str())
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::SELECT_OUTGOING_QM)
                    .bind(owner.as_str())
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::SELECT_OUTGOING_PG)
                    .bind(owner.as_str())
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    /// List the outgoing shares `owner` has created on `path`. `home_storage_id`
    /// is the namespace under which the filecache lookup is scoped — caller
    /// derives it via the storage factory (e.g. `storage_factory.home_storage(owner)
    /// .id()`). This crate doesn't depend on `crabcloud-fs`, so the id can't be
    /// computed here.
    pub async fn list_for_owner_path(
        &self,
        owner: &UserId,
        home_storage_id: &str,
        path: &str,
    ) -> Result<Vec<ShareRow>, ShareError> {
        let storage_path = parse_wire_path(path)?;
        let fcrow = self
            .filecache
            .lookup(home_storage_id, &storage_path)
            .await
            .map_err(map_filecache)?
            .ok_or(ShareError::NotFound)?;
        let fileid = fcrow.fileid;
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let rows = sqlx::query(sql::SELECT_FOR_OWNER_AND_SOURCE_QM)
                    .bind(owner.as_str())
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let rows = sqlx::query(sql::SELECT_FOR_OWNER_AND_SOURCE_QM)
                    .bind(owner.as_str())
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let rows = sqlx::query(sql::SELECT_FOR_OWNER_AND_SOURCE_PG)
                    .bind(owner.as_str())
                    .bind(fileid)
                    .fetch_all(p)
                    .await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    pub async fn list_incoming(&self, recipient: &UserId) -> Result<Vec<ShareRow>, ShareError> {
        let groups = self.users.groups_of(recipient).await.map_err(map_users)?;
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let q_text = sql::select_incoming(groups.len(), Dialect::Qm);
                let mut q = sqlx::query(&q_text).bind(recipient.as_str());
                for g in &groups {
                    q = q.bind(g.as_str().to_string());
                }
                let rows = q.fetch_all(p).await?;
                rows.into_iter().map(row_from_sqlite).collect()
            }
            DbPool::MySql(p) => {
                let q_text = sql::select_incoming(groups.len(), Dialect::Qm);
                let mut q = sqlx::query(&q_text).bind(recipient.as_str());
                for g in &groups {
                    q = q.bind(g.as_str().to_string());
                }
                let rows = q.fetch_all(p).await?;
                rows.into_iter().map(row_from_mysql).collect()
            }
            DbPool::Postgres(p) => {
                let q_text = sql::select_incoming(groups.len(), Dialect::Pg);
                let mut q = sqlx::query(&q_text).bind(recipient.as_str());
                for g in &groups {
                    q = q.bind(g.as_str().to_string());
                }
                let rows = q.fetch_all(p).await?;
                rows.into_iter().map(row_from_postgres).collect()
            }
        }
    }

    /// Bulk-count outgoing shares for a fixed set of fileids, scoped to
    /// `owner`. Returns a map `file_source → count` containing only the
    /// fileids that have at least one share row (callers default missing
    /// keys to 0). Empty `fileids` returns an empty map without hitting
    /// the database. Used by the Files UI to render `🔗 N` chips next
    /// to owner-side rows in one batched query per listing.
    pub async fn share_counts_for(
        &self,
        owner: &UserId,
        fileids: &[i64],
    ) -> Result<HashMap<i64, i64>, ShareError> {
        if fileids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out: HashMap<i64, i64> = HashMap::new();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let q_text = sql::share_counts_for(fileids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text).bind(owner.as_str());
                for id in fileids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for row in rows {
                    let fid: i64 = row.try_get("file_source")?;
                    let cnt: i64 = row.try_get("cnt")?;
                    out.insert(fid, cnt);
                }
            }
            DbPool::MySql(p) => {
                let q_text = sql::share_counts_for(fileids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text).bind(owner.as_str());
                for id in fileids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for row in rows {
                    let fid: i64 = row.try_get("file_source")?;
                    // MySQL COUNT(*) decodes as i64 on the wire.
                    let cnt: i64 = row.try_get("cnt")?;
                    out.insert(fid, cnt);
                }
            }
            DbPool::Postgres(p) => {
                let q_text = sql::share_counts_for(fileids.len(), sql::Dialect::Pg);
                let mut q = sqlx::query(&q_text).bind(owner.as_str());
                for id in fileids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for row in rows {
                    let fid: i64 = row.try_get("file_source")?;
                    let cnt: i64 = row.try_get("cnt")?;
                    out.insert(fid, cnt);
                }
            }
        }
        Ok(out)
    }

    pub async fn update(
        &self,
        id: i64,
        requester: &UserId,
        fields: UpdateShareFields,
    ) -> Result<ShareRow, ShareError> {
        let existing = self.get(id).await?.ok_or(ShareError::NotFound)?;
        if existing.uid_owner != requester.as_str() {
            return Err(ShareError::Forbidden);
        }
        if fields.note.is_some() {
            return Err(ShareError::NotImplemented);
        }
        if let Some(pw_opt) = &fields.password {
            // Only Link / Email rows accept password updates.
            if !matches!(existing.share_type, ShareType::Link | ShareType::Email) {
                return Err(ShareError::BadPermissions);
            }
            let hashed: Option<String> = match pw_opt {
                Some(pw) => Some(
                    crabcloud_publiclinks::Passwords::new()
                        .hash(pw)
                        .map_err(|_| {
                            ShareError::DbError(sqlx::Error::Protocol(
                                "password hash failed".into(),
                            ))
                        })?
                        .as_str()
                        .to_string(),
                ),
                None => None,
            };
            run_update_password(&self.pool, id, hashed.as_deref()).await?;
        }
        if let Some(raw) = fields.permissions {
            // Link rows accept either bit 1 (read) or bit 4 (create), matching
            // `create_link`. User/group rows continue to require bit 1.
            let ok = if matches!(existing.share_type, ShareType::Link | ShareType::Email) {
                raw & 1 != 0 || raw & 4 != 0
            } else {
                raw & 1 != 0
            };
            if !ok {
                return Err(ShareError::BadPermissions);
            }
            let perms = SharePermissions::from_wire(raw);
            run_update_permissions(&self.pool, id, perms.as_u32() as i32).await?;
        }
        if let Some(date_opt) = fields.expire_date {
            let naive: Option<NaiveDateTime> = date_opt.and_then(|d| d.and_hms_opt(0, 0, 0));
            run_update_expiration(&self.pool, id, naive).await?;
        }
        self.get(id).await?.ok_or(ShareError::NotFound)
    }

    pub async fn delete(&self, id: i64, requester: &UserId) -> Result<(), ShareError> {
        let row = self.get(id).await?.ok_or(ShareError::NotFound)?;
        let is_owner = row.uid_owner == requester.as_str();
        let is_direct = matches!(
            (&row.share_type, row.share_with.as_deref()),
            (ShareType::User, Some(s)) if s == requester.as_str()
        );
        let is_group_recipient =
            if let (ShareType::Group, Some(gname)) = (&row.share_type, row.share_with.as_deref()) {
                self.users
                    .groups_of(requester)
                    .await
                    .map_err(map_users)?
                    .iter()
                    .any(|g| g.as_str() == gname)
            } else {
                false
            };

        if is_owner {
            match self.pool.as_ref() {
                DbPool::Sqlite(p) => {
                    sqlx::query(sql::DELETE_BY_ID_QM)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
                DbPool::MySql(p) => {
                    sqlx::query(sql::DELETE_BY_ID_QM)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
                DbPool::Postgres(p) => {
                    sqlx::query(sql::DELETE_BY_ID_PG)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
            }
            // Best-effort activity emit (SP14). Use the actor as the
            // requester (the owner driving the delete).
            self.emit_share_activity(
                crabcloud_activity::EventType::ShareDeleted,
                "share_deleted_you",
                "share_deleted_by",
                id,
                requester.as_str(),
                row.share_type,
                row.share_with.as_deref(),
                row.file_target.as_str(),
            )
            .await;

            // Best-effort search fan-out unshare. Walks the same owner
            // subroot and removes recipient rows. We have to look the
            // owner subroot path up via the filecache (the share row
            // stores fileid in item_source, not the path string).
            let recipients = self
                .search_fanout_recipients(row.share_type, row.share_with.as_deref())
                .await;
            if !recipients.is_empty() {
                match self.filecache.lookup_by_id(row.item_source).await {
                    Ok(Some(fc_row)) => {
                        let owner_subroot = format!("/{}", fc_row.path.as_str());
                        // Owner-storage id is the storage id stamped on
                        // the filecache row itself.
                        if let Err(e) = self
                            .search
                            .fan_out_for_unshare(
                                &self.filecache,
                                recipients,
                                &fc_row.storage_id,
                                &owner_subroot,
                            )
                            .await
                        {
                            tracing::warn!(
                                error = %e,
                                share_id = id,
                                "sharing: search fan_out_for_unshare failed"
                            );
                        }
                    }
                    Ok(None) => {
                        tracing::debug!(
                            share_id = id,
                            item_source = row.item_source,
                            "sharing: search fan-out unshare skipped (filecache row missing)"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            share_id = id,
                            "sharing: search fan-out unshare filecache lookup failed"
                        );
                    }
                }
            }
            Ok(())
        } else if is_direct || is_group_recipient {
            if !row.accepted {
                return Err(ShareError::NotFound);
            }
            match self.pool.as_ref() {
                DbPool::Sqlite(p) => {
                    sqlx::query(sql::UNACCEPT_BY_ID_QM)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
                DbPool::MySql(p) => {
                    sqlx::query(sql::UNACCEPT_BY_ID_QM)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
                DbPool::Postgres(p) => {
                    sqlx::query(sql::UNACCEPT_BY_ID_PG)
                        .bind(id)
                        .execute(p)
                        .await?;
                }
            }
            // Best-effort activity emit on unaccept (SP14/SP15). Actor
            // here is the recipient choosing to drop the share. Distinct
            // from `ShareDeleted` because the share row still exists
            // (unaccept just flips accepted=false), so the owner sees a
            // "share declined" event rather than a removal.
            //
            // The default `share_activity_recipients(actor, share_with)`
            // would dedup to only the unaccepting recipient (actor ==
            // share_with for direct shares; the actor is a group member
            // for group shares). The owner is the audience that actually
            // cares about this event, so we add them explicitly.
            self.emit_share_activity_with_extra_recipient(
                crabcloud_activity::EventType::ShareUnaccepted,
                "share_unaccepted_you",
                "share_unaccepted_by",
                id,
                requester.as_str(),
                row.share_type,
                row.share_with.as_deref(),
                row.file_target.as_str(),
                Some(row.uid_owner.as_str()),
            )
            .await;
            Ok(())
        } else {
            Err(ShareError::Forbidden)
        }
    }

    /// Create a `share_type=3` (public link) row. Separate from the
    /// user/group path because link rows have a different shape: no
    /// `share_with` recipient, a generated `token`, an optional bcrypt
    /// `password` hash, an optional `expiration`, and the full owner path
    /// in `file_target` (so `resolve_by_token` returns a usable subroot
    /// for the auth layer). SP8 §5.
    async fn create_link(&self, req: CreateShareRequest) -> Result<ShareRow, ShareError> {
        // Link permissions: at minimum bit 1 (read) or bit 4 (create).
        // bit 4 alone is the "file-drop" mode.
        if req.permissions & 1 == 0 && req.permissions & 4 == 0 {
            return Err(ShareError::BadPermissions);
        }
        let perms = SharePermissions::from_wire(req.permissions);

        let storage_path = parse_wire_path(&req.path)?;
        let row = self
            .filecache
            .lookup(&req.home_storage_id, &storage_path)
            .await
            .map_err(map_filecache)?
            .ok_or(ShareError::PathNotOwned)?;
        if row.storage_id != req.home_storage_id {
            return Err(ShareError::ReshareRejected);
        }

        let item_type = if row.mimetype.as_str() == crabcloud_filecache::DIRECTORY_MIMETYPE {
            ItemType::Folder
        } else {
            ItemType::File
        };
        // Link rows store the FULL owner path in `file_target` (unlike
        // user/group shares which store only the basename). The auth layer
        // reads this back via `Shares::resolve_by_token` and uses it as the
        // `SharedSubrootStorage` root, so it must be unambiguous.
        let file_target = format!("/{}", storage_path.as_str());
        let stime = unix_now();
        let share_type_db: i16 = req.share_type.into();
        let perms_db = perms.as_u32() as i32;
        let fileid = row.fileid;

        // Hash the password (bcrypt — Batch A landed bcrypt for workspace
        // consistency with `crabcloud-users`).
        let password_hash: Option<String> = match req.password.as_deref() {
            Some(pw) => {
                let h = crabcloud_publiclinks::Passwords::new()
                    .hash(pw)
                    .map_err(|_| {
                        ShareError::DbError(sqlx::Error::Protocol("password hash failed".into()))
                    })?;
                Some(h.as_str().to_string())
            }
            None => None,
        };

        let expiration: Option<NaiveDateTime> =
            req.expire_date.and_then(|d| d.and_hms_opt(0, 0, 0));

        // Token-collision retry: skipped for MVP. With 89 bits of entropy a
        // collision is implausible during a single create; if it ever happens,
        // `sqlx::Error::Database` surfaces a UNIQUE violation and the caller
        // sees a 500.
        let token = crabcloud_publiclinks::Tokens::new().generate();
        let token_str = token.to_string();
        // Email shares persist the recipient address in `share_with` for
        // audit + UI display. Plain Link shares omit it (None).
        let share_with_for_insert: Option<&str> = match req.share_type {
            ShareType::Email => Some(req.share_with.as_str()),
            _ => None,
        };
        let id = self
            .insert_link_row(
                share_type_db,
                share_with_for_insert,
                &req.requester,
                fileid,
                &file_target,
                perms_db,
                stime,
                item_type,
                expiration,
                &token_str,
                password_hash.as_deref(),
            )
            .await?;

        // Email-share post-insert hook: enqueue link_emailed mail.
        // No opt-out gate here — the requester explicitly addressed an
        // external recipient by typing their email, so the prefs table
        // (which is keyed on uid, not address) doesn't apply.
        if matches!(req.share_type, ShareType::Email) {
            self.try_enqueue_link_emailed_mail(
                req.share_with.as_str(),
                req.requester.as_str(),
                &storage_path,
                &token_str,
                password_hash.is_some(),
                expiration,
            )
            .await;
        }

        // Best-effort activity emit (SP14). Link / email shares fan out
        // to just the actor (the external recipient isn't a Crabcloud user).
        self.emit_share_activity(
            crabcloud_activity::EventType::ShareCreated,
            "share_created_you",
            "share_created_by",
            id,
            req.requester.as_str(),
            req.share_type,
            None,
            req.path.as_str(),
        )
        .await;

        Ok(ShareRow {
            id,
            share_type: req.share_type,
            // For Email shares, `share_with` is the recipient email address.
            // For Link shares it's `None`.
            share_with: match req.share_type {
                ShareType::Email => Some(req.share_with.clone()),
                _ => None,
            },
            uid_owner: req.requester.clone(),
            uid_initiator: req.requester,
            parent: None,
            item_type,
            item_source: fileid,
            file_source: fileid,
            file_target,
            permissions: perms,
            stime,
            accepted: true,
            expiration: expiration.map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc)),
            token: Some(token_str),
            password_hash,
            last_warned: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_link_row(
        &self,
        share_type_db: i16,
        share_with: Option<&str>,
        requester: &str,
        fileid: i64,
        file_target: &str,
        perms_db: i32,
        stime: i64,
        item_type: ItemType,
        expiration: Option<NaiveDateTime>,
        token: &str,
        password: Option<&str>,
    ) -> Result<i64, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind(share_with) // share_with (None for Link, email for Email)
                    .bind(requester) // uid_owner
                    .bind(requester) // uid_initiator
                    .bind::<Option<i64>>(None) // parent
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16) // accepted
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16) // mail_send
                    .bind::<Option<NaiveDateTime>>(None) // last_warned
                    .execute(p)
                    .await?;
                Ok(res.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind(share_with)
                    .bind(requester)
                    .bind(requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .execute(p)
                    .await?;
                Ok(res.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(share_type_db)
                    .bind(share_with)
                    .bind(requester)
                    .bind(requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16)
                    .bind::<Option<NaiveDateTime>>(None)
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }

    /// Resolve the recipient list for an activity event tied to a share.
    /// User shares fan out to `[actor, share_with]`; group shares fan out
    /// to `[actor, ...group_members]`; link / email shares fan out to
    /// just `[actor]`. Unparseable group ids degrade to just the actor
    /// (log + continue per SP14 §6).
    async fn share_activity_recipients(
        &self,
        actor: &str,
        share_type: ShareType,
        share_with: Option<&str>,
    ) -> Vec<UserId> {
        let mut raw: Vec<String> = vec![actor.to_string()];
        match share_type {
            ShareType::User => {
                if let Some(s) = share_with {
                    raw.push(s.to_string());
                }
            }
            ShareType::Group => {
                if let Some(gname) = share_with {
                    match GroupId::new(gname) {
                        Ok(gid) => match self.users.group_store().members_of(&gid).await {
                            Ok(members) => raw.extend(members.into_iter().map(|u| u.into_inner())),
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    group = gname,
                                    "sharing: activity group fan-out failed, emitting actor-only"
                                );
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                group = gname,
                                "sharing: activity skipping invalid group id"
                            );
                        }
                    }
                }
            }
            ShareType::Link | ShareType::Email => {}
        }
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .filter_map(|s| UserId::new(s).ok())
            .filter(|u| seen.insert(u.as_str().to_string()))
            .collect()
    }

    /// Best-effort emit a `share_created` / `share_deleted` event.
    /// Failure is logged + swallowed per SP14 spec §6.
    #[allow(clippy::too_many_arguments)]
    async fn emit_share_activity(
        &self,
        event_type: crabcloud_activity::EventType,
        subject_id_actor: &str,
        subject_id_recipient: &str,
        share_id: i64,
        actor: &str,
        share_type: ShareType,
        share_with: Option<&str>,
        path: &str,
    ) {
        self.emit_share_activity_with_extra_recipient(
            event_type,
            subject_id_actor,
            subject_id_recipient,
            share_id,
            actor,
            share_type,
            share_with,
            path,
            None,
        )
        .await
    }

    /// Same as [`Self::emit_share_activity`] but adds `extra_recipient`
    /// to the audience after the default `actor + share_with` set is
    /// resolved. Used by the `unaccept` branch of `delete` so the
    /// share's OWNER also sees the event — the default set dedups to
    /// just the unaccepting recipient otherwise.
    #[allow(clippy::too_many_arguments)]
    async fn emit_share_activity_with_extra_recipient(
        &self,
        event_type: crabcloud_activity::EventType,
        subject_id_actor: &str,
        subject_id_recipient: &str,
        share_id: i64,
        actor: &str,
        share_type: ShareType,
        share_with: Option<&str>,
        path: &str,
        extra_recipient: Option<&str>,
    ) {
        let mut recipients = self
            .share_activity_recipients(actor, share_type, share_with)
            .await;
        if let Some(uid) = extra_recipient {
            if let Ok(uid) = UserId::new(uid) {
                if !recipients.iter().any(|r| r.as_str() == uid.as_str()) {
                    recipients.push(uid);
                }
            }
        }
        let event = crabcloud_activity::ActivityEvent {
            actor: actor.to_string(),
            event_type,
            subject_id_actor: subject_id_actor.to_string(),
            subject_id_recipient: subject_id_recipient.to_string(),
            subject_params: serde_json::json!({
                "actor": actor,
                "file": path,
                "recipient": share_with.unwrap_or(""),
            }),
            object_type: crabcloud_activity::ObjectType::Share,
            object_id: Some(share_id),
            recipients,
            occurred_at: chrono::Utc::now().timestamp(),
        };
        if let Err(e) = self.activity.emit(event).await {
            tracing::warn!(
                error = %e,
                share_id,
                ?event_type,
                "sharing: activity emit failed"
            );
        }
    }

    /// Resolve the recipient list for a search fan-out tied to a share.
    /// User shares fan out to `[share_with]`; group shares fan out to
    /// every member of the group. Link / email shares fan out to none
    /// (public-link visibility is token-based, not viewer-uid-based).
    /// The owner is NOT included — they're already indexed via the
    /// per-write indexer; fan-out only touches recipient rows.
    async fn search_fanout_recipients(
        &self,
        share_type: ShareType,
        share_with: Option<&str>,
    ) -> Vec<UserId> {
        let mut raw: Vec<String> = Vec::new();
        match share_type {
            ShareType::User => {
                if let Some(s) = share_with {
                    raw.push(s.to_string());
                }
            }
            ShareType::Group => {
                if let Some(gname) = share_with {
                    match GroupId::new(gname) {
                        Ok(gid) => match self.users.group_store().members_of(&gid).await {
                            Ok(members) => raw.extend(members.into_iter().map(|u| u.into_inner())),
                            Err(e) => tracing::warn!(
                                error = %e,
                                group = gname,
                                "sharing: search fanout group expansion failed"
                            ),
                        },
                        Err(e) => tracing::warn!(
                            error = %e,
                            group = gname,
                            "sharing: search fanout skipping invalid group id"
                        ),
                    }
                }
            }
            ShareType::Link | ShareType::Email => {}
        }
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .filter_map(|s| UserId::new(s).ok())
            .filter(|u| seen.insert(u.as_str().to_string()))
            .collect()
    }

    /// Returns the de-duped set of `UserId`s that can see `fileid` via
    /// the share graph: every direct user-share recipient plus every
    /// group-share member (group expanded). Plus the owner row of each
    /// matched share so the indexer can index the owner under their own
    /// uid even when the file is shared.
    ///
    /// Cascading shares are honored: a share of `/docs` makes
    /// `/docs/q1/r.docx` (a descendant) visible to the recipients. The
    /// implementation walks the filecache parent chain from `fileid`
    /// up to root collecting ancestor fileids, then queries `oc_share`
    /// for any row whose `item_source` is one of those fileids.
    ///
    /// Used by `crabcloud-search`'s `SearchIndexer` for per-write
    /// recipient resolution. The owner of files that aren't shared at
    /// all is NOT returned by this method (no oc_share row exists);
    /// the indexer is responsible for separately indexing the owner.
    pub async fn recipients_for_fileid(&self, fileid: i64) -> Result<Vec<UserId>, ShareError> {
        // 1. Collect ancestor fileids (including `fileid` itself).
        let mut ancestor_ids: Vec<i64> = vec![fileid];
        let mut cursor: i64 = fileid;
        for _ in 0..64 {
            // Depth-cap defensively; path depth in practice is < 16.
            let row = self
                .filecache
                .lookup_by_id(cursor)
                .await
                .map_err(map_filecache)?;
            let Some(row) = row else { break };
            match row.parent {
                Some(parent_id) => {
                    ancestor_ids.push(parent_id);
                    cursor = parent_id;
                }
                None => break,
            }
        }

        // 2. Query oc_share for any user/group share whose item_source
        //    is in the ancestor set. Build the placeholder list dynamically.
        let mut shares: Vec<(i16, String, String)> = Vec::new(); // (share_type, share_with, uid_owner)
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let q_text = build_select_shares_for_sources(ancestor_ids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get("share_with")?;
                    let uid_owner: String = r.try_get("uid_owner")?;
                    if let Some(sw) = share_with {
                        shares.push((share_type, sw, uid_owner));
                    }
                }
            }
            DbPool::MySql(p) => {
                let q_text = build_select_shares_for_sources(ancestor_ids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get_unchecked("share_with")?;
                    let uid_owner: String = r.try_get_unchecked("uid_owner")?;
                    if let Some(sw) = share_with {
                        shares.push((share_type, sw, uid_owner));
                    }
                }
            }
            DbPool::Postgres(p) => {
                let q_text = build_select_shares_for_sources(ancestor_ids.len(), sql::Dialect::Pg);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get("share_with")?;
                    let uid_owner: String = r.try_get("uid_owner")?;
                    if let Some(sw) = share_with {
                        shares.push((share_type, sw, uid_owner));
                    }
                }
            }
        }

        // 3. Expand group shares + de-dupe.
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<UserId> = Vec::new();
        let push_uid =
            |raw: String, seen: &mut std::collections::HashSet<String>, out: &mut Vec<UserId>| {
                if let Ok(uid) = UserId::new(raw) {
                    if seen.insert(uid.as_str().to_string()) {
                        out.push(uid);
                    }
                }
            };
        const ST_USER: i16 = ShareType::User as i16;
        const ST_GROUP: i16 = ShareType::Group as i16;
        for (share_type, share_with, uid_owner) in shares {
            push_uid(uid_owner, &mut seen, &mut out);
            match share_type {
                ST_USER => push_uid(share_with, &mut seen, &mut out),
                ST_GROUP => {
                    if let Ok(gid) = GroupId::new(share_with) {
                        match self.users.group_store().members_of(&gid).await {
                            Ok(members) => {
                                for m in members {
                                    push_uid(m.into_inner(), &mut seen, &mut out);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    group = gid.as_str(),
                                    "sharing: recipients_for_fileid group expansion failed"
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Like [`Self::recipients_for_fileid`], but returns one
    /// [`ShareFanoutContext`] per covering share row so the caller can
    /// translate the owner-side path into each recipient's view (via
    /// `crabcloud_search::translate_path`). The owner is NOT included
    /// here — callers that also need the owner (e.g. the search
    /// indexer) layer it on themselves using the storage's
    /// `owner_uid()`.
    ///
    /// One share row → one context. If a fileid is covered by multiple
    /// shares (e.g. shared to alice directly AND to a group alice is
    /// in), each share yields its own context. Recipients within a
    /// context are de-duplicated; recipients across contexts are not —
    /// the per-write upsert is idempotent on `(viewer_uid, fileid)`, so
    /// later contexts simply overwrite earlier paths for the same
    /// recipient. Documented MVP behavior; the per-write path remains
    /// well-formed, just non-deterministic when multiple shares apply.
    pub async fn share_fanout_contexts_for_fileid(
        &self,
        fileid: i64,
    ) -> Result<Vec<ShareFanoutContext>, ShareError> {
        // Walk ancestors (same shape as recipients_for_fileid).
        let mut ancestor_ids: Vec<i64> = vec![fileid];
        let mut cursor: i64 = fileid;
        for _ in 0..64 {
            let row = self
                .filecache
                .lookup_by_id(cursor)
                .await
                .map_err(map_filecache)?;
            let Some(row) = row else { break };
            match row.parent {
                Some(parent_id) => {
                    ancestor_ids.push(parent_id);
                    cursor = parent_id;
                }
                None => break,
            }
        }

        // Pull the full per-share row context we need to build a
        // [`ShareFanoutContext`]: `item_source` (to look up the owner
        // subroot path) and `file_target` (the recipient prefix).
        #[derive(Debug)]
        struct ShareCtxRow {
            share_type: i16,
            share_with: String,
            item_source: i64,
            file_target: String,
        }
        let mut share_rows: Vec<ShareCtxRow> = Vec::new();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let q_text =
                    build_select_share_ctx_for_sources(ancestor_ids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get("share_with")?;
                    let item_source: i64 = r.try_get("item_source")?;
                    let file_target: String = r.try_get("file_target")?;
                    if let Some(sw) = share_with {
                        share_rows.push(ShareCtxRow {
                            share_type,
                            share_with: sw,
                            item_source,
                            file_target,
                        });
                    }
                }
            }
            DbPool::MySql(p) => {
                let q_text =
                    build_select_share_ctx_for_sources(ancestor_ids.len(), sql::Dialect::Qm);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get_unchecked("share_with")?;
                    let item_source: i64 = r.try_get("item_source")?;
                    let file_target: String = r.try_get_unchecked("file_target")?;
                    if let Some(sw) = share_with {
                        share_rows.push(ShareCtxRow {
                            share_type,
                            share_with: sw,
                            item_source,
                            file_target,
                        });
                    }
                }
            }
            DbPool::Postgres(p) => {
                let q_text =
                    build_select_share_ctx_for_sources(ancestor_ids.len(), sql::Dialect::Pg);
                let mut q = sqlx::query(&q_text);
                for id in &ancestor_ids {
                    q = q.bind(*id);
                }
                let rows = q.fetch_all(p).await?;
                for r in rows {
                    let share_type: i16 = r.try_get("share_type")?;
                    let share_with: Option<String> = r.try_get("share_with")?;
                    let item_source: i64 = r.try_get("item_source")?;
                    let file_target: String = r.try_get("file_target")?;
                    if let Some(sw) = share_with {
                        share_rows.push(ShareCtxRow {
                            share_type,
                            share_with: sw,
                            item_source,
                            file_target,
                        });
                    }
                }
            }
        }

        const ST_USER: i16 = ShareType::User as i16;
        const ST_GROUP: i16 = ShareType::Group as i16;
        let mut out: Vec<ShareFanoutContext> = Vec::with_capacity(share_rows.len());
        for share in share_rows {
            // Look up the OWNER-side path for this share's item_source.
            let item_row = self
                .filecache
                .lookup_by_id(share.item_source)
                .await
                .map_err(map_filecache)?;
            let Some(item_row) = item_row else {
                // Stale share row whose source has been hard-deleted;
                // skip silently — the un-share fan-out should clean it
                // up on the next lifecycle event.
                continue;
            };
            let owner_subroot = format!("/{}", item_row.path.as_str().trim_start_matches('/'));
            let recipient_prefix =
                format!("/{}", share.file_target.trim_start_matches('/'));

            let mut recipients: Vec<UserId> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let push = |raw: String,
                        seen: &mut std::collections::HashSet<String>,
                        out: &mut Vec<UserId>| {
                if let Ok(uid) = UserId::new(raw) {
                    if seen.insert(uid.as_str().to_string()) {
                        out.push(uid);
                    }
                }
            };
            match share.share_type {
                ST_USER => push(share.share_with, &mut seen, &mut recipients),
                ST_GROUP => {
                    if let Ok(gid) = GroupId::new(share.share_with) {
                        match self.users.group_store().members_of(&gid).await {
                            Ok(members) => {
                                for m in members {
                                    push(m.into_inner(), &mut seen, &mut recipients);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    group = gid.as_str(),
                                    "sharing: share_fanout_contexts_for_fileid group expansion failed"
                                );
                            }
                        }
                    }
                }
                _ => {}
            }

            if recipients.is_empty() {
                continue;
            }

            out.push(ShareFanoutContext {
                recipients,
                owner_storage_id: item_row.storage_id,
                owner_subroot,
                recipient_prefix,
            });
        }
        Ok(out)
    }
}

/// Like [`build_select_shares_for_sources`] but pulls the additional
/// `item_source` + `file_target` columns the per-share fan-out context
/// needs.
fn build_select_share_ctx_for_sources(n: usize, dialect: sql::Dialect) -> String {
    let mut q = String::with_capacity(180 + n * 4);
    q.push_str(
        "SELECT share_type, share_with, item_source, file_target FROM oc_share \
                WHERE share_type IN (0, 1) AND accepted = 1 AND item_source IN (",
    );
    match dialect {
        sql::Dialect::Qm => {
            for i in 0..n {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('?');
            }
        }
        sql::Dialect::Pg => {
            for i in 0..n {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('$');
                q.push_str(&(i + 1).to_string());
            }
        }
    }
    q.push(')');
    q
}

/// Build a `SELECT ... FROM oc_share WHERE item_source IN (?,?,...)` for
/// `n` placeholders. Used by `Shares::recipients_for_fileid`.
fn build_select_shares_for_sources(n: usize, dialect: sql::Dialect) -> String {
    let mut q = String::with_capacity(160 + n * 4);
    q.push_str(
        "SELECT share_type, share_with, uid_owner FROM oc_share \
                WHERE share_type IN (0, 1) AND accepted = 1 AND item_source IN (",
    );
    match dialect {
        sql::Dialect::Qm => {
            for i in 0..n {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('?');
            }
        }
        sql::Dialect::Pg => {
            for i in 0..n {
                if i > 0 {
                    q.push_str(", ");
                }
                q.push('$');
                q.push_str(&(i + 1).to_string());
            }
        }
    }
    q.push(')');
    q
}

async fn run_update_permissions(pool: &DbPool, id: i64, perms_db: i32) -> Result<(), ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(sql::UPDATE_PERMISSIONS_QM)
                .bind(perms_db)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(sql::UPDATE_PERMISSIONS_QM)
                .bind(perms_db)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(sql::UPDATE_PERMISSIONS_PG)
                .bind(perms_db)
                .bind(id)
                .execute(p)
                .await?;
        }
    }
    Ok(())
}

async fn run_update_password(
    pool: &DbPool,
    id: i64,
    value: Option<&str>,
) -> Result<(), ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_PG)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
    }
    Ok(())
}

async fn run_update_expiration(
    pool: &DbPool,
    id: i64,
    value: Option<NaiveDateTime>,
) -> Result<(), ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(sql::UPDATE_EXPIRATION_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(sql::UPDATE_EXPIRATION_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(sql::UPDATE_EXPIRATION_PG)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
    }
    Ok(())
}

fn parse_wire_path(p: &str) -> Result<StoragePath, ShareError> {
    let trimmed = p.trim_start_matches('/');
    StoragePath::new(trimmed).map_err(|_| ShareError::PathNotOwned)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn map_filecache(e: crabcloud_filecache::FileCacheError) -> ShareError {
    use crabcloud_filecache::FileCacheError as FE;
    match e {
        FE::Db(err) => ShareError::DbError(err),
        FE::NotFound | FE::AncestorMissing(_) | FE::Storage(_) | FE::Invalid(_) => {
            ShareError::PathNotOwned
        }
    }
}

// `map_users` is only invoked when the input is the *authenticated requester*
// (e.g. `groups_of(&ctx.user_id)`), never a free-form "share with" target.
// A non-Db error there means the requester's own user record is missing or
// storage is broken — not that a sharing target is unknown. Route those
// through DbError so they surface as 500 in OCS, not 404.
fn map_users(e: crabcloud_users::UsersError) -> ShareError {
    match e {
        crabcloud_users::UsersError::Db(crabcloud_db::DbError::Sqlx(inner)) => {
            ShareError::DbError(inner)
        }
        _ => ShareError::DbError(sqlx::Error::Protocol(format!("users service: {e}"))),
    }
}

/// Decoded slice of a row that the dialect-specific decoders all agree on.
/// Assembled by `assemble_row` into a typed `ShareRow`.
struct RowParts {
    id: i64,
    share_type: i16,
    share_with: Option<String>,
    uid_owner: String,
    uid_initiator: String,
    parent: Option<i64>,
    item_type: String,
    item_source: i64,
    file_source: i64,
    file_target: String,
    permissions: i32,
    stime: i64,
    accepted: i16,
    expiration: Option<NaiveDateTime>,
    token: Option<String>,
    password: Option<String>,
    last_warned: Option<NaiveDateTime>,
}

fn assemble_row(parts: RowParts) -> Result<ShareRow, ShareError> {
    let share_type =
        ShareType::try_from(parts.share_type).map_err(|_| ShareError::InvalidShareType)?;
    let item_type = ItemType::from_db_str(&parts.item_type).ok_or_else(|| {
        ShareError::DbError(sqlx::Error::Decode(
            format!("unknown item_type {:?}", parts.item_type).into(),
        ))
    })?;
    let permissions = SharePermissions::from_wire(parts.permissions as u32);
    let expiration = parts
        .expiration
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    let last_warned = parts
        .last_warned
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    Ok(ShareRow {
        id: parts.id,
        share_type,
        share_with: parts.share_with,
        uid_owner: parts.uid_owner,
        uid_initiator: parts.uid_initiator,
        parent: parts.parent,
        item_type,
        item_source: parts.item_source,
        file_source: parts.file_source,
        file_target: parts.file_target,
        permissions,
        stime: parts.stime,
        accepted: parts.accepted != 0,
        expiration,
        token: parts.token,
        password_hash: parts.password,
        last_warned,
    })
}

fn row_from_sqlite(row: sqlx::sqlite::SqliteRow) -> Result<ShareRow, ShareError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        share_type: row.try_get("share_type")?,
        share_with: row.try_get("share_with")?,
        uid_owner: row.try_get("uid_owner")?,
        uid_initiator: row.try_get("uid_initiator")?,
        parent: row.try_get("parent")?,
        item_type: row.try_get("item_type")?,
        item_source: row.try_get("item_source")?,
        file_source: row.try_get("file_source")?,
        file_target: row.try_get("file_target")?,
        permissions: row.try_get("permissions")?,
        stime: row.try_get("stime")?,
        accepted: row.try_get("accepted")?,
        expiration: row.try_get("expiration")?,
        token: row.try_get("token")?,
        password: row.try_get("password")?,
        last_warned: row.try_get("last_warned")?,
    })
}

fn row_from_mysql(row: sqlx::mysql::MySqlRow) -> Result<ShareRow, ShareError> {
    // Schema notes: `id`, `parent`, `item_source`, `file_source`, `stime` are
    // signed `BIGINT`. `permissions` is signed `INTEGER`. `share_type`,
    // `accepted`, `mail_send` are `SMALLINT`. The table is created with
    // `COLLATE=utf8mb4_bin`, which the mysql wire protocol reports as
    // `VARBINARY` rather than `VARCHAR` — `try_get_unchecked::<String, _>`
    // bypasses the runtime type check so the bytes round-trip into a String.
    assemble_row(RowParts {
        id: row.try_get("id")?,
        share_type: row.try_get("share_type")?,
        share_with: row.try_get_unchecked("share_with")?,
        uid_owner: row.try_get_unchecked("uid_owner")?,
        uid_initiator: row.try_get_unchecked("uid_initiator")?,
        parent: row.try_get("parent")?,
        item_type: row.try_get_unchecked("item_type")?,
        item_source: row.try_get("item_source")?,
        file_source: row.try_get("file_source")?,
        file_target: row.try_get_unchecked("file_target")?,
        permissions: row.try_get("permissions")?,
        stime: row.try_get("stime")?,
        accepted: row.try_get("accepted")?,
        // `expiration TIMESTAMP NULL` decodes as `DATETIME` in mysql wire
        // format; `try_get_unchecked` accepts both since the byte layout is
        // identical.
        expiration: row.try_get_unchecked("expiration")?,
        token: row.try_get_unchecked("token")?,
        password: row.try_get_unchecked("password")?,
        last_warned: row.try_get_unchecked("last_warned")?,
    })
}

fn row_from_postgres(row: sqlx::postgres::PgRow) -> Result<ShareRow, ShareError> {
    assemble_row(RowParts {
        id: row.try_get("id")?,
        share_type: row.try_get("share_type")?,
        share_with: row.try_get("share_with")?,
        uid_owner: row.try_get("uid_owner")?,
        uid_initiator: row.try_get("uid_initiator")?,
        parent: row.try_get("parent")?,
        item_type: row.try_get("item_type")?,
        item_source: row.try_get("item_source")?,
        file_source: row.try_get("file_source")?,
        file_target: row.try_get("file_target")?,
        permissions: row.try_get("permissions")?,
        stime: row.try_get("stime")?,
        accepted: row.try_get("accepted")?,
        expiration: row.try_get("expiration")?,
        token: row.try_get("token")?,
        password: row.try_get("password")?,
        last_warned: row.try_get("last_warned")?,
    })
}