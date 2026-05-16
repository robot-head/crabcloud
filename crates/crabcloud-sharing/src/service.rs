//! `Shares` — sharing CRUD service. SP7 spec sections §5 (surface) and §9
//! (auth/permissions). Schema lives in migration `0006_shares`.

use chrono::{DateTime, NaiveDateTime, Utc};
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_storage::StoragePath;
use crabcloud_users::{GroupId, UserId, UsersService};
use sqlx::Row as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ShareError;
use crate::permissions::SharePermissions;
use crate::sql::{self, Dialect};
use crate::types::{CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields};

#[derive(Clone)]
pub struct Shares {
    pub(crate) pool: Arc<DbPool>,
    pub(crate) users: Arc<UsersService>,
    pub(crate) filecache: Arc<FileCache>,
}

impl Shares {
    pub fn new(pool: Arc<DbPool>, users: Arc<UsersService>, filecache: Arc<FileCache>) -> Self {
        Self {
            pool,
            users,
            filecache,
        }
    }

    pub async fn create(&self, req: CreateShareRequest) -> Result<ShareRow, ShareError> {
        // Link rows take a different code path: no `share_with` target,
        // password and expiration handled, token generated. SP8 §5.
        if matches!(req.share_type, ShareType::Link) {
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
            ShareType::Link => unreachable!(),
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
                    .fetch_one(p)
                    .await?;
                row.try_get::<i64, _>("id")?
            }
        };

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

    /// Look up a share row by token. Returns `None` for unknown / non-link
    /// rows (the SQL is filtered to `share_type = 3`). Does NOT enforce
    /// expiration — the caller must compare `expiration` to `now()` and
    /// treat past-expired as missing. SP8 §5.
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
            // Only Link rows accept password updates.
            if !matches!(existing.share_type, ShareType::Link) {
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
            if raw & 1 == 0 {
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
                        ShareError::DbError(sqlx::Error::Protocol(
                            "password hash failed".into(),
                        ))
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
        let id = self
            .insert_link_row(
                share_type_db,
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

        Ok(ShareRow {
            id,
            share_type: ShareType::Link,
            share_with: None,
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
            expiration: expiration
                .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc)),
            token: Some(token_str),
            password_hash,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_link_row(
        &self,
        share_type_db: i16,
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
                    .bind::<Option<&str>>(None) // share_with
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
                    .execute(p)
                    .await?;
                Ok(res.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind::<Option<&str>>(None)
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
                    .execute(p)
                    .await?;
                Ok(res.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(share_type_db)
                    .bind::<Option<&str>>(None)
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
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }
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
    })
}
