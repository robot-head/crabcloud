//! Composition crate for the Crabcloud substrate. Holds `AppState`, the
//! unified `Error` type, the runtime `AppConfigService`, and the
//! `BootstrapHook` extension point.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1.

// `paste` is only referenced from `tests/mail_queue_e2e.rs`. The lib's
// own test target doesn't see it, which triggers
// `unused_crate_dependencies`. Silence it for test builds.
#![cfg_attr(test, allow(unused_crate_dependencies))]

mod activity_sweeper;
mod appconfig;
mod bootstrap;
mod error;
mod expiration_sweeper;
mod mail_queue;
mod mail_queue_cleanup;
mod mail_worker;
mod preview_cache_cleanup;
mod publiclinks;
mod state;
mod trash_sweeper;
mod versions_sweeper;

pub use activity_sweeper::ActivitySweeper;
pub use appconfig::AppConfigService;
pub use bootstrap::{boxed_hook, BootstrapHook, BootstrapRegistry};
pub use error::{CoreResult, Error};
pub use expiration_sweeper::ExpirationWarningSweeper;
pub use mail_queue::{MailQueue, MailQueueError, MailQueueRow};
pub use mail_queue_cleanup::MailQueueCleanup;
pub use mail_worker::MailWorker;
pub use preview_cache_cleanup::PreviewCacheCleanup;
pub use publiclinks::SharesTokenLookup;
pub use state::{AppState, AppStateBuilder};
pub use trash_sweeper::TrashSweeper;
pub use versions_sweeper::VersionsSweeper;
