//! `AuthenticatedUser` and `OptionalUser` axum extractors. Phase 3 resolves
//! the user purely from the session cookie — Bearer/Basic/app-password auth
//! lands later.

use crate::error::ApiError;
use crate::session::SessionHandle;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_core::{AppState, Error as CoreError};
use std::convert::Infallible;

/// Extractor that resolves the authenticated user from the session cookie.
/// Returns [`UnauthorizedRejection`] (401) when no valid session is present.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// Identifier of the authenticated user.
    pub user_id: String,
    /// Method used to authenticate the request.
    pub auth_method: AuthMethod,
}

pub use crate::auth_context::AuthMethod;

/// Rejection produced when `AuthenticatedUser` fails to resolve; renders as HTTP 401.
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

/// Extractor that resolves an authenticated user AND verifies the user is in
/// the `admin` group. Returns 401 if unauthenticated, 403 if authenticated but
/// not admin, 500 on backend errors.
#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthenticatedUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let authed = AuthenticatedUser::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError(CoreError::Unauthorized))?;
        let uid = crabcloud_users::UserId::new(&authed.user_id)
            .map_err(|_| ApiError(CoreError::Unauthorized))?;
        let is_admin = state
            .users
            .is_admin(&uid)
            .await
            .map_err(CoreError::Users)
            .map_err(ApiError)?;
        if !is_admin {
            return Err(ApiError(CoreError::Forbidden));
        }
        Ok(AdminUser(authed))
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

    // --- AdminUser tests ---
    //
    // These tests need a real `AppState` so the extractor can resolve
    // `state.users.is_admin`. They build a minimal sqlite-backed state, seed
    // an oc_users row, optionally add it to the `admin` group, then drive a
    // tiny router with a `/admin` route.

    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::{AppState, AppStateBuilder};
    use crabcloud_users::{
        BcryptVerifier, GroupId, GroupStore, PasswordVerifier, SqlGroupStore, User as UserRow,
        UserId,
    };
    use tempfile::tempdir;

    async fn admin_only(AdminUser(user): AdminUser) -> String {
        user.user_id
    }

    async fn make_state_with_user(uid: &str, is_admin: bool) -> AppState {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("admin.db"));
        // Leak the tempdir so the sqlite file outlives this fn.
        std::mem::forget(dir);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        state
            .users
            .user_store()
            .create(
                &UserRow {
                    uid: UserId::new(uid).unwrap(),
                    display_name: uid.into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        if is_admin {
            // Use the underlying SqlGroupStore directly so we don't depend on
            // GroupStore being exposed via UsersService.
            let pool = state.pool.clone();
            let groups = SqlGroupStore::new(pool);
            groups
                .add_to_group(&UserId::new(uid).unwrap(), &GroupId::new("admin").unwrap())
                .await
                .unwrap();
        }
        state
    }

    fn admin_app(state: AppState) -> Router {
        // We bypass SessionLayer here — the unit test inserts the
        // SessionHandle extension directly. The router is statefulish
        // (typed `State<AppState>`), so we attach state via `with_state`.
        Router::new()
            .route("/admin", get(admin_only))
            .with_state(state)
    }

    #[tokio::test]
    async fn admin_user_rejects_when_unauthenticated() {
        let state = make_state_with_user("alice", true).await;
        let req = Request::builder()
            .uri("/admin")
            .body(Body::empty())
            .unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_user_rejects_non_admin_with_403() {
        let state = make_state_with_user("alice", false).await;
        let req = Request::builder()
            .uri("/admin")
            .extension(handle_with(Some("alice")))
            .body(Body::empty())
            .unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_user_resolves_when_in_admin_group() {
        let state = make_state_with_user("root", true).await;
        let req = Request::builder()
            .uri("/admin")
            .extension(handle_with(Some("root")))
            .body(Body::empty())
            .unwrap();
        let resp = admin_app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "root");
    }
}
