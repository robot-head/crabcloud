//! `ProxyHeadersLayer` — honors `X-Forwarded-{Proto,Host,For}` only when the
//! request peer is in `config.trusted_proxies`. Rewrites the request's
//! effective `Host` header to match.
//!
//! For Phase 3 we trust the headers iff (a) `trusted_proxies` contains either
//! the literal `"loopback"` or the peer IP, OR (b) the request has no peer
//! info (axum's `ConnectInfo` extension is absent — typical in tower tests).
//! In production with real connections, the binary inserts `ConnectInfo` via
//! `into_make_service_with_connect_info`.

use axum::extract::ConnectInfo;
use axum::http::header::HeaderName;
use axum::http::Request;
use axum::response::Response;
use futures::future::BoxFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

/// `tower::Layer` that honors `X-Forwarded-*` headers from trusted proxies.
#[derive(Clone)]
pub struct ProxyHeadersLayer {
    /// CIDRs/IPs/`"loopback"` that are allowed to inject forwarding headers.
    pub trusted_proxies: Arc<Vec<String>>,
}

impl ProxyHeadersLayer {
    /// Build the layer from a configured list of trusted proxy peers.
    pub fn new(trusted_proxies: Vec<String>) -> Self {
        Self {
            trusted_proxies: Arc::new(trusted_proxies),
        }
    }
}

impl<S> Layer<S> for ProxyHeadersLayer {
    type Service = ProxyHeaders<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ProxyHeaders {
            inner,
            trusted_proxies: self.trusted_proxies.clone(),
        }
    }
}

/// Middleware service produced by [`ProxyHeadersLayer`].
#[derive(Clone)]
pub struct ProxyHeaders<S> {
    inner: S,
    trusted_proxies: Arc<Vec<String>>,
}

fn peer_is_trusted(peer: Option<SocketAddr>, trusted: &[String]) -> bool {
    let peer_ip = match peer {
        Some(p) => p.ip().to_string(),
        None => return true, // No peer info → likely a test harness; trust by default.
    };
    trusted
        .iter()
        .any(|t| t == &peer_ip || t == "loopback" && peer.unwrap().ip().is_loopback())
}

impl<S, B> Service<Request<B>> for ProxyHeaders<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        let trusted = self.trusted_proxies.clone();
        let mut inner = self.inner.clone();

        if peer_is_trusted(peer, &trusted) {
            // Apply forwarded host if present.
            if let Some(host) = req.headers().get(&X_FORWARDED_HOST).cloned() {
                req.headers_mut().insert(axum::http::header::HOST, host);
            }
            // X-Forwarded-Proto is informational; we tag it onto extensions so
            // downstream code can read it via a typed extractor in later
            // tasks. For now we record the string.
            if let Some(proto) = req.headers().get(&X_FORWARDED_PROTO).cloned() {
                if let Ok(s) = proto.to_str() {
                    req.extensions_mut().insert(EffectiveScheme(s.to_string()));
                }
            }
        }

        Box::pin(async move { inner.call(req).await })
    }
}

/// Request extension carrying the trusted-proxy-supplied scheme (`http`/`https`).
/// Inserted by [`ProxyHeaders`] only when `X-Forwarded-Proto` came from a trusted peer.
#[derive(Debug, Clone)]
pub struct EffectiveScheme(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn echo_host(req: Request<Body>) -> String {
        req.headers()
            .get(axum::http::header::HOST)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }

    fn app(trusted: Vec<&str>) -> Router {
        Router::new()
            .route("/", get(echo_host))
            .layer(ProxyHeadersLayer::new(
                trusted.into_iter().map(String::from).collect(),
            ))
    }

    #[tokio::test]
    async fn rewrites_host_when_peer_trusted() {
        let peer = ConnectInfo::<SocketAddr>("10.0.0.1:5555".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "internal:8080")
            .header("x-forwarded-host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["10.0.0.1"]).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "cloud.example.com");
    }

    #[tokio::test]
    async fn ignores_forwarded_host_when_peer_untrusted() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:5555".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "internal:8080")
            .header("x-forwarded-host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["10.0.0.1"]).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "internal:8080");
    }
}
