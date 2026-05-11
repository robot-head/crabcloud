//! HTTP layer for Crabcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

mod csrf;
mod error;
pub mod extractors;
pub mod middleware;
mod router;
mod routes;
pub mod session;

pub use csrf::CsrfLayer;
pub use error::{ApiError, OcsError};
pub use extractors::auth::{AuthMethod, AuthenticatedUser, OptionalUser};
pub use router::build_router;
pub use session::{Session, SessionHandle, SessionId, SessionLayer, SessionStore};
