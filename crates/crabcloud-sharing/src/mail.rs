//! `MailEnqueuer` — abstract handoff from `Shares` to the persistent
//! mail queue.
//!
//! Defined here (rather than in `crabcloud-core`) so the sharing crate
//! can call `enqueue()` without taking a dependency on `crabcloud-core`.
//! `crabcloud-core::MailQueue` implements this trait so `AppStateBuilder`
//! can plug it into `Shares::new` without code in `crabcloud-sharing`
//! ever naming `MailQueue` directly.
//!
//! Failures returned from `enqueue` are stringly-typed at the trait
//! boundary (`MailEnqueueError(String)`) — the sharing crate logs and
//! drops them, never propagating a mail-side failure into a share
//! create/update result.

use async_trait::async_trait;
use crabcloud_mail::MailEnvelope;
use thiserror::Error;

/// Wrap any mail-side enqueue failure as a single stringly-typed variant
/// so the sharing crate doesn't depend on the queue's error type.
#[derive(Debug, Error)]
#[error("mail enqueue failed: {0}")]
pub struct MailEnqueueError(pub String);

/// Trait the `Shares` service uses to hand off envelopes to whatever
/// persistent mail queue is wired in at construction time.
///
/// `crabcloud-core::MailQueue` implements this so `Arc<MailQueue>` is a
/// drop-in `Arc<dyn MailEnqueuer>`. Tests that don't care about mail
/// can pass `Arc::new(NullEnqueuer)`.
#[async_trait]
pub trait MailEnqueuer: Send + Sync {
    async fn enqueue(&self, envelope: &MailEnvelope) -> Result<(), MailEnqueueError>;
}

/// Drop-in `MailEnqueuer` for tests and code paths that explicitly
/// want mail handoffs to be a no-op (e.g. when transport is `Disabled`
/// or when the test fixture doesn't care about mail).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEnqueuer;

#[async_trait]
impl MailEnqueuer for NullEnqueuer {
    async fn enqueue(&self, _envelope: &MailEnvelope) -> Result<(), MailEnqueueError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_mail::EventType;

    #[tokio::test]
    async fn null_enqueuer_is_ok() {
        let env = MailEnvelope {
            recipient: "x@example.com".into(),
            subject: "s".into(),
            html_body: "h".into(),
            text_body: "t".into(),
            event_type: EventType::ShareCreated,
        };
        NullEnqueuer.enqueue(&env).await.unwrap();
    }
}
