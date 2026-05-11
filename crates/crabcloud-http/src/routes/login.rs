//! `POST /index.php/login` — bootstrap-admin login. Form-encoded body with
//! `username` and `password`. Bcrypt verification. On success, populates the
//! session and 303-redirects to `/`.

use crate::session::SessionHandle;
use crate::ApiError;
use axum::extract::{Form, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::{AppState, Error as CoreError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Extension(handle): Extension<SessionHandle>,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    let admin = state
        .config
        .bootstrap_admin
        .as_ref()
        .ok_or(CoreError::Unauthorized)?;

    if admin.username != form.username {
        return Err(ApiError(CoreError::Unauthorized));
    }
    let ok = bcrypt::verify(&form.password, &admin.password_hash)
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("bcrypt verify: {e}")))?;
    if !ok {
        return Err(ApiError(CoreError::Unauthorized));
    }

    handle
        .mutate(|s| {
            s.user_id = Some(form.username.clone());
            s.rotate_csrf();
        })
        .await;

    let mut resp = (StatusCode::SEE_OTHER, "").into_response();
    resp.headers_mut()
        .insert(axum::http::header::LOCATION, HeaderValue::from_static("/"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::build_router;
    use axum::body::Body;
    use axum::http::Request;
    use crabcloud_config::test_support::sqlite_config_with_admin;
    use crabcloud_core::AppStateBuilder;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn valid_login_body(user: &str, pass: &str) -> Body {
        Body::from(format!("username={user}&password={pass}"))
    }

    #[tokio::test]
    async fn correct_credentials_set_session_and_redirect() {
        let dir = tempdir().unwrap();
        let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
        let cfg = sqlite_config_with_admin(dir.path().join("login.db"), "admin", &hash);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "hunter2"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/");
        let setc = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(setc.starts_with("oc_sessionPassphrase="));
    }

    #[tokio::test]
    async fn wrong_password_returns_401() {
        let dir = tempdir().unwrap();
        let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
        let cfg = sqlite_config_with_admin(dir.path().join("login.db"), "admin", &hash);
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "WRONG"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_admin_config_returns_401() {
        let dir = tempdir().unwrap();
        let mut cfg = sqlite_config_with_admin(dir.path().join("login.db"), "admin", "irrelevant");
        cfg.bootstrap_admin = None;
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/index.php/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(valid_login_body("admin", "hunter2"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
