//! End-to-end tests for the anonymous public-link surface mounted under
//! `/s/{token}/...`. Drives the full `build_router` so each request travels
//! through the path-conditional `public_link_auth` middleware before
//! reaching the unlock / download / upload handlers. The fixture seeds an
//! owner home on disk, materializes a small subtree, creates the link via
//! the OCS endpoint (which exercises Batch B's `Shares::create_link`), and
//! then drives the public-link URLs anonymously.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use support::{create_link, make_state, seed_file, seed_folder, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

// --- download / read --------------------------------------------------------

#[tokio::test]
async fn download_read_link_returns_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("d.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Photos").await;
    seed_file(&state, "alice", "Photos/cat.jpg", b"meow-meow").await;
    let token = create_link(&state, "alice", "/Photos", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/download/cat.jpg"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"meow-meow");
}

#[tokio::test]
async fn download_create_only_link_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("co.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    seed_file(&state, "alice", "Drop/secret.txt", b"NOPE").await;
    // File-drop: create-only (bit 4), no read.
    let token = create_link(&state, "alice", "/Drop", 4, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/download/secret.txt"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn download_with_range_returns_partial_content() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("rng.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Range").await;
    seed_file(&state, "alice", "Range/blob.bin", b"0123456789").await;
    let token = create_link(&state, "alice", "/Range", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/download/blob.bin"))
        .header("range", "bytes=2-5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"2345");
}

// --- unlock / password gate ------------------------------------------------

#[tokio::test]
async fn unlock_wrong_password_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("u.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Secret").await;
    let token = create_link(&state, "alice", "/Secret", 1, Some("correct-horse"), None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/unlock"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("password=battery-staple"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unlock_correct_password_sets_cookie_and_redirects() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("uok.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Secret").await;
    let token = create_link(&state, "alice", "/Secret", 1, Some("hunter2"), None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/unlock"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("password=hunter2"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = resp
        .headers()
        .get("set-cookie")
        .expect("set-cookie present")
        .to_str()
        .unwrap();
    assert!(
        cookie.starts_with(&format!("pl_{token}=")),
        "cookie: {cookie}"
    );
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    let loc = resp
        .headers()
        .get("location")
        .expect("location")
        .to_str()
        .unwrap();
    assert_eq!(loc, format!("/s/{token}"));
}

#[tokio::test]
async fn download_before_unlock_returns_403() {
    // A password-gated link should refuse downloads even when the URL is
    // otherwise valid — the auth context flags `password_gate_required` and
    // the download handler bails before reaching storage.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pre.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Vault").await;
    seed_file(&state, "alice", "Vault/note.txt", b"top secret").await;
    let token = create_link(&state, "alice", "/Vault", 1, Some("hunter2"), None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/s/{token}/download/note.txt"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- upload (file-drop) ----------------------------------------------------

#[tokio::test]
async fn upload_create_link_writes_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("up.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    // Use perms=5 (read + create) — file-drop pure (perms=4) hides listing
    // but upload still works either way. We pick mixed mode here so the
    // collision-suffix test (below) can stat the resulting file via the
    // same link.
    let token = create_link(&state, "alice", "/Drop", 5, None, None).await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/upload/hello.txt"))
        .body(Body::from("dropped"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Confirm via the storage layer directly.
    let storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let sp = StoragePath::new("Drop/hello.txt").unwrap();
    assert!(
        storage.exists(&sp).await.unwrap(),
        "file written under Drop/"
    );
}

#[tokio::test]
async fn upload_unsafe_filename_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("uns.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 4, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // `..` prefix is rejected before any storage interaction.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/upload/..etc..passwd"))
        .body(Body::from("malicious"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_read_only_link_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("ro.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "RO").await;
    // Read-only link: bit 1, no create.
    let token = create_link(&state, "alice", "/RO", 1, None, None).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/upload/hi.txt"))
        .body(Body::from("nope"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn upload_collision_appends_suffix() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("col.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "C").await;
    // Pre-seed an existing file so the next upload must take a suffix.
    seed_file(&state, "alice", "C/file.txt", b"v1").await;
    let token = create_link(&state, "alice", "/C", 5, None, None).await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/upload/file.txt"))
        .body(Body::from("v2"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let final_name = v["name"].as_str().unwrap();
    assert_eq!(final_name, "file (1).txt");

    // Storage should hold both names.
    let storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    assert!(storage
        .exists(&StoragePath::new("C/file.txt").unwrap())
        .await
        .unwrap());
    assert!(storage
        .exists(&StoragePath::new("C/file (1).txt").unwrap())
        .await
        .unwrap());
}

/// Regression test for double URL-decoding in the upload handler. axum's
/// `Path<String>` extractor already percent-decodes captured segments once;
/// the handler used to call `urlencoding::decode` again, which mangled any
/// filename containing a literal `%`. A client uploading `foo%20bar.txt`
/// percent-escapes the `%` and sends `/s/<token>/upload/foo%2520bar.txt` —
/// axum decodes to `foo%20bar.txt`, and the handler must keep it intact
/// (pre-fix it decoded again to `foo bar.txt`).
#[tokio::test]
async fn upload_filename_with_percent_preserves_percent() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pct.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 5, None, None).await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    // Send `foo%2520bar.txt` — axum decodes once to `foo%20bar.txt`.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/s/{token}/upload/foo%2520bar.txt"))
        .body(Body::from("payload"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        v["name"].as_str().unwrap(),
        "foo%20bar.txt",
        "handler must preserve the literal `%` (pre-fix it double-decoded to `foo bar.txt`)"
    );

    let storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    assert!(
        storage
            .exists(&StoragePath::new("Drop/foo%20bar.txt").unwrap())
            .await
            .unwrap(),
        "file must land on disk with literal `%20` in the name"
    );
    assert!(
        !storage
            .exists(&StoragePath::new("Drop/foo bar.txt").unwrap())
            .await
            .unwrap(),
        "the double-decoded form must NOT exist on disk"
    );
}

#[tokio::test]
async fn unknown_token_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("nx.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // 15 chars of valid alphabet but no row exists.
    let req = Request::builder()
        .method("GET")
        .uri("/s/AAAAAAAAAAAAAAA/download/whatever.bin")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
