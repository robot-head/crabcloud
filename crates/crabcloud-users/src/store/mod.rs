//! Backend traits. SQL implementation in `sql.rs`; future LDAP/SAML backends
//! plug in via the same traits.

pub mod auth_token;
pub mod bootstrap_shim;
pub mod sql;

use crate::error::UsersResult;
use crate::group::{Group, GroupId};
use crate::user::{User, UserId};
use async_trait::async_trait;

/// User-record-with-hash for auth-time lookups only. Kept off the public
/// `User` type so handlers can't accidentally leak the hash.
#[derive(Debug, Clone)]
pub struct UserWithHash {
    pub user: User,
    pub password_hash: Option<String>,
}

#[async_trait]
pub trait UserStore: Send + Sync {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>>;
    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>>;
    /// Auth-time lookup. Returns the hash alongside the user. None on miss.
    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>>;
    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()>;
    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()>;
    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()>;
    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()>;
    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()>;
    async fn delete(&self, uid: &UserId) -> UsersResult<()>;
    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()>;
}

#[async_trait]
pub trait GroupStore: Send + Sync {
    async fn lookup(&self, gid: &GroupId) -> UsersResult<Option<Group>>;
    async fn is_in_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<bool>;
    async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>>;
    async fn members_of(&self, gid: &GroupId) -> UsersResult<Vec<UserId>>;
    async fn add_to_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()>;
    async fn remove_from_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()>;
    async fn create(&self, group: &Group) -> UsersResult<()>;
    async fn delete(&self, gid: &GroupId) -> UsersResult<()>;
}

#[async_trait]
pub trait PreferenceStore: Send + Sync {
    async fn get(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<Option<String>>;
    async fn set(&self, uid: &UserId, app: &str, key: &str, value: &str) -> UsersResult<()>;
    async fn delete(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<()>;
    async fn list(&self, uid: &UserId, app: &str) -> UsersResult<Vec<(String, String)>>;
    async fn delete_all_for(&self, uid: &UserId) -> UsersResult<()>;
}
