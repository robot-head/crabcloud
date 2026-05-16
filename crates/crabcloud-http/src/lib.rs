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

// `zip` is a dev-dep used only by the `files_zip_e2e` integration test
// (it parses the response body to assert archive contents). Anchor it
// here so the lib-test binary doesn't trip the workspace-wide
// `unused_crate_dependencies` lint.
#[cfg(test)]
use zip as _;

// `image` is a dev-dep used only by the `files_preview_e2e` integration
// test (it encodes seeded JPEGs and decodes the response body). Anchor
// it here for the same reason as `zip` above.
#[cfg(test)]
use image as _;

// `crabcloud-mail` is a dev-dep used only by the `files_sharing_e2e`
// integration test (it imports `EventType` to assert the queued row's
// event_type column). Same anchor pattern as `zip` / `image`.
#[cfg(test)]
use crabcloud_mail as _;

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
pub use middleware::auth::AuthLayer;
pub use router::build_router;
pub use session::{PendingCookie, Session, SessionHandle, SessionId, SessionLayer, SessionStore};
