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
use crate::middleware::asset_rewrite::{AssetRewriteLayer, AssetRewriteMap};
use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::session::{SessionLayer, SessionStore};

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

    // Load the dx asset manifest (sibling of `public/`) and apply href
    // rewrites to SSR'd HTML. The native server isn't dx-post-processed,
    // so `Asset` values otherwise serialize to their absolute source
    // paths — see `middleware::asset_rewrite` for the full story.
    let asset_rewrite = AssetRewriteLayer::new(AssetRewriteMap::from_env());

    // OCS + app (Dioxus SSR + server functions) sub-router. Wrapped in CORS
    // and CSRF below. The asset rewrite layer sits inside CSRF/CORS so it
    // runs *after* SSR produces a body but before the response is shipped.
    let ocs_app = app_router.layer(asset_rewrite).nest(
        "/ocs",
        crate::routes::ocs::router().with_state(state.clone()),
    );

    Router::new()
        .merge(dav_router)
        .merge(ocs_app.layer(CsrfLayer::new()).layer(cors_layer))
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
