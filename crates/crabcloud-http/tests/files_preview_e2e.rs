//! End-to-end tests for the authed preview endpoint
//! (`GET /api/files/preview/{fileid}`). Drives the full `build_router`
//! so each request runs through the real `AuthLayer` and our handler
//! picks up the authenticated user via the `AuthenticatedUser`
//! extractor.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use support::{bearer, make_state, seed_file_with_mime, seed_jpeg, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 16 * 1024 * 1024;

#[tokio::test]
async fn preview_returns_jpeg_for_image_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("ok.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, "alice", "/cat.jpg", 800, 600).await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    let body = to_bytes(resp.into_body(), BODY_LIMIT).await.unwrap();
    let img = image::load_from_memory(&body).expect("decode preview jpeg");
    assert!(
        img.width() <= 64 && img.height() <= 64,
        "thumbnail {} x {} should fit within 64",
        img.width(),
        img.height()
    );
    assert!(
        img.width() == 64 || img.height() == 64,
        "longest edge should hit the requested 64 (got {} x {})",
        img.width(),
        img.height()
    );
}

#[tokio::test]
async fn preview_unsupported_mime_returns_415() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("u.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    // A minimal valid zip header so the storage backend sniffs the mime
    // as `application/zip` rather than a fallback.
    let zip_bytes = b"PK\x03\x04junk";
    let row = seed_file_with_mime(
        &state,
        "alice",
        "/archive.zip",
        zip_bytes,
        "application/zip",
    )
    .await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn preview_unknown_fileid_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("n.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/preview/999999?size=64")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_cross_user_fileid_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("x.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    let alice_row = seed_jpeg(&state, "alice", "/cat.jpg", 200, 200).await;
    let bob_token = bearer(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", alice_row.fileid))
                .header("authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 404 (not 403): the endpoint must not be usable as a fileid-oracle.
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_size_too_large_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("big.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, "alice", "/cat.jpg", 100, 100).await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=2048", row.fileid))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_no_auth_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("noauth.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/preview/1?size=64")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn preview_etag_revalidation_returns_304() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("etag.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, "alice", "/cat.jpg", 200, 200).await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let etag = r1
        .headers()
        .get(header::ETAG)
        .expect("first response carries an ETag")
        .to_str()
        .unwrap()
        .to_string();

    let r2 = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("authorization", format!("Bearer {token}"))
                .header("if-none-match", &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::NOT_MODIFIED);
}
