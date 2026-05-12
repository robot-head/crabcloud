//! WebDAV route surface. Mounted by `crate::router::build_router` at
//! BOTH `/remote.php/dav` (legacy) and `/dav` (modern alias).
//!
//! Batch B ships OPTIONS / GET / HEAD / PUT / MKCOL / DELETE + conditional
//! headers + single Range. MOVE/COPY (Batch C), PROPFIND (D), PROPPATCH (E),
//! LOCK/UNLOCK (F), and chunked uploads (G) wire into items declared here
//! that are intentionally unused at this batch boundary.
#![allow(dead_code)]

pub mod error;
pub mod extractor;
pub mod headers;
pub mod methods;
pub mod moves;

use axum::routing::{any, options, MethodRouter};
use axum::Router;
use crabcloud_core::AppState;

/// Builds the DAV router. All routes are auth-gated by the outer AuthLayer.
pub fn dav_router() -> Router<AppState> {
    Router::new()
        // /files/{user}/{*path} — the main WebDAV surface.
        // axum 0.8 wildcard path: `{*path}` captures the rest.
        .route("/files/{user}/{*path}", any(methods::dispatch_files))
        // /files/{user} — root of the user's filesystem (path is empty).
        .route("/files/{user}", any(methods::dispatch_files_root))
        // /files (root of all users) — OPTIONS only; returns DAV class.
        .route("/files", method_options_only())
}

/// `MethodRouter` that responds to OPTIONS only with DAV capability.
fn method_options_only() -> MethodRouter<AppState> {
    options(methods::options_capability_root)
}
