//! Background task that drains `oc_mail_queue` and sends via [`Mailer`].
//!
//! Wired by `AppStateBuilder` whenever the configured transport is not
//! [`crabcloud_mail::TransportKind::Disabled`]. The worker polls the
//! queue every 5 seconds (or wakes early on shutdown) and reclaims
//! stuck rows every 5th cycle. Tests construct an `AppState` and then
//! call `state.mail_worker_shutdown.notify_one()` to terminate the
//! background loop cleanly between runs.

use crate::mail_queue::MailQueue;
use crabcloud_mail::{MailEnvelope, Mailer};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Worker that drains the mail queue. Construct via [`MailWorker::new`],
/// then spawn with `tokio::spawn(worker.run())`. The returned
/// `Arc<Notify>` is the shutdown handle: signalling it causes the next
/// idle sleep (or the next pre-batch shutdown check) to return.
pub struct MailWorker {
    queue: MailQueue,
    mailer: Arc<Mailer>,
    shutdown: Arc<Notify>,
}

impl MailWorker {
    /// Construct a new worker. Returns the worker plus a shutdown
    /// handle. Drop both ends when shutting down the process — the
    /// worker terminates after at most one in-flight send completes.
    pub fn new(queue: MailQueue, mailer: Arc<Mailer>) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                queue,
                mailer,
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Run the worker loop until the shutdown handle is signalled.
    pub async fn run(self) {
        let mut cycles = 0u64;
        loop {
            cycles += 1;
            if cycles.is_multiple_of(5) {
                if let Err(e) = self.queue.reclaim_stuck().await {
                    tracing::warn!(error = %e, "mail_worker.reclaim_stuck failed");
                }
            }
            let batch = match self.queue.claim_batch(8).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "mail_worker.claim_batch failed");
                    if self.sleep_or_shutdown(Duration::from_secs(5)).await {
                        return;
                    }
                    continue;
                }
            };
            if batch.is_empty() {
                if self.sleep_or_shutdown(Duration::from_secs(5)).await {
                    return;
                }
                continue;
            }
            for row in batch {
                let env = MailEnvelope {
                    recipient: row.recipient,
                    subject: row.subject,
                    html_body: row.html_body,
                    text_body: row.text_body,
                    event_type: row.event_type,
                };
                match self.mailer.send(&env).await {
                    Ok(()) => {
                        if let Err(e) = self.queue.mark_sent(row.id).await {
                            tracing::warn!(error = %e, id = row.id, "mail_worker.mark_sent failed");
                        }
                    }
                    Err(e) if e.is_transient() && row.attempts < 3 => {
                        if let Err(db_err) = self
                            .queue
                            .mark_failed_retry(row.id, &e.to_string(), row.attempts)
                            .await
                        {
                            tracing::warn!(error = %db_err, id = row.id, "mail_worker.mark_failed_retry failed");
                        }
                    }
                    Err(e) => {
                        if let Err(db_err) = self
                            .queue
                            .mark_failed_permanent(row.id, &e.to_string())
                            .await
                        {
                            tracing::warn!(error = %db_err, id = row.id, "mail_worker.mark_failed_permanent failed");
                        }
                    }
                }
            }
        }
    }

    /// Sleep for `dur` or return `true` if shutdown was signaled.
    async fn sleep_or_shutdown(&self, dur: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(dur) => false,
            _ = self.shutdown.notified() => true,
        }
    }
}
