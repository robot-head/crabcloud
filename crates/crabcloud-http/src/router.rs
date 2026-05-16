//! axum router composition. `build_router(state, app_router)` merges the
//! caller-supplied Dioxus fullstack router (SSR + assets + server functions)
//! with the OCS REST surface and the shared middleware stack. Outermost-to-
//! innermost layer order follows spec §7.2.

use axum::http::{HeaderValue, Method};
use axum::Router;
use crabcloud_core::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::csrf::CsrfLayer;
use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::session::{SessionLayer, SessionStore};
use axum::extract::State;
use std::sync::Arc;

/// Path-conditional wrapper around `public_link_auth`. The OCS+app sub-router
/// catches every non-DAV request — including ordinary authenticated traffic
/// (`/index.php/login`, `/api/files/list`, the `/apps/files` page, OCS calls,
/// …). The real `public_link_auth` would 404 those because they don't carry a
/// `/s/{token}` prefix, so we gate it on the path explicitly here. Requests
/// matching `/s/...` flow through the real layer; everything else is passed
/// straight to the inner service.
///
// TODO(sp8-followup): replace this path-conditional wrapper with axum's
// `Router::nest("/s", router.route_layer(public_link_auth))`. The current
// shape exists because `crabcloud_publiclinks::auth_layer::extract_token`
// expects the absolute request path (it strips a leading `/s/`); a nested
// router would hand it the post-strip path and short-circuit token parsing.
// Fix: change `extract_token` (and the DAV equivalent) to accept the
// already-stripped path, then drop this wrapper.
async fn public_link_gate(
    state: Arc<crabcloud_publiclinks::PublicLinkAuthState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.uri().path().starts_with("/s/") {
        crabcloud_publiclinks::public_link_auth(
            state,
            crabcloud_publiclinks::AuthSurface::Browser,
            req,
            next,
        )
        .await
    } else {
        next.run(req).await
    }
}

/// Sibling of `public_link_gate` for the DAV surface
/// (`/public.php/dav/files/{token}/...`). Same path-conditional shape:
/// matching paths flow through `public_link_auth(AuthSurface::Dav)`
/// (HTTP Basic + per-token rate limit), every other path is passed
/// through untouched so this wrapper can layer the merged DAV router
/// without touching authed-surface traffic.
///
// TODO(sp8-followup): inherits the same factoring opportunity as
// `public_link_gate` — `extract_token` for the Dav surface still
// expects the absolute request path. Folding both gates into a nested
// router is a single follow-up commit once `extract_token` is taught
// to accept post-strip paths.
async fn public_dav_gate(
    state: Arc<crabcloud_publiclinks::PublicLinkAuthState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.uri().path().starts_with("/public.php/dav/files/") {
        crabcloud_publiclinks::public_link_auth(
            state,
            crabcloud_publiclinks::AuthSurface::Dav,
            req,
            next,
        )
        .await
    } else {
        next.run(req).await
    }
}

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router. `app_router` is the Dioxus fullstack router
/// the caller assembled via `dioxus::server::router(App)`; it handles SSR
/// fallback, asset serving, and `#[server]` function endpoints. We merge it
/// with the OCS REST router and wrap everything in the shared middleware.
pub fn build_router(state: AppState, app_router: Router) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();
    let secret = state.config.secret.clone();
    let cache = state.cache.clone();
    let instance_id = state.config.instanceid.clone();
    let secure_cookies = state
        .config
        .overwrite_protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    let session_store = SessionStore::new(cache, &instance_id);

    let cors_origins: Vec<HeaderValue> = trusted_domains
        .iter()
        .flat_map(|d| {
            [
                HeaderValue::from_str(&format!("https://{d}")).ok(),
                HeaderValue::from_str(&format!("http://{d}")).ok(),
            ]
        })
        .flatten()
        .collect();
    let cors_layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_credentials(true)
        .allow_origin(AllowOrigin::list(cors_origins));

    // WebDAV sub-router. WebDAV uses OPTIONS as a capability probe
    // (`DAV: 1, 2, 3` advertisement); tower-http's CorsLayer short-circuits
    // every OPTIONS request as a CORS preflight before it reaches the inner
    // service, so DAV cannot share the CORS layer with the OCS surface.
    // DAV doesn't need CSRF either (it's authenticated per-request via
    // Bearer/Basic/cookie; there's no form posting that needs token gating).
    // The outer AuthLayer, SecurityHeaders, ProxyHeaders, body limit, panic
    // catcher, and TraceLayer still wrap DAV because they're applied below.
    let dav_router = Router::new()
        .nest(
            "/remote.php/dav",
            crate::routes::dav::dav_router().with_state(state.clone()),
        )
        .nest(
            "/dav",
            crate::routes::dav::dav_router().with_state(state.clone()),
        );

    // Public-link DAV surface (`/public.php/dav/files/{token}/...`). Lives
    // alongside the authed DAV surface — same per-method response shape via
    // the surface-neutral helpers in `routes::dav` — but auth comes from
    // the public-link layer (HTTP Basic against the link's bcrypt hash)
    // and the request's `View` is built from a `PublicLinkMountResolver`.
    // We layer `public_dav_gate` directly here so only this prefix flows
    // through the auth middleware; the outer AuthLayer skips this prefix
    // (see `middleware::auth::AuthMiddleware::call`).
    let pl_auth_state_for_dav = state.publiclinks_auth.clone();
    let public_dav_router = crate::routes::public_dav::router()
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            pl_auth_state_for_dav,
            |State(state): State<Arc<crabcloud_publiclinks::PublicLinkAuthState>>,
             req: axum::extract::Request,
             next: axum::middleware::Next| async move {
                public_dav_gate(state, req, next).await
            },
        ));

    // OCS + app (Dioxus SSR + server functions) sub-router. Wrapped in CORS
    // and CSRF below. The dx-built binary substitutes asset hrefs at link
    // time, so the legacy `AssetRewriteLayer` is no longer needed.
    //
    // `routes::files_zip` mounts the authed folder-zip endpoints
    // (`GET /api/files/zip/...`) alongside the OCS surface: it shares the
    // outer `AuthLayer` (so `Extension<AuthContext>` is present on every
    // request), and goes through the same CSRF + CORS layers via
    // `ocs_app_layered` below. CSRF bypasses safe methods, so a GET
    // request without a session token is admitted as expected.
    let ocs_app = app_router
        .nest(
            "/ocs",
            crate::routes::ocs::router().with_state(state.clone()),
        )
        .merge(crate::routes::files_zip::router().with_state(state.clone()))
        .merge(crate::routes::files_preview::router().with_state(state.clone()));

    // Public-link surface (`/s/{token}/...`). The unlock / download / upload
    // handlers live in `routes::public_link`; the viewer PAGE (`/s/{token}`
    // and `/s/{token}/{*path}`) is rendered by the dx fullstack SSR pipeline
    // inside `ocs_app`. Both need `PublicLinkAuthContext` attached, so we
    // wrap the entire OCS+app+publiclink merged tree in a path-conditional
    // `public_link_auth` middleware: requests matching `/s/...` flow through
    // the real auth layer, every other path is passed through untouched.
    //
    // CSRF is intentionally NOT applied to the public-link surface: it's
    // anonymous (the `pl_<token>` cookie carries the password capability,
    // not session identity), and the unlock POST is the only state-changing
    // operation — protected separately by the per-token rate limiter and
    // the bcrypt verification.
    let public_link_rest = crate::routes::public_link::router().with_state(state.clone());
    let pl_auth_state = state.publiclinks_auth.clone();
    let ocs_app_layered = ocs_app
        .merge(public_link_rest)
        .layer(axum::middleware::from_fn_with_state(
            pl_auth_state,
            |State(state): State<Arc<crabcloud_publiclinks::PublicLinkAuthState>>,
             req: axum::extract::Request,
             next: axum::middleware::Next| async move {
                public_link_gate(state, req, next).await
            },
        ))
        .layer(CsrfLayer::new())
        .layer(cors_layer);

    Router::new()
        .merge(dav_router)
        .merge(public_dav_router)
        .merge(ocs_app_layered)
        // Install AppState as a request extension so `FullstackContext::extension`
        // can pull it from inside `#[server]` function bodies.
        .layer(axum::Extension(state.clone()))
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(crate::middleware::auth::AuthLayer::new(state))
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}
