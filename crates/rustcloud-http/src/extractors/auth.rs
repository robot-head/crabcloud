//! `AuthenticatedUser` and `OptionalUser` axum extractors. Phase 3 resolves
//! the user purely from the session cookie — Bearer/Basic/app-password auth
//! lands later.

use crate::session::SessionHandle;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub auth_method: AuthMethod,
}

#[derive(Debug, Clone)]
pub enum AuthMethod {
    Session,
    // Bearer / Basic / AppPassword variants land in the users sub-project.
}

pub struct UnauthorizedRejection;

impl IntoResponse for UnauthorizedRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = UnauthorizedRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let handle = parts
            .extensions
            .get::<SessionHandle>()
            .cloned()
            .ok_or(UnauthorizedRejection)?;
        let session = handle.read().await;
        let user_id = session.user_id.ok_or(UnauthorizedRejection)?;
        Ok(AuthenticatedUser {
            user_id,
            auth_method: AuthMethod::Session,
        })
    }
}

/// `Option<AuthenticatedUser>`-style extractor for handlers that work for both
/// anonymous and authenticated callers.
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<AuthenticatedUser>);

impl<S> FromRequestParts<S> for OptionalUser
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let handle = parts.extensions.get::<SessionHandle>().cloned();
        if let Some(h) = handle {
            let session = h.read().await;
            if let Some(uid) = session.user_id {
                return Ok(OptionalUser(Some(AuthenticatedUser {
                    user_id: uid,
                    auth_method: AuthMethod::Session,
                })));
            }
        }
        Ok(OptionalUser(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, SessionHandle, SessionId};
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn auth_only(user: AuthenticatedUser) -> String {
        user.user_id
    }
    async fn optional(opt: OptionalUser) -> String {
        opt.0.map(|u| u.user_id).unwrap_or_else(|| "guest".into())
    }

    fn handle_with(user: Option<&str>) -> SessionHandle {
        let mut s = Session::new();
        s.user_id = user.map(String::from);
        SessionHandle {
            id: SessionId("00".into()),
            inner: Arc::new(Mutex::new(s)),
            destroy: Arc::new(Mutex::new(false)),
        }
    }

    fn app() -> Router {
        Router::new()
            .route("/auth", get(auth_only))
            .route("/opt", get(optional))
    }

    #[tokio::test]
    async fn authenticated_user_rejects_when_no_session() {
        let req = Request::builder().uri("/auth").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_user_rejects_when_session_has_no_user() {
        let req = Request::builder()
            .uri("/auth")
            .extension(handle_with(None))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_user_resolves_when_session_has_user() {
        let req = Request::builder()
            .uri("/auth")
            .extension(handle_with(Some("alice")))
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "alice");
    }

    #[tokio::test]
    async fn optional_user_is_none_for_anon() {
        let req = Request::builder().uri("/opt").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "guest");
    }
}
