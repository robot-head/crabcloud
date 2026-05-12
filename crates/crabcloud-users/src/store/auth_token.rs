//! `TokenStore` — async trait for `oc_authtoken` CRUD + lifecycle.
//! `SqlTokenStore` body lands in Task 4. `TokenAuthCache` lands in Task 5
//! (Batch B).

use crate::auth_token::AuthToken;
use crate::error::UsersResult;
use crate::user::UserId;
use async_trait::async_trait;
use crabcloud_db::DbPool;

#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Insert a fresh row. Returns the new row id.
    async fn create(&self, row: &AuthToken) -> UsersResult<i64>;

    /// Look up by hash. Returns `None` on miss (NOT an error).
    async fn lookup_by_hash(&self, hash: &str) -> UsersResult<Option<AuthToken>>;

    /// Look up by primary key. Returns `None` on miss.
    async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>>;

    /// All rows for `uid`, newest-`last_activity`-first.
    async fn list_for_user(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>>;

    /// Set `last_activity = last_check = now`. Best-effort: a missing row
    /// is silently ignored to avoid failing an otherwise-successful auth.
    async fn bump_activity(&self, id: i64, now: u64) -> UsersResult<()>;

    /// Delete by id. Idempotent (deleting an absent row is fine).
    async fn revoke(&self, id: i64) -> UsersResult<()>;

    /// Delete every row owned by `uid`.
    async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()>;

    /// Delete every row owned by `uid` except `except`.
    async fn revoke_all_for_user_except(&self, uid: &UserId, except: i64) -> UsersResult<()>;

    /// Set `password_invalid = 1` on every row owned by `uid`.
    async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()>;
}

#[derive(Clone)]
pub struct SqlTokenStore {
    #[allow(dead_code)]
    pool: DbPool,
}

impl SqlTokenStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}
