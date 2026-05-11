//! Layered configuration for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §5.

mod loader;
#[cfg(feature = "test-support")]
pub mod test_support;
mod types;

pub use loader::{load, LoadError};
pub use types::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig, FileConfigError};
