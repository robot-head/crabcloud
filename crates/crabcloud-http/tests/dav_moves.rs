//! Integration tests for batch C: MOVE + COPY + Destination + Overwrite.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

// Test helpers duplicated from `dav_basic.rs`. The plan acknowledges this
// duplication; lifting to `tests/support/` is a follow-up.

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // Disable the scanner — its async `cache.apply` races our handler's
    // populate path, producing SQLite "database is locked" errors in CI.
    // The handler's populate path is enough to keep the cache consistent
    // within a single request lifetime.
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

/// Mint a Bearer token for `alice` against the live token store.
async fn alice_bearer(state: &AppState) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

async fn seed(app: &axum::Router, token: &str, path: &str, body: &[u8]) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/dav/files/alice/{path}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let rbody = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert!(
        status.is_success(),
        "seed put {path} failed: {status} body: {}",
        String::from_utf8_lossy(&rbody)
    );
}

#[tokio::test]
async fn move_renames_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "from.txt", b"data").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/from.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/to.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Source gone.
    let src = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/from.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(src).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // Dest present with the body.
    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/to.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"data");
}

#[tokio::test]
async fn move_overwrite_f_blocks_when_dest_exists() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "a.txt", b"A").await;
    seed(&app, &token, "b.txt", b"B").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/b.txt")
        .header("overwrite", "F")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::PRECONDITION_FAILED,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn move_overwrite_t_replaces_dest_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "a.txt", b"AAA").await;
    seed(&app, &token, "b.txt", b"old").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/b.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "body: {}",
        String::from_utf8_lossy(&body)
    );

    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/b.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"AAA");
}

#[tokio::test]
async fn copy_duplicates_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "src.txt", b"copy-me").await;

    let req = Request::builder()
        .method("COPY")
        .uri("/dav/files/alice/src.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/dst.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "body: {}",
        String::from_utf8_lossy(&body)
    );

    for path in ["src.txt", "dst.txt"] {
        let r = Request::builder()
            .method("GET")
            .uri(format!("/dav/files/alice/{path}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let b = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&b[..], b"copy-me");
    }
}

#[tokio::test]
async fn move_to_other_user_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "x.txt", b"X").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/x.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/bob/x.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
