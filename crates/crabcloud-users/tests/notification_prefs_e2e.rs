//! Integration tests for [`crabcloud_users::NotificationPrefs`] against a
//! real sqlite database (with migrations applied). Other dialects are
//! covered by sharing's e2e + the core mail_queue e2e harnesses; this
//! suite just pins the default-true + round-trip semantics.

#![allow(unused_crate_dependencies)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_users::NotificationPrefs;
use std::sync::Arc;
use tempfile::tempdir;

async fn build_prefs() -> (NotificationPrefs, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("prefs.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let prefs = NotificationPrefs::new(Arc::new(pool));
    (prefs, dir)
}

#[tokio::test]
async fn get_returns_true_by_default() {
    let (prefs, _dir) = build_prefs().await;
    assert!(prefs.get("alice", "share_created").await.unwrap());
}

#[tokio::test]
async fn set_then_get_round_trips_false() {
    let (prefs, _dir) = build_prefs().await;
    prefs.set("alice", "share_created", false).await.unwrap();
    assert!(!prefs.get("alice", "share_created").await.unwrap());
}

#[tokio::test]
async fn set_then_set_true_round_trips() {
    let (prefs, _dir) = build_prefs().await;
    prefs.set("alice", "share_created", false).await.unwrap();
    prefs.set("alice", "share_created", true).await.unwrap();
    assert!(prefs.get("alice", "share_created").await.unwrap());
}

#[tokio::test]
async fn set_is_per_event_type() {
    let (prefs, _dir) = build_prefs().await;
    prefs.set("alice", "share_created", false).await.unwrap();
    // share_created opted out, but link_emailed still defaults to true.
    assert!(!prefs.get("alice", "share_created").await.unwrap());
    assert!(prefs.get("alice", "link_emailed").await.unwrap());
}
