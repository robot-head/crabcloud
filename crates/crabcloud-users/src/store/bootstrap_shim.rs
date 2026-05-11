//! BootstrapAdminBackend — wraps any `UserStore` and synthesizes a virtual
//! admin from `config.bootstrap_admin` if the wrapped store has no matching
//! user. First write through this backend retires the shim by INSERTing a
//! real DB row.

use super::{UserStore, UserWithHash};
use crate::error::{UsersError, UsersResult};
use crate::group::GroupId;
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

    async fn set_password(&self, uid: &UserId, new_hash: &str) -> UsersResult<()> {
        if self.inner.lookup(uid).await?.is_some() {
            return self.inner.set_password(uid, new_hash).await;
        }
        if uid.as_str() == self.admin.username {
            let user = self.synthesized_user()?;
            self.inner.create(&user, Some(new_hash)).await?;
            self.groups
                .add_to_group(&user.uid, &GroupId::new("admin")?)
                .await?;
            tracing::info!(
                uid = uid.as_str(),
                "promoted bootstrap admin to oc_users; remove [bootstrap_admin] from config.toml"
            );
            return Ok(());
        }
        Err(UsersError::NotFound)
    }

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
}
