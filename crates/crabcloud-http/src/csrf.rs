//! CSRF middleware — matches Nextcloud's request-token scheme. Reads the
//! token from `requesttoken` header, compares against the session's
//! `csrf_token`, bypasses entirely for `OCS-APIRequest: true`, for
//! non-authenticated requests, and for non-Session auth methods (Bearer /
//! Basic).
//!
//! Spec §7.4.

use crate::auth_context::{AuthContext, AuthMethod};
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

/// `tower::Layer` that enforces Nextcloud-style CSRF tokens on unsafe methods.
#[derive(Clone, Default)]
pub struct CsrfLayer;

impl CsrfLayer {
    /// Construct the layer with default behavior.
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
            // Bearer / Basic auth: CSRF doesn't apply.
            let method = req.extensions().get::<AuthContext>().map(|c| c.method);
            match method {
                Some(AuthMethod::Bearer) | Some(AuthMethod::Basic) => {
                    return inner.call(req).await;
                }
                _ => {}
            }
            // Anonymous (no AuthContext) bypass — they can't have CSRF state.
            if method.is_none() {
                return inner.call(req).await;
            }
            // Session-auth: require matching token.
            let handle = req.extensions().get::<SessionHandle>().cloned();
            let expected = match &handle {
                Some(h) => h.read().await.csrf_token.clone(),
                None => String::new(),
            };
            let supplied = req
                .headers()
                .get(&TOKEN_HEADER)
                .and_then(|v| v.to_str().ok());
            // Defense-in-depth: refuse to authorize an unsafe request when
            // either side of the comparison is empty. An empty expected
            // token would otherwise admit a request whose supplied token is
            // also empty (e.g., `requesttoken: `), turning a cache-miss on
            // the session blob into a silent CSRF bypass.
            let valid = !expected.is_empty()
                && supplied
                    .map(|s| !s.is_empty() && s == expected)
                    .unwrap_or(false);
            if valid {
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
    use crate::auth_context::{AuthContext, AuthMethod};
    use crate::session::SessionHandle;
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use crabcloud_users::UserId;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn handler() -> &'static str {
        "ok"
    }

    fn session_handle_with_token(token: &str) -> SessionHandle {
        let mut s = crate::session::Session::new();
        s.csrf_token = token.into();
        SessionHandle {
            token_id: Some(1),
            inner: Arc::new(Mutex::new(s)),
            pending_cookie: Arc::new(Mutex::new(None)),
        }
    }

    fn auth_ctx(method: AuthMethod) -> AuthContext {
        AuthContext {
            user_id: UserId::new("alice").unwrap(),
            method,
            token_id: 1,
            login_name: "alice".into(),
            remember: false,
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
            .extension(auth_ctx(AuthMethod::Session))
            .extension(session_handle_with_token("expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_auth_bypasses_csrf() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .extension(auth_ctx(AuthMethod::Bearer))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn basic_auth_bypasses_csrf() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .extension(auth_ctx(AuthMethod::Basic))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_session_post_without_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .extension(auth_ctx(AuthMethod::Session))
            .extension(session_handle_with_token("expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticated_session_post_with_matching_token_passes() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "expected")
            .extension(auth_ctx(AuthMethod::Session))
            .extension(session_handle_with_token("expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_session_post_with_mismatching_token_is_forbidden() {
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "wrong")
            .extension(auth_ctx(AuthMethod::Session))
            .extension(session_handle_with_token("expected"))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn empty_expected_csrf_rejects_unsafe_post() {
        // Regression for the Batch D bug: if the SessionLayer ever produced
        // a Session with an empty csrf_token (e.g., cache miss + a defaulted
        // blob), the CSRF middleware must NOT accept an empty supplied
        // header — that would be a silent auth bypass.
        let req = Request::builder()
            .method("POST")
            .uri("/danger")
            .header("requesttoken", "")
            .extension(auth_ctx(AuthMethod::Session))
            .extension(session_handle_with_token(""))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
