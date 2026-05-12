//! End-to-end flow against the OCS surface of the HTTP router exercising
//! spec acceptance criteria for the users sub-project.
//!
//! Login itself moved to a Dioxus `#[server]` function in `crabcloud-ui` as
//! part of the fullstack migration, so it's not reachable from `build_router`
//! alone; verification is covered by the Playwright suite. These tests seed
//! the session directly via `SessionStore` to focus on the `/cloud/user`
//! semantics.

#![allow(unused_crate_dependencies)]
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use crabcloud_http::session::{encode_cookie, Session, SessionId, SessionStore, COOKIE_NAME};
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
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

    // Seed an authenticated session for alice.
    let store = SessionStore::new(state.cache.clone(), &state.config.instanceid);
    let id = SessionId::new_random();
    let mut session = Session::new();
    session.user_id = Some("alice".to_string());
    session.rotate_csrf();
    session.two_factor_passed = true;
    store.save(&id, &session).await.unwrap();
    store.record_for_user("alice", &id).await.unwrap();
    let cookie_value = encode_cookie(id.as_str(), state.config.secret.expose_secret().as_bytes());
    let cookie = format!("{COOKIE_NAME}={cookie_value}");

    (build_router(state, axum::Router::new()), cookie)
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
