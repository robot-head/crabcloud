//! Mailer infrastructure for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-email-notifications-design.md`.
//!
//! Public entry points are [`Mailer`] (send actual mail) and
//! [`render_template`] (compose a [`MailEnvelope`] from an [`EventType`] +
//! [`TemplateContext`]). The queue and worker layers (Batch B) own the
//! "decide to send" path; this crate just transports.

mod envelope;
mod error;
mod mailer;
mod transport;

pub use envelope::{EventType, MailEnvelope};
pub use error::MailError;
pub use mailer::Mailer;
pub use transport::{SmtpSecurity, Transport, TransportConfig, TransportKind};

// Quiet `unused_crate_dependencies` for deps that later modules and Batch
// B/C consumers wire up. Anchored here to avoid drift as those modules land.
use async_trait as _;
use chrono as _;
use crabcloud_i18n as _;
use crabcloud_users as _;
use lettre as _;
use rust_embed as _;
use secrecy as _;
use serde as _;
use serde_json as _;
use tera as _;
use tokio as _;
use tracing as _;
