//! Composition crate for the Rustcloud substrate. Holds `AppState`, the
//! unified `Error` type, the runtime `AppConfigService`, and the
//! `BootstrapHook` extension point.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1.

// `tracing` is declared as a dependency in preparation for the AppConfigService
// instrumentation work in Phase 5 Batch B (Task 4). Suppress the lint until
// the call sites land.
use tracing as _;

mod appconfig;
mod bootstrap;
mod error;
mod state;

pub use appconfig::AppConfigService;
pub use bootstrap::{boxed_hook, BootstrapHook, BootstrapRegistry};
pub use error::{CoreResult, Error};
pub use state::{AppState, AppStateBuilder};
