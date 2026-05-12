//! Helpers for the server-bin's user/group management subcommands.
//! Pure async functions consuming `UsersService`.

use crate::app_password::AppPasswordService;
use crate::auth_token::AuthTokenType;
use crate::email::Email;
use crate::error::UsersResult;
use crate::group::GroupId;
use crate::service::UsersService;
use crate::user::{User, UserId};

/// Create a user (and optionally add to the admin group). Not atomic: if the
/// process dies between `inner.create` and `add_to_group`, the user lands in
/// `oc_users` without admin membership. The state is recoverable by re-running
/// `user-add` (which fails fast with `UidAlreadyExists`) and then running
/// `group-add-member admin <uid>` manually.
pub async fn user_add(
    svc: &UsersService,
    uid: &str,
    password: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    admin: bool,
) -> UsersResult<()> {
    let user_id = UserId::new(uid)?;
    let dn = display_name
        .map(str::to_string)
        .unwrap_or_else(|| uid.to_string());
    let email_opt = match email {
        Some(e) => Some(Email::parse(e)?),
        None => None,
    };
    let hash = svc.verifier().hash(password)?;
    let user = User {
        uid: user_id.clone(),
        display_name: dn,
        email: email_opt,
        enabled: true,
        last_seen: 0,
    };
    svc.user_store().create(&user, Some(&hash)).await?;
    if admin {
        svc.group_store()
            .add_to_group(&user_id, &GroupId::new("admin")?)
            .await?;
    }
    Ok(())
}

pub async fn user_set_password(svc: &UsersService, uid: &str, new: &str) -> UsersResult<()> {
    let user_id = UserId::new(uid)?;
    svc.set_password(&user_id, new).await
}

pub async fn user_delete(svc: &UsersService, uid: &str) -> UsersResult<()> {
    // `SqlUserStore::delete` cascades to oc_group_user + oc_preferences + oc_users
    // atomically (sequentially under the same pool). Keeping cascade ownership inside
    // the store is more atomic than composing two awaits from here.
    let user_id = UserId::new(uid)?;
    svc.user_store().delete(&user_id).await?;
    Ok(())
}

pub async fn group_add_member(svc: &UsersService, gid: &str, uid: &str) -> UsersResult<()> {
    svc.group_store()
        .add_to_group(&UserId::new(uid)?, &GroupId::new(gid)?)
        .await
}

pub async fn group_remove_member(svc: &UsersService, gid: &str, uid: &str) -> UsersResult<()> {
    svc.group_store()
        .remove_from_group(&UserId::new(uid)?, &GroupId::new(gid)?)
        .await
}

/// Mint a fresh `AppPassword`-kind token for `uid`. Returns the row id +
/// raw plaintext token; the caller must surface the plaintext exactly
/// once (it is not retrievable after this returns).
pub async fn app_password_add(
    ap: &AppPasswordService,
    uid: &str,
    name: &str,
) -> UsersResult<(i64, String)> {
    let user_id = UserId::new(uid)?;
    let (row, raw) = ap
        .mint(&user_id, uid, name, AuthTokenType::AppPassword, false)
        .await?;
    Ok((row.id, raw.expose().to_string()))
}

/// List every `oc_authtoken` row owned by `uid` as
/// `(id, name, kind, last_activity)` tuples.
pub async fn app_password_list(
    ap: &AppPasswordService,
    uid: &str,
) -> UsersResult<Vec<(i64, String, AuthTokenType, u64)>> {
    let user_id = UserId::new(uid)?;
    Ok(ap
        .list(&user_id)
        .await?
        .into_iter()
        .map(|r| (r.id, r.name, r.kind, r.last_activity))
        .collect())
}

/// Revoke a token row by id. Idempotent: deleting an absent row succeeds.
pub async fn app_password_revoke(ap: &AppPasswordService, id: i64) -> UsersResult<()> {
    ap.revoke(id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::BcryptVerifier;
    use crate::store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn fresh_svc() -> UsersService {
        let dir = tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("c.db"));
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
    async fn user_add_then_set_password_then_delete() {
        let svc = fresh_svc().await;
        user_add(&svc, "alice", "hunter2", Some("Alice"), None, false)
            .await
            .unwrap();
        assert!(svc.lookup_by_login("alice").await.unwrap().is_some());

        user_set_password(&svc, "alice", "newpass").await.unwrap();
        // verify works against new password
        let _ = svc.verify("alice", "newpass").await.unwrap();

        user_delete(&svc, "alice").await.unwrap();
        assert!(svc.lookup_by_login("alice").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn user_add_admin_lands_in_admin_group() {
        let svc = fresh_svc().await;
        user_add(&svc, "bob", "pw", None, None, true).await.unwrap();
        assert!(svc.is_admin(&UserId::new("bob").unwrap()).await.unwrap());
    }

    #[tokio::test]
    async fn app_password_add_then_list_then_revoke() {
        use crate::app_password::AppPasswordService;
        use crate::store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
        use crabcloud_cache::MemoryCache;
        use secrecy::SecretString;

        let dir = tempfile::tempdir().unwrap();
        let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("c2.db"));
        std::mem::forget(dir);
        let pool = crabcloud_db::DbPool::connect(&cfg).await.unwrap();
        let mut runner = crabcloud_db::MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(crabcloud_db::core_set());
        runner.run().await.unwrap();
        let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool));
        let cache = Arc::new(TokenAuthCache::new(
            token_store,
            Arc::new(MemoryCache::new()),
            "inst",
        ));
        let ap = AppPasswordService::new(cache, SecretString::new("s".into()));

        let (id, raw) = super::app_password_add(&ap, "alice", "DAV").await.unwrap();
        assert!(id > 0);
        assert!(raw.len() > 50);
        let listed = super::app_password_list(&ap, "alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, id);
        super::app_password_revoke(&ap, id).await.unwrap();
        let empty = super::app_password_list(&ap, "alice").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn group_add_and_remove_member_round_trip() {
        let svc = fresh_svc().await;
        user_add(&svc, "carol", "pw", None, None, false)
            .await
            .unwrap();
        group_add_member(&svc, "admin", "carol").await.unwrap();
        assert!(svc.is_admin(&UserId::new("carol").unwrap()).await.unwrap());
        group_remove_member(&svc, "admin", "carol").await.unwrap();
        assert!(!svc.is_admin(&UserId::new("carol").unwrap()).await.unwrap());
    }
}
