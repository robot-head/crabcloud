//! Integration tests for Batch G: chunked upload routes.
//!
//! Covers:
//!   1. MKCOL → PUT × 2 → MOVE happy path (`chunked_upload_begin_put_commit_flow`).
//!   2. PUT against an unknown upload_id returns 404
//!      (`chunked_upload_unknown_id_returns_404_on_put`).
//!   3. DELETE on an unknown upload_id is idempotent (returns 204)
//!      (`chunked_upload_abort_returns_204`).

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use support::{bearer, make_state, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

/// Build state + seed `alice`. Thin wrapper over the shared `make_state` +
/// `seed_user` so the existing call sites stay one-liners.
async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    state
}

/// Mint a Bearer token for `alice` against the live token store.
async fn alice_bearer(state: &AppState) -> String {
    bearer(state, "alice").await
}

#[tokio::test]
async fn chunked_upload_begin_put_commit_flow() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let upload_id = "client-upload-1";

    // 1. MKCOL: begin upload, Destination points at the final file.
    let begin = Request::builder()
        .method("MKCOL")
        .uri(format!("/dav/uploads/alice/{upload_id}"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/big.bin")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(begin).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 2. PUT part 1.
    let p1 = Request::builder()
        .method("PUT")
        .uri(format!("/dav/uploads/alice/{upload_id}/1"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("AAAA"))
        .unwrap();
    let resp = app.clone().oneshot(p1).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag1 = resp
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 3. PUT part 2.
    let p2 = Request::builder()
        .method("PUT")
        .uri(format!("/dav/uploads/alice/{upload_id}/2"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("BBBB"))
        .unwrap();
    let resp = app.clone().oneshot(p2).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag2 = resp
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 4. MOVE /uploads/{user}/{id}/.file → /files/{user}/big.bin
    //    with the per-part tags as JSON.
    let part_tags =
        format!(r#"[{{"part_number":1,"etag":"{etag1}"}},{{"part_number":2,"etag":"{etag2}"}}]"#);
    let commit = Request::builder()
        .method("MOVE")
        .uri(format!("/dav/uploads/alice/{upload_id}/.file"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/big.bin")
        .header("x-crabcloud-part-tags", &part_tags)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(commit).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let final_etag = resp.headers().get("etag").unwrap().to_str().unwrap();
    assert!(final_etag.starts_with('"') && final_etag.ends_with('"'));

    // 5. GET the assembled file: parts joined.
    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/big.bin")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(get).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"AAAABBBB");
}

#[tokio::test]
async fn chunked_upload_unknown_id_returns_404_on_put() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/uploads/alice/no-such-upload/1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("ignored"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn chunked_upload_abort_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // DELETE without a prior MKCOL is idempotent and returns 204.
    let req = Request::builder()
        .method("DELETE")
        .uri("/dav/uploads/alice/never-started")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // After MKCOL, DELETE returns 204 and the upload id can no longer be
    // PUT to.
    let begin = Request::builder()
        .method("MKCOL")
        .uri("/dav/uploads/alice/abort-me")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/aborted.bin")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(begin).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let del = Request::builder()
        .method("DELETE")
        .uri("/dav/uploads/alice/abort-me")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(del).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Subsequent PUT against the aborted id should be 404.
    let put = Request::builder()
        .method("PUT")
        .uri("/dav/uploads/alice/abort-me/1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("x"))
        .unwrap();
    let resp = app.oneshot(put).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
