//! HTTP layer for Rustcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod error;
mod router;
mod routes;

pub use error::{ApiError, OcsError};
pub use router::build_router;
