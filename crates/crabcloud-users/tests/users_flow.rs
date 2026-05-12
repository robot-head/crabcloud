//! End-to-end flow against the OCS surface of the HTTP router exercising
//! spec acceptance criteria for the users sub-project.
//!
//! Login itself moved to a Dioxus `#[server]` function in `crabcloud-ui` as
//! part of the fullstack migration, so it's not reachable from `build_router`
//! alone; verification is covered by the Playwright suite. These tests mint
//! a real session-kind `AuthToken` directly to focus on the `/cloud/user`
//! semantics.

#![allow(unused_crate_dependencies)]
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use crabcloud_http::session::{encode_cookie, COOKIE_NAME};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use secrecy::ExposeSecret;
use tempfile::tempdir;
use tower::ServiceExt;

async fn build_app() -> (axum::Router, String) {
    let dir = tempdir().unwrap();
    let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("flow.db"));
    let state = AppStateBuilder::new(cfg)
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    std::mem::forget(dir);

    // Mint a real session-kind AuthToken for alice and sign the raw token
    // into a cookie value.
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "test",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    let cookie_value = encode_cookie(raw.expose(), state.config.secret.expose_secret().as_bytes());
    let cookie = format!("{COOKIE_NAME}={cookie_value}");

    // Mount Dioxus `#[server]` functions only (no SSR / no static assets):
    // tests don't need rendered HTML or a built `public/` dir, but they do
    // need `/index.php/login`, `/index.php/login/v2`, etc. to resolve.
    use dioxus::server::{DioxusRouterExt, FullstackState};
    let app_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    // Silence the `crabcloud-ui` dev-dep unused-crate-dependencies warning —
    // the dep is pulled in so `cargo test` triggers compilation of the
    // `#[server]` declarations that register themselves via inventory.
    let _ = crabcloud_ui::App;
    (build_router(state, app_router), cookie)
}

#[tokio::test]
async fn login_v2_start_returns_urls() {
    let (app, _cookie) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed["poll"]["token"].as_str().unwrap().len() > 16);
    assert!(parsed["login"]
        .as_str()
        .unwrap()
        .contains("/login/v2/flow/"));
}

#[tokio::test]
async fn login_v2_full_cycle() {
    let (app, _cookie) = build_app().await;

    // 1. Start.
    let start_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2")
        .body(Body::empty())
        .unwrap();
    let start_resp = app.clone().oneshot(start_req).await.unwrap();
    assert_eq!(start_resp.status(), StatusCode::OK);
    let start_body = to_bytes(start_resp.into_body(), 16 * 1024).await.unwrap();
    let start: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
    let poll_token = start["poll"]["token"].as_str().unwrap().to_string();
    let login_url = start["login"].as_str().unwrap().to_string();
    let flow_id = login_url.rsplit('/').next().unwrap().to_string();

    // 2. Pre-authorize poll → non-200.
    let poll_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/poll")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            "{{\"req\":{{\"token\":\"{poll_token}\"}}}}"
        )))
        .unwrap();
    let pre = app.clone().oneshot(poll_req).await.unwrap();
    assert_ne!(pre.status(), StatusCode::OK);

    // 3. Login via /index.php/login to get a cookie.
    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from(
            "{\"username\":\"alice\",\"password\":\"hunter2\"}",
        ))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // 4. Authorize as cookie-authed user. `ocs-apirequest: true` bypasses
    // the CSRF check (matching Nextcloud convention) — real browser callers
    // include the `requesttoken` meta value instead, which would require
    // reading it back out of the SessionStore for tests.
    let auth_req = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/authorize")
        .header("content-type", "application/json")
        .header("cookie", &cookie)
        .header("ocs-apirequest", "true")
        .body(Body::from(format!("{{\"flow_id\":\"{flow_id}\"}}")))
        .unwrap();
    let auth_resp = app.clone().oneshot(auth_req).await.unwrap();
    assert_eq!(auth_resp.status(), StatusCode::OK);

    // 5. Poll → 200 with the app password.
    let poll_req2 = Request::builder()
        .method("POST")
        .uri("/index.php/login/v2/poll")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            "{{\"req\":{{\"token\":\"{poll_token}\"}}}}"
        )))
        .unwrap();
    let poll2 = app.oneshot(poll_req2).await.unwrap();
    assert_eq!(poll2.status(), StatusCode::OK);
    let body = to_bytes(poll2.into_body(), 16 * 1024).await.unwrap();
    let p: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(p["loginName"], "alice");
    assert!(p["appPassword"].as_str().unwrap().len() > 50);
}

#[tokio::test]
async fn getapppassword_via_cookie_mints_bridge_token() {
    let (app, _cookie) = build_app().await;

    // Login via /index.php/login to get a freshly-minted Session cookie
    // (so AuthContext.method is Session and the OCS endpoint admits us).
    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from(
            "{\"username\":\"alice\",\"password\":\"hunter2\"}",
        ))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let req = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let p: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(p["ocs"]["data"]["apppassword"].as_str().unwrap().len() > 50);
}

#[tokio::test]
async fn delete_app_password_revokes_current_token() {
    let (app, _cookie) = build_app().await;

    // Login → cookie.
    let login_req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/json")
        .body(Body::from(
            "{\"username\":\"alice\",\"password\":\"hunter2\"}",
        ))
        .unwrap();
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Mint bridge token via getapppassword.
    let gap = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let gap_resp = app.clone().oneshot(gap).await.unwrap();
    let gap_body = to_bytes(gap_resp.into_body(), 16 * 1024).await.unwrap();
    let raw_token = serde_json::from_slice::<serde_json::Value>(&gap_body).unwrap()["ocs"]["data"]
        ["apppassword"]
        .as_str()
        .unwrap()
        .to_string();

    // Token works via Bearer.
    let me = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {raw_token}"))
        .body(Body::empty())
        .unwrap();
    let me_resp = app.clone().oneshot(me).await.unwrap();
    assert_eq!(me_resp.status(), StatusCode::OK);

    // Self-revoke via DELETE apppassword (using the token itself).
    let del = Request::builder()
        .method("DELETE")
        .uri("/ocs/v2.php/core/apppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {raw_token}"))
        .body(Body::empty())
        .unwrap();
    let del_resp = app.clone().oneshot(del).await.unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);

    // Reuse → 401.
    let again = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {raw_token}"))
        .body(Body::empty())
        .unwrap();
    let again_resp = app.oneshot(again).await.unwrap();
    assert_eq!(again_resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn getapppassword_via_bearer_is_forbidden() {
    // Bypass /login: build an AppState directly and mint an AppPassword Bearer.
    let dir = tempdir().unwrap();
    let cfg = crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("ap.db"));
    let state = AppStateBuilder::new(cfg)
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
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

    let req = Request::builder()
        .uri("/ocs/v2.php/core/getapppassword?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {}", raw.expose()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let _ = dir;
}

#[tokio::test]
async fn get_self_returns_payload() {
    let (app, cookie) = build_app().await;
    let req = Request::builder()
        .method("GET")
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["data"]["id"], "alice");
    assert_eq!(parsed["ocs"]["data"]["display-name"], "Alice");
    assert_eq!(parsed["ocs"]["data"]["enabled"], true);
}
