//! `GET /ocs/v2.php/cloud/user` and `PUT /ocs/v2.php/cloud/user` — self-only.
//!
//! Self-service endpoints for the authenticated user. `GET` returns the user's
//! own record (id, display-name, email, groups, enabled, last-login). `PUT`
//! accepts a `key`/`value`/`currentpassword` form body and applies a single
//! mutation to the authenticated user (password, displayname, or email).
//! Password changes verify `currentpassword` and revoke every OTHER session
//! belonging to the same uid (keeping the caller's current session alive).

use crate::extractors::auth::AuthenticatedUser;
use crate::extractors::format::OcsFormat;
use crate::session::{SessionHandle, SessionStore};
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct UserPayload {
    id: String,
    #[serde(rename = "display-name")]
    display_name: String,
    email: Option<String>,
    groups: Vec<String>,
    enabled: bool,
    #[serde(rename = "last-login")]
    last_login: u64,
}

fn users_err(e: crabcloud_users::UsersError, fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt)
}

fn unauth(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Unauthorized, OcsVersion::V2, fmt)
}

pub async fn get_self(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&authed.user_id).map_err(|e| users_err(e, fmt.0))?;
    let user = state
        .users
        .lookup(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .ok_or_else(|| unauth(fmt.0))?;
    let groups = state
        .users
        .groups_of(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;

    let payload = UserPayload {
        id: user.uid.into_inner(),
        display_name: user.display_name,
        email: user.email.map(|e| e.as_str().to_string()),
        groups: groups.into_iter().map(|g| g.as_str().to_string()).collect(),
        enabled: user.enabled,
        last_login: user.last_seen,
    };
    let envelope = OcsResponse::ok(payload, OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

#[derive(Debug, Deserialize)]
pub struct PutForm {
    pub key: String,
    pub value: String,
    pub currentpassword: String,
}

pub async fn put_self(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Extension(handle): Extension<SessionHandle>,
    fmt: OcsFormat,
    Form(form): Form<PutForm>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&authed.user_id).map_err(|e| users_err(e, fmt.0))?;
    state
        .users
        .verify(uid.as_str(), &form.currentpassword)
        .await
        .map_err(|_| unauth(fmt.0))?;

    match form.key.as_str() {
        "password" => {
            state
                .users
                .set_password(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            let store = SessionStore::new(state.cache.clone(), &state.config.instanceid);
            if let Err(err) = store
                .destroy_all_for_except(uid.as_str(), Some(&handle.id))
                .await
            {
                ::tracing::warn!(
                    error = ?err,
                    uid = %uid,
                    "destroy_all_for_except failed after password change"
                );
            }
        }
        "displayname" => {
            state
                .users
                .user_store()
                .set_display_name(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
        }
        "email" => {
            let new = if form.value.is_empty() {
                None
            } else {
                Some(form.value.as_str())
            };
            state
                .users
                .user_store()
                .set_email(&uid, new)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
        }
        other => {
            return Err(OcsError::new(
                CoreError::BadRequest(format!("unknown key: {other}")),
                OcsVersion::V2,
                fmt.0,
            ));
        }
    }

    let envelope = OcsResponse::ok(serde_json::json!({}), OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

#[cfg(test)]
mod tests {
    use crate::router::build_router;
    use crate::session::{encode_cookie, Session, SessionId, SessionStore, COOKIE_NAME};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::{AppState, AppStateBuilder};
    use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
    use secrecy::ExposeSecret;
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn make_state(db_path: std::path::PathBuf) -> AppState {
        let cfg = minimal_sqlite_config(db_path);
        AppStateBuilder::new(cfg).build().await.unwrap()
    }

    async fn seed_user(state: &AppState, uid: &str, password: &str) {
        let hash = BcryptVerifier::new().hash(password).unwrap();
        state
            .users
            .user_store()
            .create(
                &User {
                    uid: UserId::new(uid).unwrap(),
                    display_name: format!("{uid} display"),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
    }

    /// Seed an authenticated session directly into the store and return the
    /// `Cookie:` value that the SessionLayer will accept.
    ///
    /// Login itself is a Dioxus `#[server]` function now (`/index.php/login`)
    /// and the verification flow is covered by `crabcloud-users`' Playwright
    /// suite. These OCS tests just need to *be authenticated* to exercise the
    /// `/cloud/user` GET/PUT branches; going through HTTP login from inside
    /// crabcloud-http's tests would require a cargo dev-dep cycle on
    /// crabcloud-ui, which compiles SessionHandle in a separate unit and
    /// breaks extension lookup at runtime via `TypeId` mismatch.
    async fn seed_login(state: &AppState, uid: &str) -> String {
        let store = SessionStore::new(state.cache.clone(), &state.config.instanceid);
        let id = SessionId::new_random();
        let mut session = Session::new();
        session.user_id = Some(uid.to_string());
        session.rotate_csrf();
        session.two_factor_passed = true;
        store.save(&id, &session).await.unwrap();
        store.record_for_user(uid, &id).await.unwrap();
        let cookie_value =
            encode_cookie(id.as_str(), state.config.secret.expose_secret().as_bytes());
        format!("{COOKIE_NAME}={cookie_value}")
    }

    #[tokio::test]
    async fn get_self_returns_authenticated_user() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let cookie = seed_login(&state, "alice").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
        assert_eq!(parsed["ocs"]["data"]["id"], "alice");
        assert_eq!(parsed["ocs"]["data"]["display-name"], "alice display");
        assert_eq!(parsed["ocs"]["data"]["enabled"], true);
    }

    #[tokio::test]
    async fn get_self_without_session_is_unauthorized() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn put_self_password_change_requires_currentpassword() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "old").await;
        let cookie = seed_login(&state, "alice").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from(
                "key=password&value=newpass&currentpassword=WRONG",
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn put_self_password_change_destroys_other_sessions() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "old").await;
        // Two parallel sessions for alice.
        let cookie_a = seed_login(&state, "alice").await;
        let cookie_b = seed_login(&state, "alice").await;
        let app = build_router(state.clone(), axum::Router::new());
        assert_ne!(cookie_a, cookie_b, "sessions should differ");

        // Sanity: session B can currently see itself.
        let req_pre = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie_b.clone())
            .body(Body::empty())
            .unwrap();
        let resp_pre = app.clone().oneshot(req_pre).await.unwrap();
        assert_eq!(resp_pre.status(), StatusCode::OK);

        // Change password from session A.
        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie_a.clone())
            .body(Body::from("key=password&value=newpass&currentpassword=old"))
            .unwrap();
        let put_resp = app.clone().oneshot(put_req).await.unwrap();
        assert_eq!(put_resp.status(), StatusCode::OK);

        // Session B should be gone — GET /user with that cookie returns 401.
        let req_post = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie_b)
            .body(Body::empty())
            .unwrap();
        let resp_post = app.clone().oneshot(req_post).await.unwrap();
        assert_eq!(resp_post.status(), StatusCode::UNAUTHORIZED);

        // Session A still works.
        let req_self = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie_a)
            .body(Body::empty())
            .unwrap();
        let resp_self = app.oneshot(req_self).await.unwrap();
        assert_eq!(resp_self.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn put_self_displayname_updates_record() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let cookie = seed_login(&state, "alice").await;
        let app = build_router(state.clone(), axum::Router::new());

        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from(
                "key=displayname&value=Alice+Wonderland&currentpassword=hunter2",
            ))
            .unwrap();
        let put_resp = app.clone().oneshot(put_req).await.unwrap();
        assert_eq!(put_resp.status(), StatusCode::OK);

        let user = state
            .users
            .lookup(&UserId::new("alice").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.display_name, "Alice Wonderland");
    }

    #[tokio::test]
    async fn put_self_rejects_unknown_key() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let cookie = seed_login(&state, "alice").await;
        let app = build_router(state, axum::Router::new());

        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from("key=banana&value=split&currentpassword=hunter2"))
            .unwrap();
        let put_resp = app.oneshot(put_req).await.unwrap();
        assert_eq!(put_resp.status(), StatusCode::BAD_REQUEST);
    }
}
