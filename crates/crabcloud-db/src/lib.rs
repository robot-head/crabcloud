//! Multi-dialect database layer for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §6.

mod core_migrations;
mod error;
mod pool;

pub mod migrate;

pub use core_migrations::{core_set, CORE_NAMESPACE};
pub use error::{DbError, DbResult};
pub use migrate::{Migration, MigrationRunner, MigrationSet};
pub use pool::DbPool;
