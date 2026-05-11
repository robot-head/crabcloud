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
const CSP_API: &str = "default-src 'none'; frame-ancestors 'self'; base-uri 'self'";
// The single inline `<script>` we emit (`render_hydration_data_script` in
// crabcloud-ui) sets `window.initial_dioxus_hydration_data` to a fixed string
// for dioxus-web 0.7's hydrator. Its SHA-256 is allow-listed below; the body
// is the static contents of `EMPTY_HYDRATION_DATA_BASE64` so the hash never
// changes per response.
const CSP_UI: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval' 'sha256-2Um9bmAkI+7EnHO+y0iR+2Mtb+HfZKZF0ywRt8/LfRI='; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'self'; base-uri 'self'";

fn csp_for_content_type(ct: Option<&axum::http::HeaderValue>) -> &'static str {
    match ct.and_then(|v| v.to_str().ok()) {
        Some(s) if s.starts_with("text/html") => CSP_UI,
        _ => CSP_API,
    }
}

/// `tower::Layer` that appends the spec §7.2 security headers to every response,
/// choosing the CSP variant based on the response `Content-Type`.
#[derive(Clone, Default)]
pub struct SecurityHeadersLayer;

impl SecurityHeadersLayer {
    /// Build the layer with default settings.
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

/// Middleware service produced by [`SecurityHeadersLayer`].
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
            for (name, value) in &[HSTS, XCTO, REFERRER, XFO] {
                headers.insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            let csp = csp_for_content_type(headers.get(axum::http::header::CONTENT_TYPE));
            headers.insert(
                HeaderName::from_static("content-security-policy"),
                HeaderValue::from_static(csp),
            );
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, HeaderValue, Request, Response, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn plain_response() -> &'static str {
        "ok"
    }

    async fn html_response() -> Response<Body> {
        let mut resp = Response::new(Body::from("<html><body>hi</body></html>"));
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        *resp.status_mut() = StatusCode::OK;
        resp
    }

    #[tokio::test]
    async fn all_baseline_security_headers_present() {
        let app = Router::new()
            .route("/", get(plain_response))
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

    #[tokio::test]
    async fn non_html_response_gets_restrictive_csp() {
        let app = Router::new()
            .route("/", get(plain_response))
            .layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.starts_with("default-src 'none'"));
    }

    #[tokio::test]
    async fn html_response_gets_ui_csp_allowing_wasm() {
        let app = Router::new()
            .route("/", get(html_response))
            .layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("'wasm-unsafe-eval'"));
        assert!(csp.contains("script-src 'self'"));
    }
}
