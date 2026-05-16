//! End-to-end tests for the anonymous public-link WebDAV surface mounted
//! under `/public.php/dav/files/{token}/...`. Drives the full
//! `build_router` so each request travels through the nested
//! `public_link_auth(AuthSurface::Dav)` middleware (HTTP Basic against
//! the link's bcrypt hash) before reaching the surface-neutral DAV
//! handlers.
//!
//! Fixture mirrors `dav_basic.rs` (filecache scanner disabled) plus the
//! seed/create-link helpers from `public_link_e2e.rs`: seed an owner home
//! on disk, materialise a small subtree, create the link directly via
//! `Shares::create` (sidesteps the OCS handler), then drive the public
//! DAV URLs anonymously.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use support::{create_link, make_state, seed_file, seed_folder, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

/// Helper: build a `Basic` header value from a password. The link auth
/// layer ignores the username, so `anonymous:` is conventional but any
/// string works.
fn basic_auth(password: &str) -> String {
    let raw = format!("anonymous:{password}");
    format!("Basic {}", B64.encode(raw.as_bytes()))
}

// --- PROPFIND --------------------------------------------------------------

#[tokio::test]
async fn propfind_read_link_returns_multistatus() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pf.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Photos").await;
    seed_file(&state, "alice", "Photos/cat.jpg", b"meow").await;
    seed_file(&state, "alice", "Photos/dog.jpg", b"woof").await;
    let token = create_link(&state, "alice", "/Photos", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/public.php/dav/files/{token}/"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let s = String::from_utf8_lossy(&body);
    assert!(s.contains("cat.jpg"), "body should list cat.jpg: {s}");
    assert!(s.contains("dog.jpg"), "body should list dog.jpg: {s}");
    // The href prefix must be the public surface, not the authed one.
    assert!(
        s.contains(&format!("/public.php/dav/files/{token}")),
        "hrefs should be token-rooted: {s}"
    );
}

#[tokio::test]
async fn propfind_create_only_link_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pfco.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    seed_file(&state, "alice", "Drop/secret.txt", b"NOPE").await;
    // File-drop: create-only (bit 4), no read.
    let token = create_link(&state, "alice", "/Drop", 4, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // PROPFIND on a child path inside a file-drop link must be denied —
    // the storage wrapper hides non-root entries from listing/stat.
    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/public.php/dav/files/{token}/secret.txt"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- GET -------------------------------------------------------------------

#[tokio::test]
async fn get_read_link_returns_file_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("g.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Photos").await;
    seed_file(&state, "alice", "Photos/photo.jpg", b"jpeg-bytes").await;
    let token = create_link(&state, "alice", "/Photos", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/public.php/dav/files/{token}/photo.jpg"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"jpeg-bytes");
}

#[tokio::test]
async fn get_create_only_link_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("gco.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    seed_file(&state, "alice", "Drop/secret.txt", b"NOPE").await;
    let token = create_link(&state, "alice", "/Drop", 4, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/public.php/dav/files/{token}/secret.txt"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- PUT -------------------------------------------------------------------

#[tokio::test]
async fn put_create_only_link_with_correct_basic_writes_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pp.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 5, Some("hunter2"), None).await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/public.php/dav/files/{token}/uploaded.txt"))
        .header("authorization", basic_auth("hunter2"))
        .body(Body::from("dropped"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // File should land under owner's home at Drop/uploaded.txt.
    let storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let sp = StoragePath::new("Drop/uploaded.txt").unwrap();
    assert!(storage.exists(&sp).await.unwrap(), "file written to owner");
}

#[tokio::test]
async fn put_create_only_link_with_wrong_basic_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pwrong.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 5, Some("hunter2"), None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/public.php/dav/files/{token}/upload.txt"))
        .header("authorization", basic_auth("nope"))
        .body(Body::from("payload"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let challenge = resp
        .headers()
        .get("www-authenticate")
        .expect("Basic challenge present")
        .to_str()
        .unwrap();
    assert!(
        challenge.contains("Basic") && challenge.contains("public-link"),
        "expected Basic realm=\"public-link\" challenge, got: {challenge}"
    );
}

#[tokio::test]
async fn put_create_only_link_with_no_basic_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pnone.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 5, Some("hunter2"), None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/public.php/dav/files/{token}/upload.txt"))
        .body(Body::from("payload"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let challenge = resp
        .headers()
        .get("www-authenticate")
        .expect("Basic challenge present")
        .to_str()
        .unwrap();
    assert!(
        challenge.contains("Basic") && challenge.contains("public-link"),
        "expected Basic realm=\"public-link\" challenge, got: {challenge}"
    );
}

// --- DELETE ----------------------------------------------------------------

#[tokio::test]
async fn delete_read_link_returns_403() {
    // Read-only link's permission mask doesn't carry the delete bit, so
    // the storage-wrapper layer rejects the delete with a 403 — there's
    // no separate denial logic in the handler.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("dr.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Photos").await;
    seed_file(&state, "alice", "Photos/photo.jpg", b"jpeg").await;
    let token = create_link(&state, "alice", "/Photos", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/public.php/dav/files/{token}/photo.jpg"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- expiration / unknown token --------------------------------------------

#[tokio::test]
async fn propfind_expired_token_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("exp.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Old").await;
    let past = chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    let token = create_link(&state, "alice", "/Old", 1, None, Some(past)).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Expired links are indistinguishable from missing — the auth layer
    // returns 404 before the handler runs.
    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/public.php/dav/files/{token}/"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
