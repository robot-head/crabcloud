//! `SecurityHeadersLayer` — sets the cluster of security headers spec §7.2
//! requires on every response. CSP differs between API and UI responses;
//! Phase 3 ships the API-restrictive baseline. Phase 4 (UI) will add a
//! per-route override for the Dioxus surface.

use axum::http::header::{HeaderName, HeaderValue};
use axum::http::Request;
use axum::response::Response;
use futures::future::BoxFuture;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const HSTS: (&str, &str) = (
    "strict-transport-security",
    "max-age=31536000; includeSubDomains",
);
const XCTO: (&str, &str) = ("x-content-type-options", "nosniff");
const REFERRER: (&str, &str) = ("referrer-policy", "strict-origin-when-cross-origin");
const XFO: (&str, &str) = ("x-frame-options", "SAMEORIGIN");
const CSP: (&str, &str) = (
    "content-security-policy",
    "default-src 'none'; frame-ancestors 'self'; base-uri 'self'",
);

#[derive(Clone, Default)]
pub struct SecurityHeadersLayer;

impl SecurityHeadersLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeaders<S>;
    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeaders { inner }
    }
}

#[derive(Clone)]
pub struct SecurityHeaders<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for SecurityHeaders<S>
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
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut resp = inner.call(req).await?;
            let headers = resp.headers_mut();
            for (name, value) in &[HSTS, XCTO, REFERRER, XFO, CSP] {
                headers.insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            Ok(resp)
        })
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

    #[tokio::test]
    async fn all_baseline_security_headers_present() {
        let app = Router::new()
            .route("/", get(ok))
            .layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let h = resp.headers();
        assert!(h.get("strict-transport-security").is_some());
        assert!(h.get("x-content-type-options").is_some());
        assert!(h.get("referrer-policy").is_some());
        assert!(h.get("x-frame-options").is_some());
        assert!(h.get("content-security-policy").is_some());
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
    }
}
