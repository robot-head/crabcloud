//! Layered configuration for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §5.

mod loader;
mod types;

pub use loader::{load, LoadError};
pub use types::{CacheConfig, DbType, FileConfig, FileConfigError};
