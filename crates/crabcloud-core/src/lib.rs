//! Composition crate for the Crabcloud substrate. Holds `AppState`, the
//! unified `Error` type, the runtime `AppConfigService`, and the
//! `BootstrapHook` extension point.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1.

mod appconfig;
mod bootstrap;
mod error;
mod publiclinks;
mod state;

pub use appconfig::AppConfigService;
pub use bootstrap::{boxed_hook, BootstrapHook, BootstrapRegistry};
pub use error::{CoreResult, Error};
pub use publiclinks::SharesTokenLookup;
pub use state::{AppState, AppStateBuilder};
