//! `UsersService` — the public composition handlers reach for.

use crate::app_password::AppPasswordService;
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
    app_passwords: Option<Arc<AppPasswordService>>,
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
            app_passwords: None,
        }
    }

    /// Attach an `AppPasswordService` so `set_password` cascades
    /// `password_invalid=1` on every other token row.
    pub fn with_app_passwords(mut self, svc: Arc<AppPasswordService>) -> Self {
        self.app_passwords = Some(svc);
        self
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
    pub fn app_passwords(&self) -> Option<&Arc<AppPasswordService>> {
        self.app_passwords.as_ref()
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

    /// Rehash + write the new password. If an [`AppPasswordService`] is
    /// attached, also cascades `password_invalid=1` on every token row.
    ///
    /// **Non-atomic.** The password update and the token-cascade UPDATE are
    /// two separate SQL statements. If the cascade fails after the password
    /// has been rotated, the caller receives an `Err` and the system is left
    /// in a state where the new password is in effect but old app-password
    /// tokens are still valid. Both operations are idempotent — retrying
    /// `set_password` with the same `new` password rehashes to a different
    /// hash (bcrypt salts), but the post-conditions ("oc_users row's
    /// password column matches the verifier's `hash(new)`" + "every
    /// `oc_authtoken` row for `uid` has `password_invalid=1`") converge on
    /// retry. Callers should treat any `Err` as "retry once before giving
    /// up" so a transient cascade failure doesn't leave the user without
    /// invalidated tokens. A future transactional rewrite (single sqlx tx
    /// spanning both UPDATEs) closes this window entirely.
    pub async fn set_password(&self, uid: &UserId, new: &str) -> UsersResult<()> {
        let hash = self.verifier.hash(new)?;
        self.users.set_password(uid, &hash).await?;
        if let Some(ap) = &self.app_passwords {
            ap.invalidate_all_for_user(uid).await?;
        }
        Ok(())
    }

    /// Flip `enabled=false` AND delete every `oc_authtoken` row for `uid`.
    ///
    /// **Non-atomic.** The `set_enabled` UPDATE and the cascade `DELETE` on
    /// `oc_authtoken` are two separate statements. If the cascade fails after
    /// the enabled flag is flipped, the user is `enabled=false` with some
    /// token rows still present — but the AuthLayer's cookie-auth path checks
    /// `user.enabled` via `service.verify`, so subsequent cookie auth fails.
    /// Bearer/Basic auth via AuthLayer does NOT currently re-check enabled
    /// (see the parent spec §6.6); the token-row delete is the primary
    /// defense. Retry is idempotent: both target tables converge on the
    /// desired state.
    pub async fn disable_user(&self, uid: &UserId) -> UsersResult<()> {
        self.users.set_enabled(uid, false).await?;
        if let Some(ap) = &self.app_passwords {
            ap.revoke_all_for_user(uid).await?;
        }
        Ok(())
    }

    /// Delete every `oc_authtoken` row for `uid`, then delete the `oc_users`
    /// row (which cascades to `oc_group_user` + `oc_preferences` per
    /// `SqlUserStore::delete`).
    ///
    /// **Token-first ordering** is intentional: a racing in-flight auth
    /// either finds no token (already gone) or 401s. If the order were
    /// reversed, the brief window between row-delete and token-delete
    /// could allow a stale token to find a deleted user.
    ///
    /// Non-atomic, retry-idempotent — same shape as `disable_user`.
    pub async fn delete_user(&self, uid: &UserId) -> UsersResult<()> {
        if let Some(ap) = &self.app_passwords {
            ap.revoke_all_for_user(uid).await?;
        }
        self.users.delete(uid).await?;
        Ok(())
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

    #[tokio::test]
    async fn set_password_cascades_invalidate_when_app_passwords_attached() {
        use crate::app_password::AppPasswordService;
        use crate::auth_token::AuthTokenType;
        use crate::store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
        use crabcloud_cache::MemoryCache;
        use secrecy::SecretString;

        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("svc2.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let users_store: Arc<dyn crate::store::UserStore> =
            Arc::new(SqlUserStore::new(pool.clone()));
        let groups_store: Arc<dyn crate::store::GroupStore> =
            Arc::new(SqlGroupStore::new(pool.clone()));
        let prefs_store: Arc<dyn crate::store::PreferenceStore> =
            Arc::new(SqlPreferenceStore::new(pool.clone()));
        let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool));
        let token_cache = Arc::new(TokenAuthCache::new(
            token_store,
            Arc::new(MemoryCache::new()),
            "inst1",
        ));
        let app_passwords = Arc::new(AppPasswordService::new(
            token_cache,
            SecretString::new("s".into()),
        ));
        let svc = UsersService::new(
            users_store,
            groups_store,
            prefs_store,
            Arc::new(BcryptVerifier::new()),
        )
        .with_app_passwords(app_passwords.clone());

        let uid = UserId::new("alice").unwrap();
        let hash = svc.verifier.hash("old").unwrap();
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
        let (_row, raw) = app_passwords
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        assert!(app_passwords.verify(raw.expose()).await.is_ok());
        svc.set_password(&uid, "new").await.unwrap();
        let err = app_passwords.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, crate::UsersError::TokenNotFound));
    }

    async fn fresh_service_with_app_passwords() -> UsersService {
        use crate::app_password::AppPasswordService;
        use crate::store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
        use crabcloud_cache::MemoryCache;
        use secrecy::SecretString;
        let dir = tempdir().unwrap();
        let cfg =
            crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("svcap.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let users: Arc<dyn crate::store::UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
        let groups: Arc<dyn crate::store::GroupStore> = Arc::new(SqlGroupStore::new(pool.clone()));
        let prefs: Arc<dyn crate::store::PreferenceStore> =
            Arc::new(SqlPreferenceStore::new(pool.clone()));
        let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool));
        let token_cache = Arc::new(TokenAuthCache::new(
            token_store,
            Arc::new(MemoryCache::new()),
            "inst1",
        ));
        let app_passwords = Arc::new(AppPasswordService::new(
            token_cache,
            SecretString::new("s".into()),
        ));
        UsersService::new(users, groups, prefs, Arc::new(BcryptVerifier::new()))
            .with_app_passwords(app_passwords)
    }

    #[tokio::test]
    async fn disable_user_flips_enabled_and_revokes_tokens() {
        use crate::auth_token::AuthTokenType;
        let svc = fresh_service_with_app_passwords().await;
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
        let ap = svc.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        assert!(ap.verify(raw.expose()).await.is_ok());

        svc.disable_user(&uid).await.unwrap();

        let u = svc.users.lookup(&uid).await.unwrap().unwrap();
        assert!(!u.enabled);
        let err = ap.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }

    #[tokio::test]
    async fn delete_user_revokes_tokens_then_deletes_row_and_cascades() {
        use crate::auth_token::AuthTokenType;
        let svc = fresh_service_with_app_passwords().await;
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
        svc.groups
            .add_to_group(&uid, &GroupId::new("admin").unwrap())
            .await
            .unwrap();
        svc.prefs.set(&uid, "core", "lang", "en").await.unwrap();
        let ap = svc.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();

        svc.delete_user(&uid).await.unwrap();

        // user row gone
        assert!(svc.users.lookup(&uid).await.unwrap().is_none());
        // group membership gone (cascade from SqlUserStore::delete)
        let admins = svc
            .groups
            .members_of(&GroupId::new("admin").unwrap())
            .await
            .unwrap();
        assert!(!admins.iter().any(|u| u.as_str() == "alice"));
        // preference gone (cascade from SqlUserStore::delete)
        assert!(svc.prefs.get(&uid, "core", "lang").await.unwrap().is_none());
        // token revoked
        let err = ap.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }
}
