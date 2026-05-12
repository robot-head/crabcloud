//! `AppPasswordService` — public mint/list/revoke/verify surface that
//! handlers and the CLI reach for. Composes a [`TokenAuthCache`] with the
//! `config.secret` used to derive token hashes.

use crate::auth_token::{hash_token, AuthToken, AuthTokenType, RawToken};
use crate::error::{UsersError, UsersResult};
use crate::store::auth_token::TokenAuthCache;
use crate::user::UserId;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Public composition handlers / settings UI / CLI all reach for. Wraps a
/// read-through token cache + the signing secret.
#[derive(Clone)]
pub struct AppPasswordService {
    tokens: Arc<TokenAuthCache>,
    secret: Arc<SecretString>,
}

impl AppPasswordService {
    pub fn new(tokens: Arc<TokenAuthCache>, secret: SecretString) -> Self {
        Self {
            tokens,
            secret: Arc::new(secret),
        }
    }

    pub fn token_cache(&self) -> &Arc<TokenAuthCache> {
        &self.tokens
    }

    /// Mint a new token. Returns `(persisted_row, raw_token)`. The `raw_token`
    /// is the *plaintext* the caller must show the user exactly once.
    pub async fn mint(
        &self,
        uid: &UserId,
        login_name: &str,
        name: &str,
        kind: AuthTokenType,
        remember: bool,
    ) -> UsersResult<(AuthToken, RawToken)> {
        let raw = RawToken::generate();
        let now = now_secs();
        let hash = hash_token(raw.expose(), self.secret.expose_secret());
        let candidate = AuthToken {
            id: 0,
            uid: uid.clone(),
            login_name: login_name.to_string(),
            password: None,
            name: name.to_string(),
            token: hash,
            kind,
            remember,
            last_activity: now,
            last_check: now,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        };
        let id = self.tokens.create(&candidate).await?;
        let mut persisted = candidate;
        persisted.id = id;
        Ok((persisted, raw))
    }

    /// Verify a raw token. Returns the row on success, [`UsersError::TokenNotFound`]
    /// on miss / unusable. Bumps `last_activity` (rate-limited) on hit.
    pub async fn verify(&self, raw: &str) -> UsersResult<AuthToken> {
        let hash = hash_token(raw, self.secret.expose_secret());
        let row = self
            .tokens
            .lookup_by_hash(&hash)
            .await?
            .ok_or(UsersError::TokenNotFound)?;
        let now = now_secs();
        if row.is_unusable(now) {
            return Err(UsersError::TokenNotFound);
        }
        // Best-effort: a failed activity bump must not fail an otherwise-valid
        // auth. The next request hits the cache anyway; activity will catch up
        // on the next successful bump.
        let _ = self.tokens.maybe_bump_activity(&row, now).await;
        Ok(row)
    }

    pub async fn list(&self, uid: &UserId) -> UsersResult<Vec<AuthToken>> {
        self.tokens.list_for_user(uid).await
    }

    pub async fn lookup_by_id(&self, id: i64) -> UsersResult<Option<AuthToken>> {
        self.tokens.lookup_by_id(id).await
    }

    pub async fn revoke(&self, id: i64) -> UsersResult<()> {
        self.tokens.revoke(id).await
    }

    pub async fn revoke_other_sessions(&self, uid: &UserId, current: i64) -> UsersResult<()> {
        self.tokens.revoke_all_for_user_except(uid, current).await
    }

    /// Delete every token row owned by `uid`. Used by `UsersService::{disable,
    /// delete}_user` to force-logout the target across all devices.
    ///
    /// Implementation forwards to the cache's `revoke_all_for_user_except`
    /// with `except = i64::MIN` (no real row's id is `MIN`, so nothing is
    /// preserved). Future hardening could add a dedicated `revoke_all_for_user`
    /// on `TokenAuthCache` to avoid the sentinel.
    pub async fn revoke_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        self.tokens.revoke_all_for_user_except(uid, i64::MIN).await
    }

    pub async fn invalidate_all_for_user(&self, uid: &UserId) -> UsersResult<()> {
        self.tokens.invalidate_all_for_user(uid).await
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::auth_token::SqlTokenStore;
    use crabcloud_cache::MemoryCache;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use tempfile::tempdir;

    async fn fresh_svc() -> AppPasswordService {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("ap.db"));
        std::mem::forget(dir);
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let store: Arc<dyn crate::store::auth_token::TokenStore> =
            Arc::new(SqlTokenStore::new(pool));
        let cache = TokenAuthCache::new(store, Arc::new(MemoryCache::new()), "inst1");
        AppPasswordService::new(Arc::new(cache), SecretString::new("the-secret".into()))
    }

    #[tokio::test]
    async fn mint_then_verify_succeeds() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (row, raw) = svc
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        let v = svc.verify(raw.expose()).await.unwrap();
        assert_eq!(v.id, row.id);
        assert_eq!(v.uid.as_str(), "alice");
    }

    #[tokio::test]
    async fn verify_unknown_returns_token_not_found() {
        let svc = fresh_svc().await;
        let err = svc.verify("not-a-real-token").await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }

    #[tokio::test]
    async fn verify_password_invalidated_returns_token_not_found() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (_row, raw) = svc
            .mint(&uid, "alice", "DAV", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        svc.invalidate_all_for_user(&uid).await.unwrap();
        let err = svc.verify(raw.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }

    #[tokio::test]
    async fn revoke_other_sessions_keeps_current() {
        let svc = fresh_svc().await;
        let uid = UserId::new("alice").unwrap();
        let (keep, _) = svc
            .mint(&uid, "alice", "current", AuthTokenType::Session, false)
            .await
            .unwrap();
        let (_drop, raw_drop) = svc
            .mint(&uid, "alice", "other", AuthTokenType::AppPassword, false)
            .await
            .unwrap();
        svc.revoke_other_sessions(&uid, keep.id).await.unwrap();
        assert!(svc.lookup_by_id(keep.id).await.unwrap().is_some());
        let err = svc.verify(raw_drop.expose()).await.unwrap_err();
        assert!(matches!(err, UsersError::TokenNotFound));
    }
}
