//! `UsersService` — the public composition handlers reach for.

use crate::error::{UsersError, UsersResult};
use crate::group::GroupId;
use crate::password::PasswordVerifier;
use crate::store::{GroupStore, PreferenceStore, UserStore};
use crate::user::{User, UserId};
use std::sync::Arc;

#[derive(Clone)]
pub struct UsersService {
    users: Arc<dyn UserStore>,
    groups: Arc<dyn GroupStore>,
    prefs: Arc<dyn PreferenceStore>,
    verifier: Arc<dyn PasswordVerifier>,
}

impl UsersService {
    pub fn new(
        users: Arc<dyn UserStore>,
        groups: Arc<dyn GroupStore>,
        prefs: Arc<dyn PreferenceStore>,
        verifier: Arc<dyn PasswordVerifier>,
    ) -> Self {
        Self {
            users,
            groups,
            prefs,
            verifier,
        }
    }

    pub fn user_store(&self) -> &Arc<dyn UserStore> {
        &self.users
    }
    pub fn group_store(&self) -> &Arc<dyn GroupStore> {
        &self.groups
    }
    pub fn preferences(&self) -> &Arc<dyn PreferenceStore> {
        &self.prefs
    }
    pub fn verifier(&self) -> &Arc<dyn PasswordVerifier> {
        &self.verifier
    }

    /// Verify a (login, password) pair. Always runs bcrypt — even on miss —
    /// so user-enumeration timing oracles don't work.
    pub async fn verify(&self, login: &str, password: &str) -> UsersResult<User> {
        let candidate = self.users.lookup_for_auth(login).await?;
        let (user, hash) = match candidate {
            Some(uwh) => (Some(uwh.user), uwh.password_hash),
            None => (None, None),
        };
        let ok = self.verifier.verify(password, hash.as_deref());
        match (user, ok) {
            (Some(u), true) if u.enabled => {
                self.users.touch_last_seen(&u.uid).await?;
                Ok(u)
            }
            _ => Err(UsersError::InvalidCredentials),
        }
    }

    pub async fn lookup(&self, uid: &UserId) -> UsersResult<Option<User>> {
        self.users.lookup(uid).await
    }

    pub async fn lookup_by_login(&self, login: &str) -> UsersResult<Option<User>> {
        self.users.lookup_by_login(login).await
    }

    pub async fn set_password(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        let hash = self.verifier.hash(new)?;
        self.users.set_password(uid, &hash).await
    }

    pub async fn is_admin(&self, uid: &UserId) -> UsersResult<bool> {
        let admin = GroupId::new("admin")?;
        self.groups.is_in_group(uid, &admin).await
    }

    pub async fn groups_of(&self, uid: &UserId) -> UsersResult<Vec<GroupId>> {
        self.groups.groups_of(uid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::BcryptVerifier;
    use crate::store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_service() -> UsersService {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("svc.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        UsersService::new(
            Arc::new(SqlUserStore::new(pool.clone())),
            Arc::new(SqlGroupStore::new(pool.clone())),
            Arc::new(SqlPreferenceStore::new(pool)),
            Arc::new(BcryptVerifier::new()),
        )
    }

    #[tokio::test]
    async fn verify_succeeds_with_correct_password() {
        let svc = fresh_service().await;
        let uid = UserId::new("alice").unwrap();
        let hash = svc.verifier.hash("hunter2").unwrap();
        svc.users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "A".into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        let user = svc.verify("alice", "hunter2").await.unwrap();
        assert_eq!(user.uid, uid);
    }

    #[tokio::test]
    async fn verify_fails_unknown_user_with_consistent_error() {
        let svc = fresh_service().await;
        let err = svc.verify("nobody", "x").await.unwrap_err();
        assert!(matches!(err, UsersError::InvalidCredentials));
    }

    #[tokio::test]
    async fn verify_fails_for_disabled_user() {
        let svc = fresh_service().await;
        let uid = UserId::new("d").unwrap();
        let hash = svc.verifier.hash("hunter2").unwrap();
        svc.users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "D".into(),
                    email: None,
                    enabled: false,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        let err = svc.verify("d", "hunter2").await.unwrap_err();
        assert!(matches!(err, UsersError::InvalidCredentials));
    }

    #[tokio::test]
    async fn is_admin_resolves() {
        let svc = fresh_service().await;
        let uid = UserId::new("ad").unwrap();
        svc.users
            .create(
                &User {
                    uid: uid.clone(),
                    display_name: "Ad".into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                None,
            )
            .await
            .unwrap();
        assert!(!svc.is_admin(&uid).await.unwrap());
        svc.groups
            .add_to_group(&uid, &GroupId::new("admin").unwrap())
            .await
            .unwrap();
        assert!(svc.is_admin(&uid).await.unwrap());
    }
}
