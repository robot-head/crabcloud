//! End-to-end tests for the `Shares` service across sqlite, mysql, and
//! postgres. Each scenario is a free async fn taking a `Fixture`; thin
//! `#[tokio::test]` wrappers per dialect (sqlite live, mysql + postgres
//! `#[ignore]`) keep the matrix shape obvious.

#![allow(unused_crate_dependencies)]

mod common;

use chrono::NaiveDate;
use common::{
    add_user_to_group, seed_file, seed_group, seed_user, share_request, Fixture, FixtureKind,
};
use crabcloud_sharing::{ShareError, ShareType, UpdateShareFields};
use crabcloud_users::UserId;

// ---------------- create ----------------

async fn create_user_share_happy_path(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/Vacation", true).await;

    let sid = fx.home_storage_id("alice");
    let row = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/Vacation",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    assert!(row.id > 0);
    assert_eq!(row.share_type, ShareType::User);
    assert_eq!(row.share_with.as_deref(), Some("bob"));
    assert_eq!(row.uid_owner, "alice");
    assert_eq!(row.uid_initiator, "alice");
    assert_eq!(row.permissions.as_u32(), 3);
    assert_eq!(row.file_target, "/Vacation");
    assert!(row.accepted);
}

async fn rejects_bit_one_cleared(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let err = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            2,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::BadPermissions), "got {err:?}");
}

async fn strips_bit_16(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let row = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            0x1F,
        ))
        .await
        .unwrap();
    assert_eq!(row.permissions.as_u32(), 0x0F);
}

async fn rejects_reshare_attempt(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_user(fx, "carol").await;
    let _ = seed_file(fx, "alice", "/X", true).await;
    // bob attempts to share alice's /X with carol. Lookup runs against
    // bob's home storage_id, which doesn't contain /X -> PathNotOwned.
    let sid = fx.home_storage_id("bob");
    let err = fx
        .shares
        .create(share_request(
            "bob",
            &sid,
            "/X",
            ShareType::User,
            "carol",
            3,
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ShareError::PathNotOwned | ShareError::ReshareRejected),
        "got {err:?}"
    );
}

async fn rejects_link_share_type(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let err = fx
        .shares
        .create(share_request("alice", &sid, "/X", ShareType::Link, "", 3))
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::NotImplemented), "got {err:?}");
}

async fn rejects_unknown_recipient(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let err = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "nobody",
            3,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::RecipientUnknown), "got {err:?}");
}

// ---------------- read ----------------

async fn get_returns_the_inserted_share(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let got = fx.shares.get(created.id).await.unwrap().expect("present");
    assert_eq!(got.id, created.id);
    assert_eq!(got.share_with.as_deref(), Some("bob"));
}

async fn get_returns_none_for_unknown_id(fx: &Fixture) {
    let got = fx.shares.get(999_999).await.unwrap();
    assert!(got.is_none());
}

async fn list_outgoing_returns_each_share_alice_created(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_user(fx, "carol").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let _ = seed_file(fx, "alice", "/Y", true).await;
    let sid = fx.home_storage_id("alice");
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/Y",
            ShareType::User,
            "carol",
            3,
        ))
        .await
        .unwrap();
    let rows = fx
        .shares
        .list_outgoing(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

async fn list_for_owner_path_filters_by_source(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_user(fx, "carol").await;
    seed_user(fx, "dave").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let _ = seed_file(fx, "alice", "/Y", false).await;
    let sid = fx.home_storage_id("alice");
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "carol",
            3,
        ))
        .await
        .unwrap();
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/Y",
            ShareType::User,
            "dave",
            3,
        ))
        .await
        .unwrap();
    let rows = fx
        .shares
        .list_for_owner_path(&UserId::new("alice").unwrap(), &sid, "/X")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.file_target == "/X"));
}

async fn list_incoming_returns_user_shares(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let rows = fx
        .shares
        .list_incoming(&UserId::new("bob").unwrap())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, created.id);
}

async fn list_incoming_returns_group_shares(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_group(fx, "team").await;
    add_user_to_group(fx, "bob", "team").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::Group,
            "team",
            3,
        ))
        .await
        .unwrap();
    let rows = fx
        .shares
        .list_incoming(&UserId::new("bob").unwrap())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, created.id);
    assert_eq!(rows[0].share_type, ShareType::Group);
}

async fn list_incoming_skips_unaccepted_rows(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    // bob self-unshares.
    fx.shares
        .delete(created.id, &UserId::new("bob").unwrap())
        .await
        .unwrap();
    let rows = fx
        .shares
        .list_incoming(&UserId::new("bob").unwrap())
        .await
        .unwrap();
    assert!(rows.is_empty());
}

// ---------------- update / delete ----------------

async fn update_permissions_owner_can_flip_bits(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let updated = fx
        .shares
        .update(
            created.id,
            &UserId::new("alice").unwrap(),
            UpdateShareFields {
                permissions: Some(1 | 2 | 8),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.permissions.as_u32(), 1 | 2 | 8);
}

async fn update_rejects_non_owner(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let err = fx
        .shares
        .update(
            created.id,
            &UserId::new("bob").unwrap(),
            UpdateShareFields {
                permissions: Some(0x0F),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::Forbidden), "got {err:?}");
}

async fn update_expire_date_round_trips(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let updated = fx
        .shares
        .update(
            created.id,
            &UserId::new("alice").unwrap(),
            UpdateShareFields {
                expire_date: Some(Some(NaiveDate::from_ymd_opt(2030, 1, 2).unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.expiration.is_some());
}

async fn delete_owner_removes_row(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    fx.shares
        .delete(created.id, &UserId::new("alice").unwrap())
        .await
        .unwrap();
    assert!(fx.shares.get(created.id).await.unwrap().is_none());
}

async fn delete_recipient_flips_accepted(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    fx.shares
        .delete(created.id, &UserId::new("bob").unwrap())
        .await
        .unwrap();
    // Row persists (owner still sees it via list_outgoing).
    let row = fx
        .shares
        .get(created.id)
        .await
        .unwrap()
        .expect("still present");
    assert!(!row.accepted);
    // Second recipient delete → NotFound.
    let err = fx
        .shares
        .delete(created.id, &UserId::new("bob").unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::NotFound), "got {err:?}");
}

async fn delete_third_party_forbidden(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_user(fx, "eve").await;
    let _ = seed_file(fx, "alice", "/X", false).await;
    let sid = fx.home_storage_id("alice");
    let created = fx
        .shares
        .create(share_request(
            "alice",
            &sid,
            "/X",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    let err = fx
        .shares
        .delete(created.id, &UserId::new("eve").unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, ShareError::Forbidden), "got {err:?}");
}

// ---------------- per-dialect test wrappers ----------------

macro_rules! per_dialect {
    ($name:ident) => {
        paste::paste! {
            #[tokio::test]
            async fn [<$name _works_on_sqlite>]() {
                let fx = Fixture::new(FixtureKind::Sqlite).await;
                $name(&fx).await;
            }

            #[tokio::test]
            #[ignore = "needs docker / testcontainers"]
            async fn [<$name _works_on_mysql>]() {
                let fx = Fixture::new(FixtureKind::MySql).await;
                $name(&fx).await;
            }

            #[tokio::test]
            #[ignore = "needs docker / testcontainers"]
            async fn [<$name _works_on_postgres>]() {
                let fx = Fixture::new(FixtureKind::Postgres).await;
                $name(&fx).await;
            }
        }
    };
}

per_dialect!(create_user_share_happy_path);
per_dialect!(rejects_bit_one_cleared);
per_dialect!(strips_bit_16);
per_dialect!(rejects_reshare_attempt);
per_dialect!(rejects_link_share_type);
per_dialect!(rejects_unknown_recipient);

per_dialect!(get_returns_the_inserted_share);
per_dialect!(get_returns_none_for_unknown_id);
per_dialect!(list_outgoing_returns_each_share_alice_created);
per_dialect!(list_for_owner_path_filters_by_source);
per_dialect!(list_incoming_returns_user_shares);
per_dialect!(list_incoming_returns_group_shares);
per_dialect!(list_incoming_skips_unaccepted_rows);

per_dialect!(update_permissions_owner_can_flip_bits);
per_dialect!(update_rejects_non_owner);
per_dialect!(update_expire_date_round_trips);
per_dialect!(delete_owner_removes_row);
per_dialect!(delete_recipient_flips_accepted);
per_dialect!(delete_third_party_forbidden);
