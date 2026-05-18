//! sqlite e2e for the Activity service + ActivitySettings + coalescing.

#![allow(unused_crate_dependencies)]

use crabcloud_activity::{
    Activity, ActivityEmitter, ActivityEvent, ActivitySettings, EventType, ObjectType,
};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_users::UserId;
use std::sync::Arc;
use tempfile::TempDir;

async fn setup() -> (Arc<DbPool>, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    (Arc::new(pool), db_dir)
}

fn uid(s: &str) -> UserId {
    UserId::new(s).unwrap()
}

fn event(now: i64, recipients: Vec<&str>, object_id: Option<i64>) -> ActivityEvent {
    ActivityEvent {
        actor: "alice".into(),
        event_type: EventType::FileUpdated,
        subject_id_actor: "file_updated_you".into(),
        subject_id_recipient: "file_updated_by".into(),
        subject_params: serde_json::json!({ "file": "report.docx", "actor": "alice" }),
        object_type: ObjectType::File,
        object_id,
        recipients: recipients.into_iter().map(uid).collect(),
        occurred_at: now,
    }
}

#[tokio::test]
async fn emit_writes_one_row_per_recipient() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

    activity
        .emit(event(1_000, vec!["alice", "bob"], Some(42)))
        .await
        .unwrap();

    let alice_rows = activity.list("alice", None, 100).await.unwrap();
    let bob_rows = activity.list("bob", None, 100).await.unwrap();
    assert_eq!(alice_rows.len(), 1);
    assert_eq!(bob_rows.len(), 1);
    assert_eq!(alice_rows[0].count, 1);
    assert_eq!(alice_rows[0].subject_id, "file_updated_you");
    assert_eq!(bob_rows[0].subject_id, "file_updated_by");
}

#[tokio::test]
async fn emit_coalesces_within_window() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

    activity
        .emit(event(1_000, vec!["alice"], Some(42)))
        .await
        .unwrap();
    activity
        .emit(event(1_100, vec!["alice"], Some(42)))
        .await
        .unwrap();
    activity
        .emit(event(1_200, vec!["alice"], Some(42)))
        .await
        .unwrap();

    let rows = activity.list("alice", None, 100).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "three emits within window should coalesce into one row"
    );
    assert_eq!(rows[0].count, 3);
    assert_eq!(rows[0].last_seen_at, 1_200);
    assert_eq!(rows[0].occurred_at, 1_000);
}

#[tokio::test]
async fn emit_outside_window_inserts_new() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, /*coalesce_window_secs*/ 600);

    activity
        .emit(event(1_000, vec!["alice"], Some(42)))
        .await
        .unwrap();
    activity
        .emit(event(2_000, vec!["alice"], Some(42)))
        .await
        .unwrap(); // 1000s later

    let rows = activity.list("alice", None, 100).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].id > rows[1].id, "DESC order");
    assert_eq!(rows[0].count, 1);
    assert_eq!(rows[1].count, 1);
}

#[tokio::test]
async fn emit_skips_recipient_with_stream_disabled_unless_actor() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    // Bob has opted out of file_updated stream entries.
    settings
        .set("bob", "file_updated", /*stream*/ false)
        .await
        .unwrap();
    // Alice (the actor) also opted out — but the actor row is exempt.
    settings.set("alice", "file_updated", false).await.unwrap();

    let activity = Activity::new(pool.clone(), settings, 600);
    activity
        .emit(event(1_000, vec!["alice", "bob"], Some(42)))
        .await
        .unwrap();

    assert_eq!(
        activity.list("alice", None, 100).await.unwrap().len(),
        1,
        "actor row is exempt from opt-out"
    );
    assert_eq!(
        activity.list("bob", None, 100).await.unwrap().len(),
        0,
        "non-actor opt-out skips the row"
    );
}

#[tokio::test]
async fn list_paginates_by_id_descending() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, 0); // disable coalesce

    for i in 0..5 {
        activity
            .emit(event(1_000 + i * 1000, vec!["alice"], Some(100 + i)))
            .await
            .unwrap();
    }
    let page1 = activity.list("alice", None, 2).await.unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = page1.last().unwrap().id;
    let page2 = activity.list("alice", Some(cursor), 2).await.unwrap();
    assert_eq!(page2.len(), 2);
    assert!(page2[0].id < cursor, "page2 starts strictly before cursor");
}

#[tokio::test]
async fn sweep_expired_deletes_old_rows() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, 0); // disable coalesce

    activity
        .emit(event(1_000, vec!["alice"], Some(1)))
        .await
        .unwrap(); // old
    activity
        .emit(event(9_000, vec!["alice"], Some(2)))
        .await
        .unwrap(); // new
    let n = activity.sweep_expired(5_000).await.unwrap();
    assert_eq!(n, 1);
    let rows = activity.list("alice", None, 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id, Some(2));
}

#[tokio::test]
async fn settings_get_all_returns_set_values() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    settings.set("alice", "file_updated", false).await.unwrap();
    settings.set("alice", "share_created", true).await.unwrap();
    let mut rows = settings.get_all_for_user("alice").await.unwrap();
    rows.sort_by(|a, b| a.event_type.cmp(&b.event_type));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].event_type, "file_updated");
    assert!(!rows[0].stream);
    assert_eq!(rows[1].event_type, "share_created");
    assert!(rows[1].stream);
}

#[tokio::test]
async fn settings_upsert_updates_existing() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    settings.set("alice", "file_updated", true).await.unwrap();
    settings.set("alice", "file_updated", false).await.unwrap();
    let s = settings
        .stream_enabled("alice", "file_updated")
        .await
        .unwrap();
    assert!(!s);
}

#[tokio::test]
async fn settings_default_true_when_no_row() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let s = settings
        .stream_enabled("nobody_set_anything", "file_updated")
        .await
        .unwrap();
    assert!(s);
}

#[tokio::test]
async fn emit_with_object_id_none_coalesces_correctly() {
    let (pool, _d) = setup().await;
    let settings = ActivitySettings::new(pool.clone());
    let activity = Activity::new(pool.clone(), settings, 600);
    activity
        .emit(event(1_000, vec!["alice"], None))
        .await
        .unwrap();
    activity
        .emit(event(1_100, vec!["alice"], None))
        .await
        .unwrap();
    let rows = activity.list("alice", None, 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
}
