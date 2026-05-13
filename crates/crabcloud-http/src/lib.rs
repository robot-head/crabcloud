//! HTTP layer for Crabcloud — axum router, middleware stack, session + CSRF,
//! and Phase 3's concrete handlers.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §7.

// The `cookie` crate is a declared workspace dep but is not currently
// referenced from this crate's code (cookie encode/decode is hand-rolled in
// `session::cookie`). Keep the dep listed so future cookie work can reach for
// it without re-editing `Cargo.toml`, and silence the unused-crate-dependencies
// lint here.
use cookie as _;

// `quick-xml` and `uuid` are declared up front for SP5 / WebDAV. Their first
// real call sites land in Batches D (PROPFIND/PROPPATCH XML) and F
// (LOCK/UNLOCK lock-token UUIDs). Anchor them here so the workspace-wide
// `unused_crate_dependencies` lint stays quiet during Batches B/C.
use quick_xml as _;
use uuid as _;

mod auth_context;
mod csrf;
mod error;
pub mod extractors;
pub mod middleware;
mod router;
mod routes;
pub mod session;

pub use auth_context::{AuthContext, AuthMethod};
pub use csrf::CsrfLayer;
pub use error::{ApiError, OcsError};
pub use extractors::auth::{AdminUser, AuthenticatedUser, OptionalUser};
pub use middleware::asset_rewrite::{AssetRewriteLayer, AssetRewriteMap};
pub use middleware::auth::AuthLayer;
pub use router::build_router;
pub use session::{PendingCookie, Session, SessionHandle, SessionId, SessionLayer, SessionStore};
