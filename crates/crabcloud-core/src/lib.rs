//! Composition crate for the Crabcloud substrate. Holds `AppState`, the
//! unified `Error` type, the runtime `AppConfigService`, and the
//! `BootstrapHook` extension point.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1.

// `paste` is only referenced from `tests/mail_queue_e2e.rs`. The lib's
// own test target doesn't see it, which triggers
// `unused_crate_dependencies`. Silence it for test builds.
#![cfg_attr(test, allow(unused_crate_dependencies))]

mod appconfig;
mod bootstrap;
mod error;
mod mail_queue;
mod mail_worker;
mod publiclinks;
mod state;

pub use appconfig::AppConfigService;
pub use bootstrap::{boxed_hook, BootstrapHook, BootstrapRegistry};
pub use error::{CoreResult, Error};
pub use mail_queue::{MailQueue, MailQueueError, MailQueueRow};
pub use mail_worker::MailWorker;
pub use publiclinks::SharesTokenLookup;
pub use state::{AppState, AppStateBuilder};
