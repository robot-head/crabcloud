//! The [`ActivityEmitter`] trait. Emitter crates depend on this trait
//! and accept `Arc<dyn ActivityEmitter>` so they don't pull in the
//! concrete service implementation (mirrors `MailEnqueuer` precedent).
//!
//! A [`NoopEmitter`] is provided for tests / configurations that want to
//! skip activity logging without an `Option<Arc<...>>` plumbing dance.

use crate::error::ActivityEmitError;
use crate::types::ActivityEvent;
use async_trait::async_trait;

#[async_trait]
pub trait ActivityEmitter: Send + Sync {
    /// **Atomicity caveat:** the per-recipient loop is not transactional —
    /// each recipient gets its own SELECT+INSERT/UPDATE pair. A panic or
    /// connection loss mid-loop can leave some recipients with the row and
    /// others without. Activity is best-effort; emit failures are not
    /// retried. Callers must not wrap `emit` in a transaction expecting
    /// atomicity across recipients.
    async fn emit(&self, event: ActivityEvent) -> Result<(), ActivityEmitError>;
}

/// Drops every event. Useful for unit tests and for the boot phase
/// before `Activity` is constructed (the `AppState` builder threads the
/// real emitter through after construction).
pub struct NoopEmitter;

#[async_trait]
impl ActivityEmitter for NoopEmitter {
    async fn emit(&self, _: ActivityEvent) -> Result<(), ActivityEmitError> {
        Ok(())
    }
}
