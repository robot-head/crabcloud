//! Anonymous public-link WebDAV surface mounted under
//! `/public.php/dav/files/{token}/...`. Reuses the surface-neutral
//! helpers in `routes::dav` (`get_or_head_with_view`, `put_with_view`,
//! `delete_with_view`, `mkcol_with_view`, `propfind::handle_with_view`)
//! by handing them a `View` built from the `PublicLinkAuthContext`
//! attached by the upstream `public_link_auth` middleware.
//!
//! Methods exposed in MVP: OPTIONS, GET/HEAD, PROPFIND, PUT, DELETE,
//! MKCOL. The storage layer (`SharedSubrootStorage`) enforces the
//! per-link permission mask — read-link permission sets don't grant
//! delete/move/copy, so DELETE/MOVE/COPY/MKCOL on a read link fall
//! through to a storage-level 403 rather than needing dedicated logic
//! here. MOVE/COPY are routed to a stub 403 (the public-link MVP
//! permission sets never grant them; wiring `routes::dav::moves` would
//! pull in cross-mount machinery that has no use case here).
//!
//! Auth is layered on top by `build_router` via `route_layer` on the
//! nested DAV router — `AuthSurface::Dav` (HTTP Basic, password
//! challenge, rate limit). By the time a handler runs the context's
//! `password_gate_required` field is always `false` (the auth layer 401s
//! otherwise); we defensively recheck anyway.
#![allow(clippy::result_large_err)]

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Router};
use crabcloud_core::AppState;
use crabcloud_fs::{MountResolver, PublicLinkMountResolver, UserPath, View};
use crabcloud_publiclinks::PublicLinkAuthContext;
use crabcloud_sharing::SharePermissions;
use std::sync::Arc;

use crate::routes::dav::error::DavError;
use crate::routes::dav::methods::{
    delete_with_view, get_or_head_with_view, mkcol_with_view, put_with_view,
};
use crate::routes::dav::propfind::{handle_with_view as propfind_with_view, PropfindContext};

/// Allow header listing the methods the public-link DAV surface accepts.
/// PROPPATCH/LOCK/UNLOCK are intentionally absent — anonymous links don't
/// participate in property metadata or lock arbitration.
const ALLOW_HEADER: &str = "OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, PROPFIND";

/// HREF prefix used in PROPFIND responses for this surface.
const HREF_PREFIX: &str = "/public.php/dav/files";

/// Build the public-link DAV router. Mounted via
/// `Router::nest("/public.php/dav/files", …)` by `build_router`, with a
/// `public_link_auth(AuthSurface::Dav)` `route_layer` applied on top
/// before reaching any handler in here. Routes are nest-relative.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{token}", any(dispatch_root))
        .route("/{token}/", any(dispatch_root))
        .route("/{token}/{*path}", any(dispatch_path))
}

async fn dispatch_root(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path(token): Path<String>,
    headers: HeaderMap,
    method: Method,
    body: Body,
) -> Response {
    dispatch(state, ctx, token, String::new(), headers, method, body)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch_path(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((token, path)): Path<(String, String)>,
    headers: HeaderMap,
    method: Method,
    body: Body,
) -> Response {
    dispatch(state, ctx, token, path, headers, method, body)
        .await
        .unwrap_or_else(|e| e.into_response())
}

async fn dispatch(
    state: AppState,
    ctx: PublicLinkAuthContext,
    token: String,
    url_path: String,
    headers: HeaderMap,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    // Defensive recheck. The DAV auth layer 401s on missing/wrong Basic
    // when a password is required, so this should always be `false` by the
    // time we run; if a future refactor weakens that invariant we'd rather
    // 401 here than silently expose the linked tree.
    if ctx.password_gate_required {
        return Ok(basic_challenge_response());
    }

    // OPTIONS is the only method that doesn't need a `View` — it just
    // advertises the DAV class. Branch before doing the resolver work so
    // capability probes are cheap.
    if method == Method::OPTIONS {
        return Ok(options_capability_response());
    }

    let user_path = parse_user_path(&url_path)?;
    let view = build_view(&state, &ctx).await?;

    // The downstream `View` is built from `SharedSubrootStorage`, which
    // enforces the link's permission mask on storage-touching calls
    // (`put_file`, `mkdir`, `delete`, `rename`). Reads, by contrast, are
    // routed through the filecache layer (`FileCache::stat`/`list`) which
    // queries the OWNER storage directly — the wrapper's read-side
    // carve-outs (e.g. "create-only links hide non-root entries") aren't
    // invoked. Guard the read methods here so a file-drop link can't be
    // probed via PROPFIND/GET against arbitrary child paths.
    let perms = SharePermissions::from_wire(ctx.permissions);
    match method {
        Method::GET | Method::HEAD => {
            if !perms.contains_read() {
                return Ok((StatusCode::FORBIDDEN, "").into_response());
            }
            get_or_head_with_view(&view, &user_path, &headers, method == Method::HEAD).await
        }
        Method::PUT => put_with_view(&view, &user_path, &headers, body).await,
        Method::DELETE => delete_with_view(&view, &user_path).await,
        m if m.as_str() == "MKCOL" => mkcol_with_view(&view, &user_path).await,
        m if m.as_str() == "PROPFIND" => {
            // Allow PROPFIND on the link root for create-only links — the
            // viewer / desktop client needs a successful stat on the
            // upload target so it can render the drop zone. Children stay
            // hidden.
            if !perms.contains_read() && !user_path.is_root() {
                return Ok((StatusCode::FORBIDDEN, "").into_response());
            }
            let pctx = PropfindContext {
                href_prefix: HREF_PREFIX,
                root_label: &token,
                instanceid: state.config.instanceid.as_str(),
            };
            propfind_with_view(
                &view,
                &state.filecache,
                &ctx.owner_uid,
                &user_path,
                &headers,
                &pctx,
            )
            .await
        }
        // MOVE / COPY / PROPPATCH / LOCK / UNLOCK aren't reachable through
        // MVP link permission sets (no link mask grants the relevant
        // storage-side bits), so wiring them adds surface area for no
        // user-visible behaviour. 405 keeps clients honest while still
        // returning the supported Allow set.
        _ => Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, HeaderValue::from_static(ALLOW_HEADER))],
            "",
        )
            .into_response()),
    }
}

/// Parse the captured `{*path}` segment into a `UserPath`. axum's
/// `Path<String>` extractor has already percent-decoded the segment;
/// decoding a second time would mangle filenames containing a literal
/// `%`, so we feed the captured value to `UserPath::new` verbatim.
fn parse_user_path(raw: &str) -> Result<UserPath, DavError> {
    if raw.is_empty() {
        return Ok(UserPath::root());
    }
    UserPath::new(format!("/{raw}")).map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))
}

/// Construct the per-request `View` from the auth context. Same pattern
/// as the browser-facing public-link handlers in `routes::public_link`:
/// goes through `PublicLinkMountResolver` (not `AppState::view_for`) so
/// the wrapped `SharedSubrootStorage` enforces the link's permission
/// mask and the recipient root maps to `owner_path`.
async fn build_view(state: &AppState, ctx: &PublicLinkAuthContext) -> Result<View, DavError> {
    let perms = SharePermissions::from_wire(ctx.permissions);
    let resolver = Arc::new(PublicLinkMountResolver::new(
        state.storage_factory.clone(),
        ctx.owner_uid.clone(),
        ctx.owner_path.clone(),
        perms,
    ));
    let mounts = resolver.mounts_for(&ctx.owner_uid).await?;
    Ok(View::new(
        ctx.owner_uid.clone(),
        mounts,
        state.filecache.clone(),
        state.storage_sink.clone(),
        state.trash.clone(),
    ))
}

/// Mirrors the authed-surface OPTIONS body: `DAV: 1, 2, 3` plus the
/// supported `Allow` set. PROPPATCH/LOCK/UNLOCK are intentionally
/// missing from `Allow`.
fn options_capability_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::ALLOW, HeaderValue::from_static(ALLOW_HEADER)),
            (
                header::HeaderName::from_static("dav"),
                HeaderValue::from_static("1, 2, 3"),
            ),
            (
                header::HeaderName::from_static("ms-author-via"),
                HeaderValue::from_static("DAV"),
            ),
        ],
        "",
    )
        .into_response()
}

/// Defensive 401 with the standard public-link Basic challenge. Only
/// reached if the auth-layer invariant ("password gate already enforced
/// before context-build") regresses; the layer normally 401s us before
/// we run.
fn basic_challenge_response() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"public-link\""),
    );
    resp
}
