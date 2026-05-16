//! End-to-end test: build an `AppState` with `mail.transport = "log"`,
//! enqueue an envelope, and assert the spawned `MailWorker` drains it
//! and emits the `crabcloud_mail::log_transport` tracing event within
//! a couple of polling cycles.

#![allow(unused_crate_dependencies)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_mail::{EventType, MailEnvelope};
use tempfile::tempdir;
use tracing_test::traced_test;

async fn build_state_with_log_transport() -> (AppState, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(dir.path().join("mail_log.db"));
    cfg.mail.transport = "log".to_string();
    // Provide a `mail_from` so any future SMTP path stays valid;
    // unused for transport=log.
    cfg.mail.mail_from = Some("noreply@example.com".into());
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    (state, dir)
}

#[traced_test]
#[tokio::test]
async fn log_transport_drain_emits_tracing_event() {
    let (state, _tmp) = build_state_with_log_transport().await;
    let env = MailEnvelope {
        recipient: "bob@example.com".to_string(),
        subject: "Test".to_string(),
        html_body: "<p>hi</p>".to_string(),
        text_body: "hi".to_string(),
        event_type: EventType::ShareCreated,
    };
    state.mail_queue.enqueue(&env).await.unwrap();

    // Worker polls every 5s; wait up to 12s in 250ms slices. The
    // first claim_batch can fire immediately (no idle sleep before
    // the first batch), so this normally returns within a few hundred
    // milliseconds.
    let captured = poll_for(std::time::Duration::from_secs(12), || {
        logs_contain("mail.transport=log envelope captured")
    })
    .await;

    // Cleanly drain the worker before dropping the state so the
    // background task doesn't outlive the test.
    state.mail_worker_shutdown.notify_one();

    assert!(
        captured,
        "expected log transport to emit envelope tracing event within 12s"
    );
}

async fn poll_for(budget: std::time::Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < budget {
        if cond() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    cond()
}
