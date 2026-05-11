//! CSRF middleware — matches Nextcloud's request-token scheme. Reads the
//! token from `requesttoken` header (or query/form field), compares against
//! the session's `csrf_token`, bypasses entirely for `OCS-APIRequest: true`
//! and for non-authenticated requests.
//!
//! Spec §7.4.

use crate::session::SessionHandle;
use axum::http::{HeaderName, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use std::task::{Context, Poll};
use tower::{Layer, Service};

const TOKEN_HEADER: HeaderName = HeaderName::from_static("requesttoken");
const OCS_APIREQUEST_HEADER: HeaderName = HeaderName::from_static("ocs-apirequest");

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

#[derive(Clone, Default)]
pub struct CsrfLayer;

impl CsrfLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        CsrfMiddleware { inner }
    }
}

#[derive(Clone)]
pub struct CsrfMiddleware<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for CsrfMiddleware<S>
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
            // Safe methods bypass.
            if is_safe_method(req.method()) {
                return inner.call(req).await;
            }
            // OCS-APIRequest header bypass (Nextcloud convention).
            if req
                .headers()
                .get(&OCS_APIREQUEST_HEADER)
                .map(|v| v.as_bytes() == b"true")
                .unwrap_or(false)
            {
                return inner.call(req).await;
            }
            // Anonymous (no session user) bypass.
            let handle = req.extensions().get::<SessionHandle>().cloned();
            let user_id = match &handle {
                Some(h) => h.read().await.user_id.clone(),
                None => None,
            };
            if user_id.is_none() {
                return inner.call(req).await;
            }
            // Authenticated session: require matching token.
            let expected = match &handle {
                Some(h) => h.read().await.csrf_token.clone(),
                None => String::new(),
            };
            let supplied = req
                .headers()
                .get(&TOKEN_HEADER)
                .and_then(|v| v.to_str().ok());
            if supplied.map(|s| s == expected).unwrap_or(false) {
                inner.call(req).await
            } else {
                Ok((StatusCode::FORBIDDEN, "csrf token missing or mismatched").into_response())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionHandle;
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn handler() -> &'static str {
        "ok"
    }

    fn handle_with_user(user: Option<&str>, token: &str) -> SessionHandle {
        let mut s = crate::session::Session::new();
        s.user_id = user.map(String::from);
        s.csrf_token = token.into();
        SessionHandle {
            id: crate::session::SessionId("00".into()),
            inner: Arc::new(Mutex::new(s)),
            destroy: Arc::new(Mutex::new(false)),
        }
    }

    fn app() -> Router {
        Router::new()
            .route("/safe", get(handler))
            .route("/danger", post(handler))
            .layer(CsrfLayer::new())
    }

    #[tokio::test]
    async fn safe_method_passes_without_token() {
        let req = Request::builder()
            .method("GET")
            .uri("/safe")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn anonymous_post_passes_without_token() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ocs_apirequest_bypasses_check() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("ocs-apirequest", "true")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_post_without_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticated_post_with_matching_token_passes() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "expected")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_post_with_mismatching_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "wrong")
            .extension(handle_with_user(Some("alice"), "expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
