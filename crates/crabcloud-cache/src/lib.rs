//! Cache abstraction for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.1.

mod memory;
mod trait_def;
mod typed;

pub use memory::MemoryCache;
pub use trait_def::{Cache, CacheError, CacheResult};
pub use typed::TypedCache;
