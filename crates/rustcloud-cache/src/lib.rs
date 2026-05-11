//! Cache abstraction for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.1.

mod memory;
mod trait_def;
mod typed;

// `MemoryCache` is re-exported in Task 2; `TypedCache` in Task 3.
pub use trait_def::{Cache, CacheError, CacheResult};
