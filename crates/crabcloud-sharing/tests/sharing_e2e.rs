//! End-to-end tests for the `Shares` service across sqlite, mysql, and
//! postgres. Each scenario is a free async fn taking a `Fixture`; thin
//! `#[tokio::test]` wrappers per dialect (sqlite live, mysql + postgres
//! `#[ignore]`) keep the matrix shape obvious.

#![allow(unused_crate_dependencies)]

mod common;

use chrono::NaiveDate;
use common::{
    add_user_to_group, seed_file, seed_group, seed_user, seed_user_with_email, share_request,
    Fixture, FixtureKind,
};
use crabcloud_sharing::{CreateShareRequest, ShareError, ShareType, UpdateShareFields};
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

async fn link_share_create_persists_token_and_password(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/Photos", true).await;
    let sid = fx.home_storage_id("alice");
    let req = CreateShareRequest {
        requester: "alice".into(),
        path: "/Photos".into(),
        share_type: ShareType::Link,
        share_with: String::new(),
        permissions: 1, // read
        home_storage_id: sid,
        password: Some("hunter2".into()),
        expire_date: None,
    };
    let row = fx.shares.create(req).await.expect("link share creates");
    assert_eq!(row.share_type, ShareType::Link);
    assert!(row.share_with.is_none());
    assert_eq!(row.token.as_deref().map(str::len), Some(15));
    // SP8 Batch A landed bcrypt (not argon2) — workspace consistency with
    // `crabcloud-users`. Stored hash uses `$2a$`/`$2b$`/`$2y$` prefix.
    assert!(
        row.password_hash.as_deref().unwrap().starts_with("$2"),
        "expected bcrypt prefix, got {:?}",
        row.password_hash
    );
    // file_target for link rows stores the full owner path (not just the
    // basename) so resolve_by_token returns a usable mount root.
    assert_eq!(row.file_target, "/Photos");
    assert!(row.accepted);
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

async fn share_counts_for_returns_owner_counts(fx: &Fixture) {
    // alice has two outgoing shares on /X, one on /Y, none on /Z; bob
    // has one outgoing share unrelated to alice. share_counts_for for
    // alice returns a map with X→2, Y→1; Z is absent (the caller
    // defaults missing keys to 0).
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await;
    seed_user(fx, "carol").await;
    seed_user(fx, "dave").await;
    let fid_x = seed_file(fx, "alice", "/X", false).await;
    let fid_y = seed_file(fx, "alice", "/Y", false).await;
    let fid_z = seed_file(fx, "alice", "/Z", false).await;
    let sid_alice = fx.home_storage_id("alice");
    for recipient in ["bob", "carol"] {
        fx.shares
            .create(share_request(
                "alice",
                &sid_alice,
                "/X",
                ShareType::User,
                recipient,
                3,
            ))
            .await
            .unwrap();
    }
    fx.shares
        .create(share_request(
            "alice",
            &sid_alice,
            "/Y",
            ShareType::User,
            "bob",
            3,
        ))
        .await
        .unwrap();
    // bob also shares one of his own files with carol — must NOT appear
    // in alice's count.
    let fid_bob = seed_file(fx, "bob", "/MyFile", false).await;
    let sid_bob = fx.home_storage_id("bob");
    fx.shares
        .create(share_request(
            "bob",
            &sid_bob,
            "/MyFile",
            ShareType::User,
            "carol",
            3,
        ))
        .await
        .unwrap();

    let counts = fx
        .shares
        .share_counts_for(
            &UserId::new("alice").unwrap(),
            &[fid_x, fid_y, fid_z, fid_bob],
        )
        .await
        .unwrap();
    assert_eq!(counts.get(&fid_x).copied(), Some(2));
    assert_eq!(counts.get(&fid_y).copied(), Some(1));
    assert!(!counts.contains_key(&fid_z));
    assert!(!counts.contains_key(&fid_bob));
}

async fn share_counts_for_empty_input_is_empty(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let counts = fx
        .shares
        .share_counts_for(&UserId::new("alice").unwrap(), &[])
        .await
        .unwrap();
    assert!(counts.is_empty());
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

async fn link_share_update_sets_password_and_expiration(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/Photos", true).await;
    let sid = fx.home_storage_id("alice");
    let row = fx
        .shares
        .create(CreateShareRequest {
            requester: "alice".into(),
            path: "/Photos".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1,
            home_storage_id: sid,
            password: None,
            expire_date: None,
        })
        .await
        .unwrap();
    assert!(row.password_hash.is_none());

    let updated = fx
        .shares
        .update(
            row.id,
            &UserId::new("alice").unwrap(),
            UpdateShareFields {
                password: Some(Some("newpw".into())),
                expire_date: Some(Some(NaiveDate::from_ymd_opt(2030, 1, 1).unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.password_hash.is_some());
    assert!(
        updated.password_hash.as_deref().unwrap().starts_with("$2"),
        "expected bcrypt prefix"
    );
    assert!(updated.expiration.is_some());

    // Clear password.
    let cleared = fx
        .shares
        .update(
            row.id,
            &UserId::new("alice").unwrap(),
            UpdateShareFields {
                password: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(cleared.password_hash.is_none());
}

async fn link_share_update_to_file_drop_permissions_accepted(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/Drop", true).await;
    let sid = fx.home_storage_id("alice");
    let row = fx
        .shares
        .create(CreateShareRequest {
            requester: "alice".into(),
            path: "/Drop".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1, // read link
            home_storage_id: sid,
            password: None,
            expire_date: None,
        })
        .await
        .unwrap();
    assert_eq!(row.permissions.as_u8(), 1);

    // Flip read-link to file-drop (bit 4 only). The old validation rejected
    // this because it required bit 1; link rows should follow the same rule
    // as create_link (at least bit 1 or bit 4).
    let updated = fx
        .shares
        .update(
            row.id,
            &UserId::new("alice").unwrap(),
            UpdateShareFields {
                permissions: Some(4),
                ..Default::default()
            },
        )
        .await
        .expect("file-drop permissions accepted on link update");
    assert_eq!(updated.permissions.as_u8(), 4);
}

async fn resolve_by_token_returns_row(fx: &Fixture) {
    seed_user(fx, "alice").await;
    let _ = seed_file(fx, "alice", "/Photos", true).await;
    let sid = fx.home_storage_id("alice");
    let row = fx
        .shares
        .create(CreateShareRequest {
            requester: "alice".into(),
            path: "/Photos".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1,
            home_storage_id: sid,
            password: None,
            expire_date: None,
        })
        .await
        .unwrap();
    let token = row.token.clone().unwrap();
    let found = fx
        .shares
        .resolve_by_token(&token)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(found.id, row.id);
    assert_eq!(found.uid_owner, "alice");
    assert_eq!(found.file_target, "/Photos");
    // Token shape valid but not present in DB.
    assert!(fx
        .shares
        .resolve_by_token("ABCDEFGHIJKLMNO")
        .await
        .unwrap()
        .is_none());
}

// ---------------- notification hooks ----------------

async fn share_created_enqueues_mail_for_user_with_email(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user_with_email(fx, "bob", "bob@example.com").await;
    let _ = seed_file(fx, "alice", "/Vacation", true).await;

    let sid = fx.home_storage_id("alice");
    fx.shares
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

    let recorded = fx.mail.snapshot();
    assert_eq!(recorded.len(), 1, "expected one mail enqueued");
    assert_eq!(recorded[0].recipient, "bob@example.com");
    assert!(
        recorded[0].subject.contains("share"),
        "subject: {}",
        recorded[0].subject
    );
}

async fn share_created_skips_recipient_without_email(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user(fx, "bob").await; // no email
    let _ = seed_file(fx, "alice", "/Vacation", true).await;

    let sid = fx.home_storage_id("alice");
    fx.shares
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
    assert_eq!(fx.mail.len(), 0);
}

async fn share_created_respects_opt_out(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_user_with_email(fx, "bob", "bob@example.com").await;
    let _ = seed_file(fx, "alice", "/Vacation", true).await;
    // Bob opts out of share_created.
    let prefs = crabcloud_users::NotificationPrefs::new(fx.pool.clone());
    prefs.set("bob", "share_created", false).await.unwrap();

    let sid = fx.home_storage_id("alice");
    fx.shares
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
    assert_eq!(fx.mail.len(), 0, "opt-out should suppress mail");
}

async fn share_created_skips_for_group_shares(fx: &Fixture) {
    seed_user(fx, "alice").await;
    seed_group(fx, "team").await;
    seed_user_with_email(fx, "bob", "bob@example.com").await;
    add_user_to_group(fx, "bob", "team").await;
    let _ = seed_file(fx, "alice", "/Vacation", true).await;
    let sid = fx.home_storage_id("alice");
    fx.shares
        .create(share_request(
            "alice",
            &sid,
            "/Vacation",
            ShareType::Group,
            "team",
            3,
        ))
        .await
        .unwrap();
    // Group fan-out is deferred — group shares should NOT enqueue mail.
    assert_eq!(fx.mail.len(), 0);
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
per_dialect!(link_share_create_persists_token_and_password);
per_dialect!(link_share_update_sets_password_and_expiration);
per_dialect!(link_share_update_to_file_drop_permissions_accepted);
per_dialect!(resolve_by_token_returns_row);
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

per_dialect!(share_counts_for_returns_owner_counts);

per_dialect!(share_created_enqueues_mail_for_user_with_email);
per_dialect!(share_created_skips_recipient_without_email);
per_dialect!(share_created_respects_opt_out);
per_dialect!(share_created_skips_for_group_shares);
per_dialect!(share_counts_for_empty_input_is_empty);
