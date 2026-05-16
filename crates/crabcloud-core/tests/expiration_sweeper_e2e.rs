//! End-to-end tests for `ExpirationWarningSweeper`. Each test seeds an
//! `oc_share` link row by writing INSERT directly (so we control the
//! expiration timestamp + last_warned to the second), then drives one
//! `sweep_once()` and asserts on the queue state + last_warned column.
//!
//! Sqlite-only — multidialect coverage on the underlying SQL lives in
//! `crabcloud-sharing`'s e2e suite. Sweep behavior (gating, stamping,
//! idempotency) is pure logic above that SQL.

#![allow(unused_crate_dependencies)]

use chrono::{Duration, NaiveDateTime, Utc};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{ExpirationWarningSweeper, MailQueue};
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::{FileCache, DIRECTORY_MIMETYPE};
use crabcloud_mail::EventType;
use crabcloud_sharing::Shares;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{
    BcryptVerifier, Email, NotificationPrefs, SqlGroupStore, SqlPreferenceStore, SqlUserStore,
    User, UserId, UsersService,
};
use sqlx::Row as _;
use std::sync::Arc;
use std::time::{Duration as StdDuration, UNIX_EPOCH};
use tempfile::TempDir;

struct Fx {
    pool: Arc<DbPool>,
    shares: Arc<Shares>,
    queue: MailQueue,
    users: UsersService,
    prefs: NotificationPrefs,
    _dir: TempDir,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("sweep.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let pool_arc = Arc::new(pool);
    let users = UsersService::new(
        Arc::new(SqlUserStore::new((*pool_arc).clone())),
        Arc::new(SqlGroupStore::new((*pool_arc).clone())),
        Arc::new(SqlPreferenceStore::new((*pool_arc).clone())),
        Arc::new(BcryptVerifier::new()),
    );
    let filecache = Arc::new(FileCache::new((*pool_arc).clone()));
    let queue = MailQueue::new(pool_arc.clone());
    let prefs = NotificationPrefs::new(pool_arc.clone());
    let shares = Arc::new(Shares::new(
        pool_arc.clone(),
        Arc::new(users.clone()),
        filecache.clone(),
        Arc::new(queue.clone()),
        prefs.clone(),
        "https://test.example".to_string(),
    ));
    Fx {
        pool: pool_arc,
        shares,
        queue,
        users,
        prefs,
        _dir: dir,
    }
}

async fn seed_user(fx: &Fx, uid: &str, email: Option<&str>) {
    let user = User {
        uid: UserId::new(uid).unwrap(),
        display_name: uid.to_string(),
        email: email.map(|e| Email::parse(e).unwrap()),
        enabled: true,
        last_seen: 0,
    };
    fx.users.user_store().create(&user, None).await.unwrap();
}

/// Seed a single folder row in `uid`'s home filecache.
async fn seed_folder(fx: &Fx, uid: &str, path: &str) -> i64 {
    let storage_id = format!("home::{uid}");
    let filecache = FileCache::new((*fx.pool).clone());

    // Root first if missing.
    let root = StoragePath::root();
    if filecache
        .lookup(&storage_id, &root)
        .await
        .unwrap()
        .is_none()
    {
        let md = FileMetadata {
            path: root.clone(),
            kind: FileKind::Directory,
            size: 0,
            mtime: UNIX_EPOCH + StdDuration::from_secs(1_700_000_000),
            etag: ETag::new(),
            mimetype: Mimetype::parse(DIRECTORY_MIMETYPE).unwrap(),
            permissions: Permissions::full(),
        };
        filecache
            .apply(&StorageEvent::DirCreated {
                storage_id: storage_id.clone(),
                path: root.clone(),
                metadata: md,
            })
            .await
            .unwrap();
    }
    let sp = StoragePath::new(path.trim_start_matches('/')).unwrap();
    let md = FileMetadata {
        path: sp.clone(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + StdDuration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(DIRECTORY_MIMETYPE).unwrap(),
        permissions: Permissions::full(),
    };
    filecache
        .apply(&StorageEvent::DirCreated {
            storage_id: storage_id.clone(),
            path: sp.clone(),
            metadata: md,
        })
        .await
        .unwrap();
    filecache
        .lookup(&storage_id, &sp)
        .await
        .unwrap()
        .unwrap()
        .fileid
}

/// Insert a Link row with a specific expiration (so we don't need to
/// fake `Utc::now()` to drive the sweeper into different windows).
async fn seed_link_row(
    fx: &Fx,
    owner: &str,
    fileid: i64,
    file_target: &str,
    token: &str,
    expiration: NaiveDateTime,
) -> i64 {
    let DbPool::Sqlite(p) = fx.pool.as_ref() else {
        unreachable!()
    };
    let res = sqlx::query(
        "INSERT INTO oc_share \
         (share_type, share_with, uid_owner, uid_initiator, parent, item_type, item_source, \
          file_source, file_target, permissions, stime, accepted, expiration, token, password, \
          mail_send, last_warned) \
         VALUES (3, NULL, ?, ?, NULL, 'folder', ?, ?, ?, 1, 0, 1, ?, ?, NULL, 0, NULL)",
    )
    .bind(owner)
    .bind(owner)
    .bind(fileid)
    .bind(fileid)
    .bind(file_target)
    .bind(expiration)
    .bind(token)
    .execute(p)
    .await
    .unwrap();
    res.last_insert_rowid()
}

fn sweeper(fx: &Fx) -> ExpirationWarningSweeper {
    let (sweeper, _shutdown) = ExpirationWarningSweeper::new(
        fx.shares.clone(),
        fx.queue.clone(),
        fx.users.clone(),
        fx.prefs.clone(),
        "https://test.example".to_string(),
    );
    sweeper
}

async fn count_queue(fx: &Fx, event_type: &str) -> i64 {
    let DbPool::Sqlite(p) = fx.pool.as_ref() else {
        unreachable!()
    };
    sqlx::query_scalar("SELECT COUNT(*) FROM oc_mail_queue WHERE event_type = ?")
        .bind(event_type)
        .fetch_one(p)
        .await
        .unwrap()
}

async fn last_warned(fx: &Fx, id: i64) -> Option<NaiveDateTime> {
    let DbPool::Sqlite(p) = fx.pool.as_ref() else {
        unreachable!()
    };
    let row = sqlx::query("SELECT last_warned FROM oc_share WHERE id = ?")
        .bind(id)
        .fetch_one(p)
        .await
        .unwrap();
    row.try_get("last_warned").unwrap()
}

#[tokio::test]
async fn sweep_finds_links_in_24h_window() {
    let fx = fixture().await;
    seed_user(&fx, "alice", Some("alice@example.com")).await;
    let fileid = seed_folder(&fx, "alice", "Vacation").await;
    let exp = (Utc::now() + Duration::hours(12)).naive_utc();
    let id = seed_link_row(&fx, "alice", fileid, "/Vacation", "tok0123456789ABC", exp).await;

    let n = sweeper(&fx).sweep_once().await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(count_queue(&fx, "expiration_warning").await, 1);
    assert!(last_warned(&fx, id).await.is_some(), "last_warned stamped");
}

#[tokio::test]
async fn sweep_skips_already_warned() {
    let fx = fixture().await;
    seed_user(&fx, "alice", Some("alice@example.com")).await;
    let fileid = seed_folder(&fx, "alice", "Vacation").await;
    let exp = (Utc::now() + Duration::hours(12)).naive_utc();
    let _id = seed_link_row(&fx, "alice", fileid, "/Vacation", "tok0123456789ABC", exp).await;

    let n1 = sweeper(&fx).sweep_once().await.unwrap();
    assert_eq!(n1, 1);
    // Second sweep: row is now `last_warned IS NOT NULL`, so it's
    // excluded from the SELECT and not re-processed.
    let n2 = sweeper(&fx).sweep_once().await.unwrap();
    assert_eq!(n2, 0);
    assert_eq!(
        count_queue(&fx, "expiration_warning").await,
        1,
        "only one mail row enqueued across two sweeps"
    );
}

#[tokio::test]
async fn sweep_skips_outside_window() {
    let fx = fixture().await;
    seed_user(&fx, "alice", Some("alice@example.com")).await;
    let fileid = seed_folder(&fx, "alice", "Vacation").await;
    // 48h out — outside the (now, now+24h] window.
    let exp_far = (Utc::now() + Duration::hours(48)).naive_utc();
    let id_far = seed_link_row(
        &fx,
        "alice",
        fileid,
        "/Vacation",
        "tokFFFFFFFFFFFFFF",
        exp_far,
    )
    .await;
    // Already past — sweep should also skip (we only warn before expiry).
    let exp_past = (Utc::now() - Duration::hours(1)).naive_utc();
    let id_past = seed_link_row(
        &fx,
        "alice",
        fileid,
        "/Vacation",
        "tokPPPPPPPPPPPPPP",
        exp_past,
    )
    .await;

    let n = sweeper(&fx).sweep_once().await.unwrap();
    assert_eq!(n, 0);
    assert_eq!(count_queue(&fx, "expiration_warning").await, 0);
    assert!(last_warned(&fx, id_far).await.is_none());
    assert!(last_warned(&fx, id_past).await.is_none());
}

#[tokio::test]
async fn sweep_respects_owner_opt_out() {
    let fx = fixture().await;
    seed_user(&fx, "alice", Some("alice@example.com")).await;
    fx.prefs
        .set("alice", "expiration_warning", false)
        .await
        .unwrap();
    let fileid = seed_folder(&fx, "alice", "Vacation").await;
    let exp = (Utc::now() + Duration::hours(12)).naive_utc();
    let id = seed_link_row(&fx, "alice", fileid, "/Vacation", "tok0123456789ABC", exp).await;

    let n = sweeper(&fx).sweep_once().await.unwrap();
    // Row was processed (counted) but no mail row enqueued because
    // alice opted out. last_warned is stamped anyway so the row is
    // not re-considered next sweep.
    assert_eq!(n, 1);
    assert_eq!(count_queue(&fx, "expiration_warning").await, 0);
    assert!(last_warned(&fx, id).await.is_some());
}

// Anchor the assert_eq macro to silence an `EventType` unused warning
// — pulled in so callers that want to assert on the queued row's event
// type have it available.
const _: fn() = || {
    let _ = EventType::ExpirationWarning;
};
