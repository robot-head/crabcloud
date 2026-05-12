//! Integration tests for Batch B: OPTIONS, GET/HEAD/PUT/MKCOL/DELETE,
//! conditional headers (If-Match, If-None-Match), single Range support, and
//! the `/remote.php/dav` legacy alias. Drives the full `build_router` so each
//! request travels through the real auth + middleware stack.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // Disable the filecache scanner: under `cargo test --workspace` on Linux
    // CI the scanner's async event-apply races our handler's follow-up
    // `view.stat` calls, occasionally serving a stale "not yet populated"
    // miss back through `View::stat` -> 201 instead of 204 on overwrite PUT.
    // Same workaround Batches C–F apply in their test helpers.
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

/// Mint a Bearer token for `alice` against the live token store. Returns the
/// raw token string; callers wrap it in an `Authorization: Bearer …` header.
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

#[tokio::test]
async fn options_returns_dav_class_and_allow() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/alice")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("dav").unwrap().to_str().unwrap(),
        "1, 2, 3"
    );
    let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("PROPFIND"));
    assert!(allow.contains("LOCK"));
}

#[tokio::test]
async fn put_creates_file_returns_201_etag() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/hello.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("hello world"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = resp.headers().get("etag").unwrap().to_str().unwrap();
    assert!(etag.starts_with('"') && etag.ends_with('"') && etag.len() == 42);
}

#[tokio::test]
async fn put_overwrite_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("po.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/over.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("v1"))
        .unwrap();
    let resp1 = app.clone().oneshot(r1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::CREATED);

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/over.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("v2-longer"))
        .unwrap();
    let resp2 = app.oneshot(r2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn put_with_if_none_match_star_on_existing_returns_412() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/x.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("v1"))
        .unwrap();
    let resp1 = app.clone().oneshot(r1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::CREATED);

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/x.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("if-none-match", "*")
        .body(Body::from("v2"))
        .unwrap();
    let resp2 = app.oneshot(r2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn put_with_if_match_mismatch_returns_412() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/y.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("v1"))
        .unwrap();
    app.clone().oneshot(r1).await.unwrap();

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/y.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("if-match", "\"wrong-etag\"")
        .body(Body::from("v2"))
        .unwrap();
    let resp = app.oneshot(r2).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn put_with_if_match_match_succeeds() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/z.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("v1"))
        .unwrap();
    let resp1 = app.clone().oneshot(r1).await.unwrap();
    let etag = resp1
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let r2 = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/z.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("if-match", &etag)
        .body(Body::from("v2"))
        .unwrap();
    let resp = app.oneshot(r2).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn get_returns_file_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("g.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/hi.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("hello"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/hi.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("accept-ranges")
            .unwrap()
            .to_str()
            .unwrap(),
        "bytes"
    );
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"hello");
}

#[tokio::test]
async fn head_returns_metadata_no_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("h.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/hd.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("hello"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let head = Request::builder()
        .method("HEAD")
        .uri("/dav/files/alice/hd.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(head).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        "5"
    );
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn get_with_range_returns_206() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("r.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/big.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("0123456789"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/big.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("range", "bytes=2-5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let cr = resp
        .headers()
        .get("content-range")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(cr, "bytes 2-5/10");
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"2345");
}

#[tokio::test]
async fn get_with_invalid_range_returns_416() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ri.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/small.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("hi"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/small.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("range", "bytes=100-200")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn mkcol_creates_directory() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("MKCOL")
        .uri("/dav/files/alice/newdir")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn delete_removes_file_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("d.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/to-delete.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("bye"))
        .unwrap();
    app.clone().oneshot(put).await.unwrap();

    let del = Request::builder()
        .method("DELETE")
        .uri("/dav/files/alice/to-delete.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(del).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn legacy_remote_php_dav_alias_works() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/remote.php/dav/files/alice")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("dav").unwrap().to_str().unwrap(),
        "1, 2, 3"
    );
}

#[tokio::test]
async fn unauthenticated_dav_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/alice")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cross_user_access_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/files/bob")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
