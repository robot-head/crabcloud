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

/// Filter shape for [`UserStore::list_users`]. Substring search is case-
/// insensitive and runs against `uid`, `displayname`, `email`. Empty / `None`
/// search returns all rows in `uid` order. `limit` is clamped to `[1, 500]`
/// by the handler; offset has no upper bound (callers paginate).
#[derive(Debug, Clone)]
pub struct UserListFilter<'a> {
    pub search: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
}

/// Filter shape for [`GroupStore::list_groups`]. Substring search on
/// `gid OR displayname`. Same clamp semantics as [`UserListFilter`].
#[derive(Debug, Clone)]
pub struct GroupListFilter<'a> {
    pub search: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
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

    /// Paginated user list with optional case-insensitive substring search.
    /// Search hits `uid OR displayname OR email`; empty search returns all.
    /// Returns rows in `uid` ASC order.
    async fn list_users(&self, filter: UserListFilter<'_>) -> UsersResult<Vec<User>>;

    /// True iff a real DB row exists in `oc_users`. The default impl delegates
    /// to `lookup`; layers that synthesize users (e.g. `BootstrapAdminBackend`)
    /// override this to bypass the synthesis path. Used by admin OCS handlers
    /// to 404 cleanly on the virtual admin without exposing shim internals.
    async fn exists_in_storage(&self, uid: &UserId) -> UsersResult<bool> {
        Ok(self.lookup(uid).await?.is_some())
    }
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

    /// Paginated group list with optional case-insensitive substring search.
    /// Search hits `gid OR displayname`. Returns rows in `gid` ASC order.
    async fn list_groups(&self, filter: GroupListFilter<'_>) -> UsersResult<Vec<Group>>;
}

#[async_trait]
pub trait PreferenceStore: Send + Sync {
    async fn get(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<Option<String>>;
    async fn set(&self, uid: &UserId, app: &str, key: &str, value: &str) -> UsersResult<()>;
    async fn delete(&self, uid: &UserId, app: &str, key: &str) -> UsersResult<()>;
    async fn list(&self, uid: &UserId, app: &str) -> UsersResult<Vec<(String, String)>>;
    async fn delete_all_for(&self, uid: &UserId) -> UsersResult<()>;
}
