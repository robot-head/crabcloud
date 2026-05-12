//! `GET /ocs/v2.php/cloud/user` and `PUT /ocs/v2.php/cloud/user` — self-only.
//!
//! Self-service endpoints for the authenticated user. `GET` returns the user's
//! own record (id, display-name, email, groups, enabled, last-login). `PUT`
//! accepts a `key`/`value`/`currentpassword` form body and applies a single
//! mutation to the authenticated user (password, displayname, or email).
//! Password changes verify `currentpassword` and revoke every OTHER session
//! belonging to the same uid (keeping the caller's current session alive).
//! Password changes are only allowed from `AuthMethod::Session`; Bearer/Basic
//! callers receive 403.

use crate::auth_context::AuthMethod;
use crate::extractors::auth::AuthenticatedUser;
use crate::extractors::format::OcsFormat;
use crate::session::{PendingCookie, SessionHandle};
use crate::AuthContext;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::{AuthTokenType, UserId};
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
    Extension(ctx): Extension<AuthContext>,
    Extension(handle): Extension<SessionHandle>,
    fmt: OcsFormat,
    Form(form): Form<PutForm>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&authed.user_id).map_err(|e| users_err(e, fmt.0))?;

    // Pre-flight: password rotation is Session-only. Reject Bearer/Basic
    // here BEFORE verifying currentpassword so a stolen Bearer token can't
    // probe whether a guess matches the user's primary password (the 403
    // would otherwise differ from the 401 that a wrong `currentpassword`
    // produces, leaking the comparison result).
    if form.key == "password" && ctx.method != AuthMethod::Session {
        return Err(OcsError::new(CoreError::Forbidden, OcsVersion::V2, fmt.0));
    }

    state
        .users
        .verify(uid.as_str(), &form.currentpassword)
        .await
        .map_err(|_| unauth(fmt.0))?;

    match form.key.as_str() {
        "password" => {
            // Gate already enforced above; ctx.method == Session is guaranteed here.
            // `set_password` cascades `invalidate_all_for_user` which marks
            // every row for `uid` (including the caller's) as
            // `password_invalid=true`. To keep the caller logged in we mint
            // a fresh session token for them AFTER the cascade, delete every
            // other session, and swap the cookie.
            state
                .users
                .set_password(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            if let Some(ap) = state.users.app_passwords() {
                let (new_row, raw) = ap
                    .mint(
                        &uid,
                        &ctx.login_name,
                        "Password-rotation",
                        AuthTokenType::Session,
                        ctx.remember,
                    )
                    .await
                    .map_err(|e| users_err(e, fmt.0))?;
                // Revoke everything except the freshly-minted row.
                if let Err(err) = ap.revoke_other_sessions(&uid, new_row.id).await {
                    ::tracing::warn!(
                        error = ?err,
                        uid = %uid,
                        "revoke_other_sessions failed after password change"
                    );
                }
                // Rotate CSRF + bind the blob to the new token id via the
                // pending cookie so the SessionLayer saves the rotated csrf
                // under the freshly-minted row.id (not the old, now-revoked
                // token id).
                handle.mutate(|s| s.rotate_csrf()).await;
                handle
                    .set_pending_cookie(PendingCookie::Set {
                        raw_token: raw.expose().to_string(),
                        token_id: new_row.id,
                        max_age_secs: 30 * 60,
                    })
                    .await;
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
    use crate::session::{encode_cookie, COOKIE_NAME};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::{AppState, AppStateBuilder};
    use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
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

    /// Mint a real session-kind AuthToken via the AppPasswordService and
    /// return the `Cookie:` header value the SessionLayer will accept.
    async fn seed_login(state: &AppState, uid: &str) -> String {
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new(uid).unwrap(),
                uid,
                "test-session",
                AuthTokenType::Session,
                false,
            )
            .await
            .unwrap();
        let cookie_value =
            encode_cookie(raw.expose(), state.config.secret.expose_secret().as_bytes());
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
        // The response rotates the cookie — we capture it for the follow-up
        // request below. `set_password`'s cascade marks every row including
        // session A as `password_invalid`, so the handler mints a fresh
        // session and swaps the cookie.
        let new_cookie = put_resp
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .expect("password change rotates cookie")
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        // Session B should be gone — GET /user with that cookie returns 401.
        let req_post = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie_b)
            .body(Body::empty())
            .unwrap();
        let resp_post = app.clone().oneshot(req_post).await.unwrap();
        assert_eq!(resp_post.status(), StatusCode::UNAUTHORIZED);

        // The rotated cookie keeps session A authenticated.
        let req_self = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", new_cookie)
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

    #[tokio::test]
    async fn put_self_password_change_via_bearer_is_403() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        let app = build_router(state, axum::Router::new());

        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", format!("Bearer {}", raw.expose()))
            .body(Body::from(
                "key=password&value=newpass&currentpassword=hunter2",
            ))
            .unwrap();
        let resp = app.oneshot(put_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn put_self_password_change_via_bearer_is_403_even_with_wrong_currentpassword() {
        // Regression: the 403 vs 401 distinction would leak password equality
        // through a Bearer-authenticated probe. Both cases must return 403.
        use crabcloud_users::AuthTokenType;
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("user.db")).await;
        seed_user(&state, "alice", "hunter2").await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        let app = build_router(state, axum::Router::new());

        let put_req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", format!("Bearer {}", raw.expose()))
            .body(Body::from(
                "key=password&value=newpass&currentpassword=WRONG",
            ))
            .unwrap();
        let resp = app.oneshot(put_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
