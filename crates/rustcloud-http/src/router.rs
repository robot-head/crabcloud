//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to. The outermost-to-innermost layer order
//! follows spec §7.2.

use axum::http::{HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use rustcloud_core::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::csrf::CsrfLayer;
use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::{login, status, ui};
use crate::session::{SessionLayer, SessionStore};

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
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

    Router::new()
        .route("/status.php", get(status::handler))
        .route("/index.php/login", post(login::handler))
        .nest("/ocs", crate::routes::ocs::router())
        .fallback(ui::handler)
        .with_state(state)
        .layer(CsrfLayer::new())
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(cors_layer)
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}
