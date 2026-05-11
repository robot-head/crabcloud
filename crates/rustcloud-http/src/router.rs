//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to.

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;
use tower_http::limit::RequestBodyLimitLayer;

use crate::middleware::proxy_headers::ProxyHeadersLayer;
use crate::middleware::security_headers::SecurityHeadersLayer;
use crate::middleware::trusted_domain::TrustedDomainLayer;
use crate::routes::status;

/// Default request body limit (matches spec §7.2): 512 MiB.
const DEFAULT_BODY_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    let trusted_proxies = state.config.trusted_proxies.clone();
    let trusted_domains = state.config.trusted_domains.clone();

    Router::new()
        .route("/status.php", get(status::handler))
        .with_state(state)
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
}
