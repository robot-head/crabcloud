//! Integration tests for AuthLayer arms (Bearer, Basic, anon → 401).
//! Drives the full `build_router` to exercise layer interactions.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::STANDARD as B64STD;
use base64::Engine;
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_http::build_router;
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state(path: std::path::PathBuf) -> AppState {
    AppStateBuilder::new(minimal_sqlite_config(path))
        .build()
        .await
        .unwrap()
}

async fn seed_user(state: &AppState, uid: &str) {
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
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
}

#[tokio::test]
async fn bearer_with_minted_token_authenticates() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
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
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Bearer {}", raw.expose()))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn bearer_with_unknown_token_returns_401() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let app = build_router(state, axum::Router::new());
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", "Bearer not-a-real-token")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn basic_with_minted_token_authenticates() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
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
    let creds = B64STD.encode(format!("alice:{}", raw.expose()));
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Basic {creds}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn basic_uid_mismatch_returns_401() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
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
    let creds = B64STD.encode(format!("bob:{}", raw.expose()));
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .header("authorization", format!("Basic {creds}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn anonymous_request_is_unauthorized_on_protected_route() {
    let dir = tempdir().unwrap();
    let state = make_state(dir.path().join("auth.db")).await;
    seed_user(&state, "alice").await;
    let app = build_router(state, axum::Router::new());
    let req = Request::builder()
        .uri("/ocs/v2.php/cloud/user?format=json")
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
