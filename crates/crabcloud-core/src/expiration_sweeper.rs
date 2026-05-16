//! Background task: hourly sweep of `oc_share` for link / email-link
//! rows whose `expiration` falls inside the next 24 hours and which
//! have not yet been warned.
//!
//! For each row:
//!   1. Look up the owner's email + display name via `UsersService`.
//!   2. Gate on `expiration_warning` opt-out (default true).
//!   3. If opted in, render the `expiration_warning` template and
//!      enqueue via `MailQueue`.
//!   4. Stamp `last_warned = now()` *unconditionally* so opted-out
//!      rows are not re-considered next sweep.
//!
//! The sweeper exposes `sweep_once()` for tests so they can drive the
//! work synchronously without waiting for the hourly timer. `run()` is
//! the long-running loop wired into `AppStateBuilder::build()`.

use crate::mail_queue::MailQueue;
use crabcloud_mail::{render_template, EventType, TemplateContext};
use crabcloud_sharing::{ExpiringLink, Shares};
use crabcloud_users::{NotificationPrefs, UserId, UsersService};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// How long to sleep between sweeps in `run()`.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// Window the sweep considers — rows whose `expiration` falls in
/// `(now, now + WARN_WINDOW]` are picked up.
const WARN_WINDOW_SECS: i64 = 24 * 3600;
/// Cap on rows processed per sweep so a giant backlog can't starve
/// other tasks. The next sweep picks up where this one left off.
const SWEEP_BATCH: i64 = 200;

#[derive(Clone)]
pub struct ExpirationWarningSweeper {
    shares: Arc<Shares>,
    queue: MailQueue,
    users: UsersService,
    prefs: NotificationPrefs,
    instance_url: String,
    shutdown: Arc<Notify>,
}

impl ExpirationWarningSweeper {
    /// Construct a sweeper + paired shutdown handle. `notify_one()` on
    /// the returned `Arc<Notify>` cancels the `run()` loop after the
    /// current sweep completes.
    pub fn new(
        shares: Arc<Shares>,
        queue: MailQueue,
        users: UsersService,
        prefs: NotificationPrefs,
        instance_url: String,
    ) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                shares,
                queue,
                users,
                prefs,
                instance_url,
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop: sweep, sleep, repeat. Cancels cooperatively
    /// when the paired shutdown `Notify` is notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.sweep_once().await {
                tracing::warn!(error = %e, "expiration_sweeper.sweep_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(SWEEP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single sweep. Exposed `pub` so integration tests can
    /// invoke it directly without waiting for the hourly timer.
    /// Returns the number of rows processed (not the number of mails
    /// actually enqueued — opt-out + missing-email rows count as
    /// "processed" too).
    pub async fn sweep_once(&self) -> Result<usize, crabcloud_sharing::ShareError> {
        let now = chrono::Utc::now().naive_utc();
        let until = now + chrono::Duration::seconds(WARN_WINDOW_SECS);
        let rows = self
            .shares
            .find_expiring_links(now, until, SWEEP_BATCH)
            .await?;
        let count = rows.len();
        for row in rows {
            self.process_row(&row, now).await;
        }
        Ok(count)
    }

    async fn process_row(&self, row: &ExpiringLink, now: chrono::NaiveDateTime) {
        // 1. Resolve owner email + display name.
        let opted_in = self.gate_and_enqueue(row).await;
        // 2. Stamp last_warned UNCONDITIONALLY so opted-out rows are
        //    not re-considered next sweep.
        if let Err(e) = self.shares.stamp_last_warned(row.id, now).await {
            tracing::warn!(
                error = %e,
                row_id = row.id,
                "expiration_sweeper: stamp_last_warned failed"
            );
        }
        // Just a debug breadcrumb so logs include the outcome.
        let _ = opted_in;
    }

    /// Look up the owner's email + prefs and, if opted in, render +
    /// enqueue the expiration_warning template. Returns the opt-in
    /// state so the caller knows whether mail went out (currently
    /// unused — but kept for future telemetry).
    async fn gate_and_enqueue(&self, row: &ExpiringLink) -> bool {
        let uid = match UserId::new(row.uid_owner.clone()) {
            Ok(u) => u,
            Err(_) => return false,
        };
        let user = match self.users.user_store().lookup(&uid).await {
            Ok(Some(u)) => u,
            Ok(None) => return false,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    uid = %uid.as_str(),
                    "expiration_sweeper: owner lookup failed"
                );
                return false;
            }
        };
        let email = match &user.email {
            Some(e) => e.as_str().to_string(),
            None => return false,
        };
        match self
            .prefs
            .get(uid.as_str(), "expiration_warning")
            .await
        {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "expiration_sweeper: prefs.get failed"
                );
                return false;
            }
        }
        let link_url = build_link_url(&self.instance_url, &row.token);
        let basename = Path::new(&row.file_target)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(row.file_target.as_str())
            .to_string();
        let ctx = TemplateContext {
            lang: "en".to_string(),
            instance_url: self.instance_url.clone(),
            recipient_display_name: user.display_name.clone(),
            recipient_email: email,
            event_specific: serde_json::json!({
                "link_basename": basename,
                "link_url": link_url,
                "expiration_dt": row.expiration.date().format("%Y-%m-%d").to_string(),
            }),
        };
        let env = match render_template(EventType::ExpirationWarning, ctx) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "expiration_sweeper: render failed");
                return false;
            }
        };
        if let Err(e) = self.queue.enqueue(&env).await {
            tracing::warn!(error = %e, "expiration_sweeper: queue.enqueue failed");
            return false;
        }
        true
    }
}

/// Build the absolute share-link URL, falling back to `/s/<token>`
/// when `instance_url` is empty. Mirrors `crabcloud_sharing` shape.
fn build_link_url(instance_url: &str, token: &str) -> String {
    let trimmed = instance_url.trim_end_matches('/');
    if trimmed.is_empty() {
        format!("/s/{token}")
    } else {
        format!("{trimmed}/s/{token}")
    }
}
