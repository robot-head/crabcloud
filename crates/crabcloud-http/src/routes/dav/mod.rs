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
pub mod lock;
pub mod methods;
pub mod moves;
pub mod propfind;
pub mod proppatch;
pub mod uploads;
pub mod xml;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, options, MethodRouter};
use axum::Router;
use crabcloud_core::AppState;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::DavError;

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
        // Chunked-upload routes per spec §11. MKCOL/DELETE at the root,
        // PUT/MOVE under `/{*part}`.
        .merge(uploads_branch())
}

/// Sub-router for `/uploads/{user}/{upload_id}` and
/// `/uploads/{user}/{upload_id}/{*part}`. Uses `any()` + a method-dispatching
/// handler because WebDAV methods (MKCOL, MOVE) aren't in axum's
/// `MethodRouter` vocabulary.
fn uploads_branch() -> Router<AppState> {
    Router::new()
        .route("/uploads/{user}/{upload_id}", any(dispatch_uploads_root))
        .route(
            "/uploads/{user}/{upload_id}/{*part}",
            any(dispatch_uploads_part),
        )
}

async fn dispatch_uploads_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((user, upload_id)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    _body: Body,
) -> Result<Response, DavError> {
    match method.as_str() {
        "MKCOL" => {
            uploads::mkcol_begin(State(state), authed, headers, Path((user, upload_id))).await
        }
        "DELETE" => uploads::delete_abort(State(state), authed, Path((user, upload_id))).await,
        _ => Ok((StatusCode::METHOD_NOT_ALLOWED, "").into_response()),
    }
}

async fn dispatch_uploads_part(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path((user, upload_id, part)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, DavError> {
    match method.as_str() {
        "PUT" => {
            let part_n: u32 = part
                .parse()
                .map_err(|_| DavError::BadRequest(format!("invalid part: {part}")))?;
            uploads::put_chunk(State(state), authed, Path((user, upload_id, part_n)), body).await
        }
        "MOVE" => {
            // The trailing path is `.file` per Nextcloud convention.
            if part != ".file" {
                return Err(DavError::BadRequest(format!("expected .file, got {part}")));
            }
            uploads::move_commit(State(state), authed, headers, Path((user, upload_id))).await
        }
        _ => Ok((StatusCode::METHOD_NOT_ALLOWED, "").into_response()),
    }
}

/// `MethodRouter` that responds to OPTIONS only with DAV capability.
fn method_options_only() -> MethodRouter<AppState> {
    options(methods::options_capability_root)
}
