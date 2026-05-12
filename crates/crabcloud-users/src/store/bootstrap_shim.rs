//! BootstrapAdminBackend — wraps any `UserStore` and synthesizes a virtual
//! admin from `config.bootstrap_admin` if the wrapped store has no matching
//! user. First write through this backend retires the shim by INSERTing a
//! real DB row.

use super::{GroupListFilter, UserStore, UserWithHash};
use crate::error::{UsersError, UsersResult};
use crate::group::{Group, GroupId};
use crate::store::GroupStore;
use crate::user::{User, UserId};
use async_trait::async_trait;
use crabcloud_config::BootstrapAdminConfig;
use std::sync::Arc;

pub struct BootstrapAdminBackend {
    inner: Arc<dyn UserStore>,
    groups: Arc<dyn GroupStore>,
    admin: BootstrapAdminConfig,
}

impl BootstrapAdminBackend {
    pub fn new(
        inner: Arc<dyn UserStore>,
        groups: Arc<dyn GroupStore>,
        admin: BootstrapAdminConfig,
    ) -> Self {
        Self {
            inner,
            groups,
            admin,
        }
    }

    fn matches_login(&self, login: &str) -> bool {
        login == self.admin.username
    }

    fn synthesized_user(&self) -> UsersResult<User> {
        Ok(User {
            uid: UserId::new(&self.admin.username)?,
            display_name: self.admin.username.clone(),
            email: None,
            enabled: true,
            last_seen: 0,
        })
    }
}

#[async_trait]
impl UserStore for BootstrapAdminBackend {
    async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        if let Some(u) = self.inner.lookup(uid).await? {
            return Ok(Some(u));
        }
        if uid.as_str() == self.admin.username {
            return Ok(Some(self.synthesized_user()?));
        }
        Ok(None)
    }

    async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        if let Some(u) = self.inner.lookup_by_login(login).await? {
            return Ok(Some(u));
        }
        if self.matches_login(login) {
            return Ok(Some(self.synthesized_user()?));
        }
        Ok(None)
    }

    async fn lookup_for_auth(&self, login: &str) -> UsersResult<Option<UserWithHash>> {
        if let Some(real) = self.inner.lookup_for_auth(login).await? {
            return Ok(Some(real));
        }
        if self.matches_login(login) {
            return Ok(Some(UserWithHash {
                user: self.synthesized_user()?,
                password_hash: Some(self.admin.password_hash.clone()),
            }));
        }
        Ok(None)
    }

    /// Promote-on-write: if `uid` matches the virtual admin, INSERT into oc_users
    /// and add to the admin group. The two operations are not atomic — if
    /// `add_to_group` fails after `inner.create` succeeds, the user lands in
    /// oc_users without admin group membership and the shim will not auto-recover
    /// (subsequent calls go through the delegate path). Operators should restore
    /// admin membership via the CLI (`group-add-member admin <uid>`) in that case.
    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_password(uid, new_hash).await;
        }
        if uid.as_str() == self.admin.username {
            let user = self.synthesized_user()?;
            self.inner.create(&user, Some(new_hash)).await?;
            let admin = GroupId::new("admin")?;
            // Idempotency: a re-run after a partial create+add failure is not
            // reachable through the shim (the inner.lookup branch will short-
            // circuit), but defend anyway in case a future bootstrap path
            // pre-seeds the user row without the group.
            if !self.groups.is_in_group(&user.uid, &admin).await? {
                self.groups.add_to_group(&user.uid, &admin).await?;
            }
            tracing::info!(
                uid = uid.as_str(),
                "promoted bootstrap admin to oc_users; remove [bootstrap_admin] from config.toml"
            );
            return Ok(());
        }
        Err(UsersError::NotFound)
    }

    /// Non-password mutators on the virtual admin return `ReadOnly`. Promote
    /// the admin first by calling `set_password` (which INSERTs into oc_users),
    /// then these mutators take effect on the real DB row. Promote-then-set in
    /// one call is intentionally deferred — see plan §7.
    async fn set_display_name(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_display_name(uid, new).await;
        }
        Err(UsersError::ReadOnly)
    }

    async fn set_email(&self, uid: &UserId, new: Option<&str>) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_email(uid, new).await;
        }
        Err(UsersError::ReadOnly)
    }

    async fn set_enabled(&self, uid: &UserId, enabled: bool) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_enabled(uid, enabled).await;
        }
        Err(UsersError::ReadOnly)
    }

    async fn create(&self, user: &User, password_hash: Option<&str>) -> UsersResult<()> {
        self.inner.create(user, password_hash).await
    }

    async fn delete(&self, uid: &UserId) -> UsersResult<()> {
        self.inner.delete(uid).await
    }

    async fn touch_last_seen(&self, uid: &UserId) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.touch_last_seen(uid).await;
        }
        Ok(())
    }

    async fn list_users(&self, filter: crate::store::UserListFilter<'_>) -> UsersResult<Vec<User>> {
        // The shim never returns the virtual admin in lists — it's only
        // visible to lookup_for_auth (login by the bootstrap admin name).
        self.inner.list_users(filter).await
    }

    async fn exists_in_storage(&self, uid: &UserId) -> UsersResult<bool> {
        // Skip the shim's synthesized fall-through; ask the inner store.
        self.inner.exists_in_storage(uid).await
    }
}

/// Wraps any `GroupStore` and synthesizes admin-group membership for the
/// bootstrap virtual admin. Without this, the `AdminUser` extractor rejects
/// the virtual admin with 403 (its `is_in_group("admin", "admin")` returns
/// false because there's no `oc_group_user` row yet).
///
/// Spec §6.6: the virtual admin is invisible to admin OCS *listings*
/// (`members_of("admin")` does NOT include it), but it acts as an admin
/// caller (`is_in_group` + `groups_of` synthesize the membership).
pub struct BootstrapAdminGroupBackend {
    inner: Arc<dyn GroupStore>,
    admin_username: String,
}

impl BootstrapAdminGroupBackend {
    pub fn new(inner: Arc<dyn GroupStore>, admin_username: String) -> Self {
        Self {
            inner,
            admin_username,
        }
    }
}

#[async_trait]
impl GroupStore for BootstrapAdminGroupBackend {
    async fn lookup(&self, gid: &GroupId) -> UsersResult<Option<Group>> {
        self.inner.lookup(gid).await
    }

    async fn is_in_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<bool> {
        if self.inner.is_in_group(uid, gid).await? {
            return Ok(true);
        }
        Ok(uid.as_str() == self.admin_username && gid.as_str() == "admin")
    }

    async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        let mut groups = self.inner.groups_of(uid).await?;
        if uid.as_str() == self.admin_username {
            let admin_gid = GroupId::new("admin")?;
            if !groups.iter().any(|g| g == &admin_gid) {
                groups.push(admin_gid);
            }
        }
        Ok(groups)
    }

    /// Virtual admin is invisible to listings — `members_of("admin")` does
    /// not include it. The `require_not_last_admin` guard then protects only
    /// against deleting the last *real* admin row, which is the desired
    /// behavior: if the bootstrap admin still lives in config it can always
    /// log in and re-promote, so the cluster is never locked out.
    async fn members_of(&self, gid: &GroupId) -> UsersResult<Vec<UserId>> {
        self.inner.members_of(gid).await
    }

    async fn add_to_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        self.inner.add_to_group(uid, gid).await
    }

    async fn remove_from_group(&self, uid: &UserId, gid: &GroupId) -> UsersResult<()> {
        self.inner.remove_from_group(uid, gid).await
    }

    async fn create(&self, group: &Group) -> UsersResult<()> {
        self.inner.create(group).await
    }

    async fn delete(&self, gid: &GroupId) -> UsersResult<()> {
        self.inner.delete(gid).await
    }

    async fn list_groups(&self, filter: GroupListFilter<'_>) -> UsersResult<Vec<Group>> {
        self.inner.list_groups(filter).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::{BcryptVerifier, PasswordVerifier};
    use crate::store::sql::{SqlGroupStore, SqlUserStore};
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn make() -> (
        BootstrapAdminBackend,
        Arc<dyn GroupStore>,
        Arc<dyn UserStore>,
    ) {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("b.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let inner: Arc<dyn UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
        let groups: Arc<dyn GroupStore> = Arc::new(SqlGroupStore::new(pool));
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        let shim = BootstrapAdminBackend::new(
            inner.clone(),
            groups.clone(),
            BootstrapAdminConfig {
                username: "admin".into(),
                password_hash: hash,
            },
        );
        (shim, groups, inner)
    }

    #[tokio::test]
    async fn virtual_admin_visible_via_lookup() {
        let (shim, _, _) = make().await;
        let u = shim
            .lookup(&UserId::new("admin").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(u.uid.as_str(), "admin");
    }

    #[tokio::test]
    async fn lookup_for_auth_returns_synthesized_hash() {
        let (shim, _, _) = make().await;
        let r = shim.lookup_for_auth("admin").await.unwrap().unwrap();
        assert!(r.password_hash.is_some());
    }

    #[tokio::test]
    async fn set_password_on_virtual_admin_promotes_to_db() {
        let (shim, groups, inner) = make().await;
        let uid = UserId::new("admin").unwrap();
        let new_hash = BcryptVerifier::new().hash("newpass").unwrap();
        shim.set_password(&uid, &new_hash).await.unwrap();
        assert!(inner.lookup(&uid).await.unwrap().is_some());
        assert!(groups
            .is_in_group(&uid, &GroupId::new("admin").unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn exists_in_storage_false_for_virtual_admin() {
        let (shim, _groups, _inner) = make().await;
        // The virtual admin (config-only) has no oc_users row.
        assert!(!shim
            .exists_in_storage(&UserId::new("admin").unwrap())
            .await
            .unwrap());
        // Sanity: lookup synthesizes it.
        assert!(shim
            .lookup(&UserId::new("admin").unwrap())
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn exists_in_storage_true_for_promoted_admin() {
        let (shim, _groups, _inner) = make().await;
        let uid = UserId::new("admin").unwrap();
        let new_hash = BcryptVerifier::new().hash("newpass").unwrap();
        // Promote: set_password creates the oc_users row.
        shim.set_password(&uid, &new_hash).await.unwrap();
        assert!(shim.exists_in_storage(&uid).await.unwrap());
    }

    async fn make_group_shim() -> (BootstrapAdminGroupBackend, Arc<dyn GroupStore>) {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("g.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let inner: Arc<dyn GroupStore> = Arc::new(SqlGroupStore::new(pool));
        let shim = BootstrapAdminGroupBackend::new(inner.clone(), "admin".into());
        (shim, inner)
    }

    #[tokio::test]
    async fn group_shim_synthesizes_admin_membership_for_virtual_admin() {
        let (shim, _inner) = make_group_shim().await;
        let admin_gid = GroupId::new("admin").unwrap();
        let virtual_admin = UserId::new("admin").unwrap();
        assert!(shim.is_in_group(&virtual_admin, &admin_gid).await.unwrap());
    }

    #[tokio::test]
    async fn group_shim_excludes_non_admin_group_for_virtual_admin() {
        let (shim, inner) = make_group_shim().await;
        inner
            .create(&Group {
                gid: GroupId::new("devs").unwrap(),
                display_name: "Devs".into(),
            })
            .await
            .unwrap();
        let devs_gid = GroupId::new("devs").unwrap();
        let virtual_admin = UserId::new("admin").unwrap();
        assert!(!shim.is_in_group(&virtual_admin, &devs_gid).await.unwrap());
    }

    #[tokio::test]
    async fn group_shim_excludes_real_uid_from_synthesized_admin() {
        let (shim, _inner) = make_group_shim().await;
        let admin_gid = GroupId::new("admin").unwrap();
        let other = UserId::new("alice").unwrap();
        assert!(!shim.is_in_group(&other, &admin_gid).await.unwrap());
    }

    #[tokio::test]
    async fn group_shim_groups_of_virtual_admin_includes_admin() {
        let (shim, _inner) = make_group_shim().await;
        let groups = shim
            .groups_of(&UserId::new("admin").unwrap())
            .await
            .unwrap();
        assert!(groups.iter().any(|g| g.as_str() == "admin"));
    }

    #[tokio::test]
    async fn group_shim_members_of_admin_excludes_virtual_admin() {
        let (shim, _inner) = make_group_shim().await;
        let admin_gid = GroupId::new("admin").unwrap();
        let members = shim.members_of(&admin_gid).await.unwrap();
        assert!(!members.iter().any(|u| u.as_str() == "admin"));
    }
}
