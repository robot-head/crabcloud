//! End-to-end test for [`crabcloud_core::MailQueueCleanup`] on sqlite.
//!
//! Mirrors the scaffolding of `mail_queue_e2e.rs` but only exercises the
//! sqlite path — the cleanup query is a single DELETE that we trust
//! sqlx to translate consistently across dialects (and `mail_queue_e2e`
//! already proves the per-dialect placeholder dispatch works end-to-end
//! against the same table).

#![allow(unused_crate_dependencies)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::MailQueueCleanup;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use sqlx::Row as _;
use std::sync::Arc;
use tempfile::tempdir;

async fn insert_row(
    pool: &DbPool,
    recipient: &str,
    state: &str,
    created_at: chrono::NaiveDateTime,
) -> i64 {
    let now = chrono::Utc::now().naive_utc();
    match pool {
        DbPool::Sqlite(p) => {
            let res = sqlx::query(
                "INSERT INTO oc_mail_queue \
                 (recipient, subject, html_body, text_body, event_type, attempts, \
                  next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                 VALUES (?, ?, ?, ?, ?, 0, ?, ?, NULL, NULL, ?, NULL)",
            )
            .bind(recipient)
            .bind("Subject")
            .bind("<p>html</p>")
            .bind("text")
            .bind("share_created")
            .bind(now)
            .bind(state)
            .bind(created_at)
            .execute(p)
            .await
            .unwrap();
            res.last_insert_rowid()
        }
        _ => unreachable!("sqlite-only test"),
    }
}

async fn row_exists(pool: &DbPool, id: i64) -> bool {
    match pool {
        DbPool::Sqlite(p) => {
            let row = sqlx::query("SELECT COUNT(1) AS n FROM oc_mail_queue WHERE id = ?")
                .bind(id)
                .fetch_one(p)
                .await
                .unwrap();
            row.try_get::<i64, _>("n").unwrap() == 1
        }
        _ => unreachable!("sqlite-only test"),
    }
}

#[tokio::test]
async fn cleanup_once_deletes_old_terminal_rows_only() {
    let dir = tempdir().unwrap();
    let cfg = minimal_sqlite_config(dir.path().join("mailq-cleanup.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    let pool = Arc::new(pool);

    let now = chrono::Utc::now().naive_utc();
    let thirty_one_days_ago = now - chrono::Duration::days(31);
    let one_hour_ago = now - chrono::Duration::hours(1);

    // (a) old Sent — should be deleted
    let a = insert_row(&pool, "a@example.com", "Sent", thirty_one_days_ago).await;
    // (b) recent Sent — should remain
    let b = insert_row(&pool, "b@example.com", "Sent", one_hour_ago).await;
    // (c) Pending (always retained regardless of age)
    let c = insert_row(&pool, "c@example.com", "Pending", thirty_one_days_ago).await;
    // (d) old Failed — should be deleted
    let d = insert_row(&pool, "d@example.com", "Failed", thirty_one_days_ago).await;
    // (e) recent Failed — should remain
    let e = insert_row(&pool, "e@example.com", "Failed", one_hour_ago).await;

    let (cleanup, _shutdown) = MailQueueCleanup::new(pool.clone(), 30);
    let deleted = cleanup.cleanup_once().await.unwrap();
    assert_eq!(deleted, 2, "expected (a) and (d) to be deleted");

    assert!(!row_exists(&pool, a).await, "row (a) should be gone");
    assert!(row_exists(&pool, b).await, "row (b) should remain");
    assert!(row_exists(&pool, c).await, "row (c) should remain");
    assert!(!row_exists(&pool, d).await, "row (d) should be gone");
    assert!(row_exists(&pool, e).await, "row (e) should remain");
}
