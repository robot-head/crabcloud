//! Helpers for the server-bin's user/group management subcommands.
//! Pure async functions consuming `UsersService`.

use crate::email::Email;
use crate::error::UsersResult;
use crate::group::GroupId;
use crate::service::UsersService;
use crate::user::{User, UserId};

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
    let user_id = UserId::new(uid)?;
    svc.user_store().delete(&user_id).await?;
    svc.preferences().delete_all_for(&user_id).await?;
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
