//! Integration tests for Batch F: LOCK + UNLOCK + lock-aware mutations.
//!
//! Covers the eight scenarios called out in the SP5 plan:
//!   1. `LOCK` returns `200` with a `Lock-Token` header.
//!   2. `LOCK` on an already-locked resource (no matching `If:`) → `423`.
//!   3. `UNLOCK` with the correct `Lock-Token` releases the lock.
//!   4. `UNLOCK` with a wrong token → `409`.
//!   5. `PUT` on a locked file with no `If:` header → `423`.
//!   6. `PUT` on a locked file with a matching `If:` header → `201/204`.
//!   7. A depth-infinity lock on a directory locks descendants.
//!   8. An expired lock can be reacquired.
//!
//! The tests drive `build_router` end-to-end so each request travels
//! through the real auth layer + DAV dispatcher.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::LockStore;
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // Mirror the Batch C/D/E pattern: disable the async scanner under SQLite
    // so PUT-then-LOCK / PUT-then-UNLOCK don't race with the populate path.
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
            "DAV",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn put_file(app: axum::Router, uri: &str, token: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

async fn lock_request(app: axum::Router, uri: &str, token: &str) -> axum::response::Response {
    let req = Request::builder()
        .method("LOCK")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

/// Pull the lock-token URN (without `<` `>`) out of the `Lock-Token`
/// response header. Panics if absent.
fn extract_lock_token(resp: &axum::response::Response) -> String {
    let raw = resp
        .headers()
        .get("lock-token")
        .expect("Lock-Token header present")
        .to_str()
        .unwrap()
        .trim()
        .to_string();
    raw.trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

#[tokio::test]
async fn lock_acquire_returns_token() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l1.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );

    let resp = lock_request(app, "/dav/files/alice/a.txt", &token).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let lt = resp
        .headers()
        .get("lock-token")
        .expect("Lock-Token header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        lt.starts_with("<urn:uuid:") && lt.ends_with('>'),
        "Lock-Token must be a bracketed urn:uuid: …, got {lt}"
    );
    let body = body_string(resp).await;
    assert!(
        body.contains("d:locktoken"),
        "body should echo lock token: {body}"
    );
}

#[tokio::test]
async fn lock_on_locked_resource_returns_423() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l2.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );
    let first = lock_request(app.clone(), "/dav/files/alice/a.txt", &token).await;
    assert_eq!(first.status(), StatusCode::OK);

    // Second LOCK without a matching If: token must 423.
    let second = lock_request(app, "/dav/files/alice/a.txt", &token).await;
    assert_eq!(second.status(), StatusCode::LOCKED);
}

#[tokio::test]
async fn unlock_with_correct_token_releases() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l3.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );
    let lock_resp = lock_request(app.clone(), "/dav/files/alice/a.txt", &token).await;
    let urn = extract_lock_token(&lock_resp);

    let req = Request::builder()
        .method("UNLOCK")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("lock-token", format!("<{}>", urn))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A fresh LOCK must succeed now.
    let again = lock_request(app, "/dav/files/alice/a.txt", &token).await;
    assert_eq!(again.status(), StatusCode::OK);
}

#[tokio::test]
async fn unlock_with_wrong_token_returns_409() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l4.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );
    let lock_resp = lock_request(app.clone(), "/dav/files/alice/a.txt", &token).await;
    assert_eq!(lock_resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("UNLOCK")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("lock-token", "<urn:uuid:wrong-token>")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn put_on_locked_without_if_returns_423() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l5.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );
    let lock_resp = lock_request(app.clone(), "/dav/files/alice/a.txt", &token).await;
    assert_eq!(lock_resp.status(), StatusCode::OK);

    // PUT without If: must be 423.
    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("y"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::LOCKED);
}

#[tokio::test]
async fn put_on_locked_with_if_succeeds() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l6.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );
    let lock_resp = lock_request(app.clone(), "/dav/files/alice/a.txt", &token).await;
    let urn = extract_lock_token(&lock_resp);

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("if", format!("(<{}>)", urn))
        .body(Body::from("y"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Overwriting an existing file is 204.
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn lock_infinity_depth_locks_children() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l7.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Create a directory and a child file.
    let req = Request::builder()
        .method("MKCOL")
        .uri("/dav/files/alice/dir")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/dir/child.txt", &token, "x").await,
        StatusCode::CREATED
    );

    // LOCK the directory with Depth: infinity.
    let req = Request::builder()
        .method("LOCK")
        .uri("/dav/files/alice/dir")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "infinity")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // PUT on the child without the parent's lock token → 423.
    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/dir/child.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("y"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::LOCKED);
}

#[tokio::test]
async fn expired_lock_can_be_reacquired() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l8.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let pool = state.filecache.pool().clone();
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/a.txt", &token, "x").await,
        StatusCode::CREATED
    );

    // Insert a stale lock directly via LockStore with a TTL in the past.
    // This sidesteps the Timeout: header's 1-second minimum-positive
    // floor — the goal is to verify expired rows are transparently
    // overwritten by a fresh acquire.
    let store = LockStore::new(pool);
    let past = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        - 10;
    store
        .acquire(
            "files/alice/a.txt",
            "urn:uuid:stale",
            "exclusive",
            "0",
            None,
            past,
        )
        .await
        .unwrap();

    // LOCK on the same key must succeed (the expired row is transparently
    // replaced — `LockStore::current` filters out expired rows).
    let resp = lock_request(app, "/dav/files/alice/a.txt", &token).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let lt = resp
        .headers()
        .get("lock-token")
        .expect("Lock-Token")
        .to_str()
        .unwrap();
    assert!(!lt.contains("stale"), "fresh token expected, got {lt}");
}
