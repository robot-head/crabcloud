//! SQL backend for the three store traits. Per-dialect query dispatch follows
//! the platform-core `match &pool` pattern.

use super::{GroupStore, PreferenceStore, UserStore, UserWithHash};
use crate::email::Email;
use crate::error::{UsersError, UsersResult};
use crate::group::{Group, GroupId};
use crate::user::{User, UserId};
use async_trait::async_trait;
use crabcloud_db::{DbError, DbPool};

#[derive(Clone)]
pub struct SqlUserStore {
    pool: DbPool,
}

impl SqlUserStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

fn map_sqlx<T>(r: Result<T, sqlx::Error>) -> UsersResult<T> {
    r.map_err(|e| UsersError::Db(DbError::Sqlx(e)))
}

fn row_to_user(
    uid: String,
    display: Option<String>,
    email: Option<String>,
    last_seen: i64,
    enabled_int: i64,
) -> UsersResult<User> {
    let user_id = UserId::new(uid)?;
    let email = email.map(Email::parse).transpose()?;
    Ok(User {
        uid: user_id,
        display_name: display.unwrap_or_default(),
        email,
        enabled: enabled_int != 0,
        last_seen: last_seen.max(0) as u64,
    })
}

#[async_trait]
impl UserStore for SqlUserStore {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        let row: Option<(String, Option<String>, Option<String>, i64, i64)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = ?",
                )
                .bind(uid.as_str())
                .fetch_optional(p)
                .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = ?",
                )
                .bind(uid.as_str())
                .fetch_optional(p)
                .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE uid = $1",
                )
                .bind(uid.as_str())
                .fetch_optional(p)
                .await,
            )?,
        };
        match row {
            None => Ok(None),
            Some((u, d, e, l, en)) => Ok(Some(row_to_user(u, d, e, l, en)?)),
        }
    }

    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        let user_id = UserId::new(login).ok();
        if let Some(uid) = user_id {
            if let Some(u) = self.lookup(&uid).await? {
                return Ok(Some(u));
            }
        }
        if login.contains('@') {
            let lower = login.to_ascii_lowercase();
            let row: Option<(String, Option<String>, Option<String>, i64, i64)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = ?",
                    )
                    .bind(&lower)
                    .fetch_optional(p)
                    .await,
                )?,
                DbPool::MySql(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = ?",
                    )
                    .bind(&lower)
                    .fetch_optional(p)
                    .await,
                )?,
                DbPool::Postgres(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, last_seen, enabled FROM oc_users WHERE LOWER(email) = $1",
                    )
                    .bind(&lower)
                    .fetch_optional(p)
                    .await,
                )?,
            };
            return row
                .map(|(u, d, e, l, en)| row_to_user(u, d, e, l, en))
                .transpose();
        }
        Ok(None)
    }

    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>> {
        let row: Option<(String, Option<String>, Option<String>, Option<String>, i64, i64)> =
            match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = ? OR LOWER(email) = ?",
                    )
                    .bind(login)
                    .bind(login.to_ascii_lowercase())
                    .fetch_optional(p)
                    .await,
                )?,
                DbPool::MySql(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = ? OR LOWER(email) = ?",
                    )
                    .bind(login)
                    .bind(login.to_ascii_lowercase())
                    .fetch_optional(p)
                    .await,
                )?,
                DbPool::Postgres(p) => map_sqlx(
                    sqlx::query_as(
                        "SELECT uid, displayname, email, password, last_seen, enabled FROM oc_users WHERE uid = $1 OR LOWER(email) = $2",
                    )
                    .bind(login)
                    .bind(login.to_ascii_lowercase())
                    .fetch_optional(p)
                    .await,
                )?,
            };
        match row {
            None => Ok(None),
            Some((u, d, e, hash, l, en)) => Ok(Some(UserWithHash {
                user: row_to_user(u, d, e, l, en)?,
                password_hash: hash,
            })),
        }
    }

    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()> {
        let q_sqlite_mysql = "UPDATE oc_users SET password = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET password = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(new_hash)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::MySql(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(new_hash)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query(q_pg)
                    .bind(new_hash)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
        };
        if affected == 0 {
            return Err(UsersError::NotFound);
        }
        Ok(())
    }

    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        if new.is_empty() || new.len() > 64 || new.chars().any(|c| c.is_control()) {
            return Err(UsersError::InvalidDisplayName(format!("{new:?}")));
        }
        let q_sqlite_mysql = "UPDATE oc_users SET displayname = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET displayname = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(new)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::MySql(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(new)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query(q_pg)
                    .bind(new)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
        };
        if affected == 0 {
            return Err(UsersError::NotFound);
        }
        Ok(())
    }

    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()> {
        let canonical = match new {
            Some(raw) => Some(Email::parse(raw)?.as_str().to_string()),
            None => None,
        };
        if let Some(ref c) = canonical {
            let q_sqlite_mysql = "SELECT uid FROM oc_users WHERE email = ? AND uid <> ?";
            let q_pg = "SELECT uid FROM oc_users WHERE email = $1 AND uid <> $2";
            let dup: Option<(String,)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(
                    sqlx::query_as(q_sqlite_mysql)
                        .bind(c)
                        .bind(uid.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
                DbPool::MySql(p) => map_sqlx(
                    sqlx::query_as(q_sqlite_mysql)
                        .bind(c)
                        .bind(uid.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
                DbPool::Postgres(p) => map_sqlx(
                    sqlx::query_as(q_pg)
                        .bind(c)
                        .bind(uid.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
            };
            if dup.is_some() {
                return Err(UsersError::EmailAlreadyTaken);
            }
        }
        let q_sqlite_mysql = "UPDATE oc_users SET email = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET email = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(canonical.as_deref())
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::MySql(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(canonical.as_deref())
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query(q_pg)
                    .bind(canonical.as_deref())
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
        };
        if affected == 0 {
            return Err(UsersError::NotFound);
        }
        Ok(())
    }

    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()> {
        let v: i64 = if enabled { 1 } else { 0 };
        let q_sqlite_mysql = "UPDATE oc_users SET enabled = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET enabled = $1 WHERE uid = $2";
        let affected = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(v)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::MySql(p) => map_sqlx(
                sqlx::query(q_sqlite_mysql)
                    .bind(v)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query(q_pg)
                    .bind(v)
                    .bind(uid.as_str())
                    .execute(p)
                    .await,
            )?
            .rows_affected(),
        };
        if affected == 0 {
            return Err(UsersError::NotFound);
        }
        Ok(())
    }

    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()> {
        if self.lookup(&user.uid).await?.is_some() {
            return Err(UsersError::UidAlreadyExists);
        }
        if let Some(ref e) = user.email {
            let q_sqlite_mysql = "SELECT uid FROM oc_users WHERE email = ?";
            let q_pg = "SELECT uid FROM oc_users WHERE email = $1";
            let dup: Option<(String,)> = match &self.pool {
                DbPool::Sqlite(p) => map_sqlx(
                    sqlx::query_as(q_sqlite_mysql)
                        .bind(e.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
                DbPool::MySql(p) => map_sqlx(
                    sqlx::query_as(q_sqlite_mysql)
                        .bind(e.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
                DbPool::Postgres(p) => map_sqlx(
                    sqlx::query_as(q_pg)
                        .bind(e.as_str())
                        .fetch_optional(p)
                        .await,
                )?,
            };
            if dup.is_some() {
                return Err(UsersError::EmailAlreadyTaken);
            }
        }
        let enabled_int: i64 = if user.enabled { 1 } else { 0 };
        let last_seen: i64 = user.last_seen as i64;
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(user.uid.as_str())
                    .bind(password_hash)
                    .bind(&user.display_name)
                    .bind(user.email.as_ref().map(|e| e.as_str()))
                    .bind(last_seen)
                    .bind(enabled_int)
                    .execute(p)
                    .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(user.uid.as_str())
                    .bind(password_hash)
                    .bind(&user.display_name)
                    .bind(user.email.as_ref().map(|e| e.as_str()))
                    .bind(last_seen)
                    .bind(enabled_int)
                    .execute(p)
                    .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_users (uid, password, displayname, email, last_seen, enabled) VALUES ($1, $2, $3, $4, $5, $6)",
                    )
                    .bind(user.uid.as_str())
                    .bind(password_hash)
                    .bind(&user.display_name)
                    .bind(user.email.as_ref().map(|e| e.as_str()))
                    .bind(last_seen)
                    .bind(enabled_int)
                    .execute(p)
                    .await,
                )?;
            }
        };
        Ok(())
    }

    async fn delete(&self, uid: &UserId) -> UsersResult<()> {
        for (sqlite_mysql, pg) in &[
            (
                "DELETE FROM oc_group_user WHERE uid = ?",
                "DELETE FROM oc_group_user WHERE uid = $1",
            ),
            (
                "DELETE FROM oc_preferences WHERE userid = ?",
                "DELETE FROM oc_preferences WHERE userid = $1",
            ),
            (
                "DELETE FROM oc_users WHERE uid = ?",
                "DELETE FROM oc_users WHERE uid = $1",
            ),
        ] {
            match &self.pool {
                DbPool::Sqlite(p) => {
                    map_sqlx(
                        sqlx::query(sqlite_mysql)
                            .bind(uid.as_str())
                            .execute(p)
                            .await,
                    )?;
                }
                DbPool::MySql(p) => {
                    map_sqlx(
                        sqlx::query(sqlite_mysql)
                            .bind(uid.as_str())
                            .execute(p)
                            .await,
                    )?;
                }
                DbPool::Postgres(p) => {
                    map_sqlx(sqlx::query(pg).bind(uid.as_str()).execute(p).await)?;
                }
            };
        }
        Ok(())
    }

    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let q_sqlite_mysql = "UPDATE oc_users SET last_seen = ? WHERE uid = ?";
        let q_pg = "UPDATE oc_users SET last_seen = $1 WHERE uid = $2";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(now)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(now)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(now)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
        };
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqlGroupStore {
    pool: DbPool,
}

impl SqlGroupStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl GroupStore for SqlGroupStore {
    async fn lookup(&self, gid: &GroupId) -> UsersResult<Option<Group>> {
        let row: Option<(String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as("SELECT gid, displayname FROM oc_groups WHERE gid = ?")
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as("SELECT gid, displayname FROM oc_groups WHERE gid = ?")
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as("SELECT gid, displayname FROM oc_groups WHERE gid = $1")
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
        };
        match row {
            None => Ok(None),
            Some((g, d)) => Ok(Some(Group {
                gid: GroupId::new(g)?,
                display_name: d.unwrap_or_default(),
            })),
        }
    }

    async fn is_in_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<bool> {
        let row: Option<(i64,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as("SELECT 1 FROM oc_group_user WHERE uid = ? AND gid = ?")
                    .bind(uid.as_str())
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as("SELECT 1 FROM oc_group_user WHERE uid = ? AND gid = ?")
                    .bind(uid.as_str())
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as("SELECT 1 FROM oc_group_user WHERE uid = $1 AND gid = $2")
                    .bind(uid.as_str())
                    .bind(gid.as_str())
                    .fetch_optional(p)
                    .await,
            )?,
        };
        Ok(row.is_some())
    }

    async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        let rows: Vec<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as("SELECT gid FROM oc_group_user WHERE uid = ? ORDER BY gid")
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as("SELECT gid FROM oc_group_user WHERE uid = ? ORDER BY gid")
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as("SELECT gid FROM oc_group_user WHERE uid = $1 ORDER BY gid")
                    .bind(uid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
        };
        rows.into_iter().map(|(g,)| GroupId::new(g)).collect()
    }

    async fn members_of(&self, gid: &GroupId) -> UsersResult<Vec<UserId>> {
        let rows: Vec<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as("SELECT uid FROM oc_group_user WHERE gid = ? ORDER BY uid")
                    .bind(gid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as("SELECT uid FROM oc_group_user WHERE gid = ? ORDER BY uid")
                    .bind(gid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as("SELECT uid FROM oc_group_user WHERE gid = $1 ORDER BY uid")
                    .bind(gid.as_str())
                    .fetch_all(p)
                    .await,
            )?,
        };
        rows.into_iter().map(|(u,)| UserId::new(u)).collect()
    }

    async fn add_to_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        let q_sqlite = "INSERT OR IGNORE INTO oc_group_user (gid, uid) VALUES (?, ?)";
        let q_mysql = "INSERT IGNORE INTO oc_group_user (gid, uid) VALUES (?, ?)";
        let q_pg = "INSERT INTO oc_group_user (gid, uid) VALUES ($1, $2) ON CONFLICT DO NOTHING";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_mysql)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
        };
        Ok(())
    }

    async fn remove_from_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_group_user WHERE gid = ? AND uid = ?";
        let q_pg = "DELETE FROM oc_group_user WHERE gid = $1 AND uid = $2";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(gid.as_str())
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
        };
        Ok(())
    }

    async fn create(&self, group: &Group) -> UsersResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query("INSERT INTO oc_groups (gid, displayname) VALUES (?, ?)")
                        .bind(group.gid.as_str())
                        .bind(&group.display_name)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query("INSERT INTO oc_groups (gid, displayname) VALUES (?, ?)")
                        .bind(group.gid.as_str())
                        .bind(&group.display_name)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query("INSERT INTO oc_groups (gid, displayname) VALUES ($1, $2)")
                        .bind(group.gid.as_str())
                        .bind(&group.display_name)
                        .execute(p)
                        .await,
                )?;
            }
        };
        Ok(())
    }

    async fn delete(&self, gid: &GroupId) -> UsersResult<()> {
        for (sqlite_mysql, pg) in &[
            (
                "DELETE FROM oc_group_user WHERE gid = ?",
                "DELETE FROM oc_group_user WHERE gid = $1",
            ),
            (
                "DELETE FROM oc_groups WHERE gid = ?",
                "DELETE FROM oc_groups WHERE gid = $1",
            ),
        ] {
            match &self.pool {
                DbPool::Sqlite(p) => {
                    map_sqlx(
                        sqlx::query(sqlite_mysql)
                            .bind(gid.as_str())
                            .execute(p)
                            .await,
                    )?;
                }
                DbPool::MySql(p) => {
                    map_sqlx(
                        sqlx::query(sqlite_mysql)
                            .bind(gid.as_str())
                            .execute(p)
                            .await,
                    )?;
                }
                DbPool::Postgres(p) => {
                    map_sqlx(sqlx::query(pg).bind(gid.as_str()).execute(p).await)?;
                }
            };
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqlPreferenceStore {
    pool: DbPool,
}

impl SqlPreferenceStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PreferenceStore for SqlPreferenceStore {
    async fn get(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<Option<String>> {
        let row: Option<(Option<String>,)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configvalue FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?",
                )
                .bind(uid.as_str())
                .bind(app)
                .bind(key)
                .fetch_optional(p)
                .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configvalue FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?",
                )
                .bind(uid.as_str())
                .bind(app)
                .bind(key)
                .fetch_optional(p)
                .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configvalue FROM oc_preferences WHERE userid = $1 AND appid = $2 AND configkey = $3",
                )
                .bind(uid.as_str())
                .bind(app)
                .bind(key)
                .fetch_optional(p)
                .await,
            )?,
        };
        Ok(row.and_then(|(v,)| v))
    }

    async fn set(&self, uid: &UserId, app: &str, key: &str, value: &str) -> UsersResult<()> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES (?, ?, ?, ?) \
                         ON CONFLICT(userid, appid, configkey) DO UPDATE SET configvalue = excluded.configvalue",
                    )
                    .bind(uid.as_str())
                    .bind(app)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES (?, ?, ?, ?) \
                         ON DUPLICATE KEY UPDATE configvalue = VALUES(configvalue)",
                    )
                    .bind(uid.as_str())
                    .bind(app)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(
                        "INSERT INTO oc_preferences (userid, appid, configkey, configvalue) VALUES ($1, $2, $3, $4) \
                         ON CONFLICT (userid, appid, configkey) DO UPDATE SET configvalue = EXCLUDED.configvalue",
                    )
                    .bind(uid.as_str())
                    .bind(app)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await,
                )?;
            }
        };
        Ok(())
    }

    async fn delete(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<()> {
        let q_sqlite_mysql =
            "DELETE FROM oc_preferences WHERE userid = ? AND appid = ? AND configkey = ?";
        let q_pg = "DELETE FROM oc_preferences WHERE userid = $1 AND appid = $2 AND configkey = $3";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .bind(app)
                        .bind(key)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .bind(app)
                        .bind(key)
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(
                    sqlx::query(q_pg)
                        .bind(uid.as_str())
                        .bind(app)
                        .bind(key)
                        .execute(p)
                        .await,
                )?;
            }
        };
        Ok(())
    }

    async fn list(&self, uid: &UserId, app: &str) -> UsersResult<Vec<(String, String)>> {
        let rows: Vec<(String, Option<String>)> = match &self.pool {
            DbPool::Sqlite(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configkey, configvalue FROM oc_preferences WHERE userid = ? AND appid = ? ORDER BY configkey",
                )
                .bind(uid.as_str())
                .bind(app)
                .fetch_all(p)
                .await,
            )?,
            DbPool::MySql(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configkey, configvalue FROM oc_preferences WHERE userid = ? AND appid = ? ORDER BY configkey",
                )
                .bind(uid.as_str())
                .bind(app)
                .fetch_all(p)
                .await,
            )?,
            DbPool::Postgres(p) => map_sqlx(
                sqlx::query_as(
                    "SELECT configkey, configvalue FROM oc_preferences WHERE userid = $1 AND appid = $2 ORDER BY configkey",
                )
                .bind(uid.as_str())
                .bind(app)
                .fetch_all(p)
                .await,
            )?,
        };
        Ok(rows
            .into_iter()
            .map(|(k, v)| (k, v.unwrap_or_default()))
            .collect())
    }

    async fn delete_all_for(&self, uid: &UserId) -> UsersResult<()> {
        let q_sqlite_mysql = "DELETE FROM oc_preferences WHERE userid = ?";
        let q_pg = "DELETE FROM oc_preferences WHERE userid = $1";
        match &self.pool {
            DbPool::Sqlite(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::MySql(p) => {
                map_sqlx(
                    sqlx::query(q_sqlite_mysql)
                        .bind(uid.as_str())
                        .execute(p)
                        .await,
                )?;
            }
            DbPool::Postgres(p) => {
                map_sqlx(sqlx::query(q_pg).bind(uid.as_str()).execute(p).await)?;
            }
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_db::{core_set, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_pool() -> DbPool {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("u.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn user_crud_roundtrip() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        let uid = UserId::new("alice").unwrap();
        let user = User {
            uid: uid.clone(),
            display_name: "Alice".into(),
            email: Some(Email::parse("alice@example.com").unwrap()),
            enabled: true,
            last_seen: 0,
        };
        store.create(&user, Some("hash")).await.unwrap();

        let got = store.lookup(&uid).await.unwrap().unwrap();
        assert_eq!(got.display_name, "Alice");
        assert_eq!(got.email.unwrap().as_str(), "alice@example.com");
        assert!(got.enabled);

        store.set_display_name(&uid, "Alice Smith").await.unwrap();
        let updated = store.lookup(&uid).await.unwrap().unwrap();
        assert_eq!(updated.display_name, "Alice Smith");

        store.delete(&uid).await.unwrap();
        assert!(store.lookup(&uid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn group_membership() {
        let pool = fresh_pool().await;
        let users = SqlUserStore::new(pool.clone());
        let groups = SqlGroupStore::new(pool);
        let uid = UserId::new("bob").unwrap();
        users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "Bob".into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap();
        let admin = GroupId::new("admin").unwrap();
        assert!(!groups.is_in_group(&uid, &admin).await.unwrap());
        groups.add_to_group(&uid, &admin).await.unwrap();
        assert!(groups.is_in_group(&uid, &admin).await.unwrap());
        let g = groups.groups_of(&uid).await.unwrap();
        assert_eq!(g, vec![admin.clone()]);
    }

    #[tokio::test]
    async fn preferences_upsert_and_read() {
        let pool = fresh_pool().await;
        let users = SqlUserStore::new(pool.clone());
        let prefs = SqlPreferenceStore::new(pool);
        let uid = UserId::new("c").unwrap();
        users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "C".into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap();
        prefs.set(&uid, "files", "max_upload", "1024").await.unwrap();
        prefs.set(&uid, "files", "max_upload", "2048").await.unwrap();
        assert_eq!(
            prefs
                .get(&uid, "files", "max_upload")
                .await
                .unwrap()
                .as_deref(),
            Some("2048")
        );
    }

    #[tokio::test]
    async fn lookup_by_login_matches_email() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        store
            .create(
                &User {
                    uid: UserId::new("dave").unwrap(),
                    display_name: "Dave".into(),
                    email: Some(Email::parse("dave@example.com").unwrap()),
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap();
        let by_email = store.lookup_by_login("DAVE@example.com").await.unwrap();
        assert!(by_email.is_some());
    }

    #[tokio::test]
    async fn create_rejects_duplicate_email() {
        let pool = fresh_pool().await;
        let store = SqlUserStore::new(pool);
        store
            .create(
                &User {
                    uid: UserId::new("e1").unwrap(),
                    display_name: "E1".into(),
                    email: Some(Email::parse("e@example.com").unwrap()),
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap();
        let err = store
            .create(
                &User {
                    uid: UserId::new("e2").unwrap(),
                    display_name: "E2".into(),
                    email: Some(Email::parse("e@example.com").unwrap()),
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, UsersError::EmailAlreadyTaken));
    }
}
