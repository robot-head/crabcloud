//! Multi-dialect database layer for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §6.

mod error;
mod pool;

pub mod migrate;

pub use error::{DbError, DbResult};
pub use migrate::{Migration, MigrationRunner, MigrationSet};
pub use pool::DbPool;
