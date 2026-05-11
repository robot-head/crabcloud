//! `TrustedDomainLayer` — rejects requests whose effective `Host` isn't in
//! `config.trusted_domains`. Loopback peers are exempt so the `/status.php`
//! probe in a fresh install works.
//!
//! Spec §7.2.

use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

#[derive(Clone)]
pub struct TrustedDomainLayer {
    pub allowed: Arc<Vec<String>>,
}

impl TrustedDomainLayer {
    pub fn new(allowed: Vec<String>) -> Self {
        Self {
            allowed: Arc::new(allowed),
        }
    }
}

impl<S> Layer<S> for TrustedDomainLayer {
    type Service = TrustedDomain<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TrustedDomain {
            inner,
            allowed: self.allowed.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TrustedDomain<S> {
    inner: S,
    allowed: Arc<Vec<String>>,
}

fn host_in_list(host: &HeaderValue, list: &[String]) -> bool {
    let Ok(s) = host.to_str() else { return false };
    // Strip port if present.
    let bare = s.split(':').next().unwrap_or(s);
    list.iter().any(|d| d == bare)
}

fn peer_is_loopback(peer: Option<SocketAddr>) -> bool {
    match peer {
        Some(p) => p.ip().is_loopback(),
        None => true, // No connect info → tests; allow.
    }
}

impl<S, B> Service<Request<B>> for TrustedDomain<S>
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

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        let allowed = self.allowed.clone();
        let host = req.headers().get(axum::http::header::HOST).cloned();
        let mut inner = self.inner.clone();

        if peer_is_loopback(peer) {
            return Box::pin(async move { inner.call(req).await });
        }
        match host {
            Some(h) if host_in_list(&h, &allowed) => Box::pin(async move { inner.call(req).await }),
            _ => {
                Box::pin(
                    async move { Ok((StatusCode::BAD_REQUEST, "untrusted host").into_response()) },
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn ok() -> &'static str {
        "ok"
    }

    fn app(trusted: Vec<&str>) -> Router {
        Router::new()
            .route("/", get(ok))
            .layer(TrustedDomainLayer::new(
                trusted.into_iter().map(String::from).collect(),
            ))
    }

    #[tokio::test]
    async fn allows_request_without_connect_info() {
        // No ConnectInfo means loopback / test → allow.
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example.com")
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_untrusted_host_from_non_loopback_peer() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn allows_trusted_host_from_non_loopback_peer() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "cloud.example.com")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn strips_port_when_matching() {
        let peer = ConnectInfo::<SocketAddr>("203.0.113.10:12345".parse().unwrap());
        let req = Request::builder()
            .uri("/")
            .header("host", "cloud.example.com:8443")
            .extension(peer)
            .body(Body::empty())
            .unwrap();
        let resp = app(vec!["cloud.example.com"]).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
