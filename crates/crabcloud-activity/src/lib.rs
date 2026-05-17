//! Activity feed service for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-activity-feed-design.md`.
//!
//! Public entry points: [`Activity`] (emit/list/sweep), [`ActivitySettings`]
//! (per-user-per-event-type opt-out), and the [`ActivityEmitter`] trait
//! emitter crates depend on. SQL dispatch mirrors the
//! `crabcloud-versions` / `crabcloud-trash` pattern.

mod emitter;
mod error;
mod service;
mod settings;
mod sql;
mod subjects;
mod types;

pub use emitter::{ActivityEmitter, NoopEmitter};
pub use error::{ActivityEmitError, ActivityError};
pub use service::Activity;
pub use settings::ActivitySettings;
pub use subjects::render_subject;
pub use types::{ActivityEvent, ActivityRow, ActivitySetting, EventType, ObjectType};
