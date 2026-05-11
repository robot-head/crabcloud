//! End-to-end flow against `build_router(state)` exercising spec acceptance criteria.

#![allow(unused_crate_dependencies)]
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn build_app() -> axum::Router {
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
    build_router(state)
}

#[tokio::test]
async fn login_with_real_user_succeeds() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=hunter2"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn login_with_wrong_password_401() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=WRONG"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_with_unknown_user_401() {
    let app = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=nobody&password=anything"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_self_returns_payload() {
    let app = build_app().await;
    let req1 = Request::builder()
        .method("POST")
        .uri("/index.php/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=alice&password=hunter2"))
        .unwrap();
    let r1 = app.clone().oneshot(req1).await.unwrap();
    let cookie = r1
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let req2 = Request::builder()
        .method("GET")
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let r2 = app.oneshot(req2).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let body = to_bytes(r2.into_body(), 16 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["data"]["id"], "alice");
    assert_eq!(parsed["ocs"]["data"]["display-name"], "Alice");
    assert_eq!(parsed["ocs"]["data"]["enabled"], true);
}
