//! `POST /index.php/login` — bootstrap-admin login. Form-encoded body with
//! `username` and `password`. Bcrypt verification. On success, populates the
//! session and 303-redirects to `/`.

use crate::session::SessionHandle;
use crate::ApiError;
use axum::extract::{Form, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use rustcloud_core::{AppState, Error as CoreError};
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
    use rustcloud_config::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
    use rustcloud_core::AppStateBuilder;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn cfg_with_admin(path: PathBuf, hash: &str) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
            bootstrap_admin: Some(BootstrapAdminConfig {
                username: "admin".into(),
                password_hash: hash.into(),
            }),
        }
    }

    fn valid_login_body(user: &str, pass: &str) -> Body {
        Body::from(format!("username={user}&password={pass}"))
    }

    #[tokio::test]
    async fn correct_credentials_set_session_and_redirect() {
        let dir = tempdir().unwrap();
        let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
        let cfg = cfg_with_admin(dir.path().join("login.db"), &hash);
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
        let cfg = cfg_with_admin(dir.path().join("login.db"), &hash);
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
        let mut cfg = cfg_with_admin(dir.path().join("login.db"), "irrelevant");
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
