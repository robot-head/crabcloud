//! HTTP-level integration tests for the Files server fns. Drives the full
//! `build_router` stack with the Dioxus server-fn router merged in, so
//! requests travel through the same auth / CSRF / session middleware as
//! production. Bearer auth (matching the WebDAV test pattern in
//! `crates/crabcloud-http/tests/dav_basic.rs`) is used to satisfy
//! `AuthLayer`; the server fns read the resulting `AuthContext`.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // Disable the filecache scanner. Matches the workaround used by the
    // dav_* test helpers to avoid a race between the scanner and follow-up
    // `View::stat` calls.
    cfg.filecache.enabled = false;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
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
    state
}

async fn alice_bearer(state: &AppState) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "UI",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

/// Build the axum app: register Dioxus server functions, then layer the
/// production middleware via `build_router`. `FullstackState::headless()`
/// skips the asset / index.html plumbing the CLI bundler would otherwise
/// install.
fn build_app(state: AppState) -> axum::Router {
    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    crabcloud_http::build_router(state, dioxus_router)
}

/// Write a small file via WebDAV PUT so we have content to list.
async fn put_file(app: &axum::Router, token: &str, path: &str, body: &'static str) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/dav/files/alice{path}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "PUT failed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn list_dir_returns_entries() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = build_app(state);

    put_file(&app, &token, "/hello.txt", "hi").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/list")
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "path": "/" }).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "got {}", resp.status());
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let entries: Vec<crabcloud_ui::FileEntry> = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "decode entries: {e} body={:?}",
            String::from_utf8_lossy(&body)
        )
    });
    assert!(
        entries.iter().any(|e| e.name == "hello.txt" && !e.is_dir),
        "expected hello.txt in {:?}",
        entries.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn list_dir_unauthenticated_returns_non_ok() {
    // `AuthLayer` only 401s when an auth header is present-but-invalid.
    // A request with no auth at all falls through anonymous; the server
    // fn body returns its own `ServerFnError::new("unauthorized")`, which
    // Dioxus maps to 500. Either way it's not 200 — that's the contract
    // tests assert here.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/list")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "path": "/" }).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn list_dir_invalid_path_returns_non_ok() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("i.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/list")
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "path": "not-absolute" }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}

/// POST a JSON body to a server-fn endpoint with bearer auth and the
/// OCS sentinel header (required by the CSRF / OCS middleware in
/// `build_router`). Mirrors the WebDAV `put_file` helper above.
async fn post_json(
    app: &axum::Router,
    token: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
async fn mkdir_creates_directory() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("mk.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = build_app(state);
    let resp = post_json(
        &app,
        &token,
        "/api/files/mkdir",
        serde_json::json!({ "path": "/newdir" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "got {}", resp.status());
}

#[tokio::test]
async fn rename_moves_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("rn.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = build_app(state);
    put_file(&app, &token, "/old.txt", "hi").await;
    let resp = post_json(
        &app,
        &token,
        "/api/files/rename",
        serde_json::json!({ "from": "/old.txt", "to": "/new.txt" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Verify via DAV GET that the file moved.
    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/new.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.clone().oneshot(get).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_removes_files() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("dl.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = build_app(state);
    put_file(&app, &token, "/a.txt", "a").await;
    put_file(&app, &token, "/b.txt", "b").await;
    let resp = post_json(
        &app,
        &token,
        "/api/files/delete",
        serde_json::json!({ "paths": ["/a.txt", "/b.txt"] }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(get).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
