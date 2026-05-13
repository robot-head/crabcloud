//! `Shares` — sharing CRUD service. SP7 spec sections §5 (surface) and §9
//! (auth/permissions). Schema lives in migration `0006_shares`.

use chrono::{DateTime, NaiveDateTime, Utc};
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_storage::StoragePath;
use crabcloud_users::{GroupId, UserId, UsersService};
use sqlx::Row as _;
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
        if matches!(req.share_type, ShareType::Link) {
            return Err(ShareError::NotImplemented);
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
                UserId::new(req.share_with.clone()).map_err(|_| ShareError::RecipientUnknown)?;
                if !user_row_exists(&self.pool, &req.share_with).await? {
                    return Err(ShareError::RecipientUnknown);
                }
            }
            ShareType::Group => {
                GroupId::new(req.share_with.clone()).map_err(|_| ShareError::RecipientUnknown)?;
                if !group_row_exists(&self.pool, &req.share_with).await? {
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
        if fields.password.is_some() || fields.note.is_some() {
            return Err(ShareError::NotImplemented);
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

async fn user_row_exists(pool: &DbPool, uid: &str) -> Result<bool, ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM oc_users WHERE uid = ?")
                .bind(uid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
        DbPool::MySql(p) => {
            let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM oc_users WHERE uid = ?")
                .bind(uid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
        DbPool::Postgres(p) => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM oc_users WHERE uid = $1")
                .bind(uid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
    }
}

async fn group_row_exists(pool: &DbPool, gid: &str) -> Result<bool, ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM oc_groups WHERE gid = ?")
                .bind(gid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
        DbPool::MySql(p) => {
            let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM oc_groups WHERE gid = ?")
                .bind(gid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
        DbPool::Postgres(p) => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM oc_groups WHERE gid = $1")
                .bind(gid)
                .fetch_optional(p)
                .await?;
            Ok(row.is_some())
        }
    }
}

fn map_users(e: crabcloud_users::UsersError) -> ShareError {
    match e {
        crabcloud_users::UsersError::Db(crabcloud_db::DbError::Sqlx(inner)) => {
            ShareError::DbError(inner)
        }
        _ => ShareError::RecipientUnknown,
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
