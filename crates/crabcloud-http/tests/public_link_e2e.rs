//! End-to-end tests for the anonymous public-link surface mounted under
//! `/s/{token}/...`. Drives the full `build_router` so each request travels
//! through the path-conditional `public_link_auth` middleware before
//! reaching the unlock / download / upload handlers. The fixture seeds an
//! owner home on disk, materializes a small subtree, creates the link via
//! the OCS endpoint (which exercises Batch B's `Shares::create_link`), and
//! then drives the public-link URLs anonymously.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::DIRECTORY_MIMETYPE;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, NoopEventSink, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::tempdir;
use tokio::io::AsyncRead;
use tower::ServiceExt;

async fn make_state(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    cfg.filecache.enabled = false;
    AppStateBuilder::new(cfg).build().await.unwrap()
}

async fn seed_user(state: &AppState, uid: &str) {
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new(uid).unwrap(),
                display_name: format!("{uid} display"),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
}

/// Materialise a folder under `uid`'s home on disk and ensure the filecache
/// has the chain `/`, `/seg1`, `/seg1/seg2`, … so `Shares::create_link` can
/// locate it. Idempotent.
async fn seed_folder(state: &AppState, uid: &str, path: &str) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();
    apply_dir(state, &storage_id, &StoragePath::root()).await;
    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    let segments: Vec<&str> = stripped.split('/').collect();
    let mut cur = String::new();
    for seg in segments {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(seg);
        let sp = StoragePath::new(cur.clone()).unwrap();
        if !storage.exists(&sp).await.unwrap() {
            storage.mkdir(&sp, &NoopEventSink).await.unwrap();
        }
        apply_dir(state, &storage_id, &sp).await;
    }
}

async fn apply_dir(state: &AppState, storage_id: &str, path: &StoragePath) {
    if state
        .filecache
        .lookup(storage_id, path)
        .await
        .unwrap()
        .is_some()
    {
        return;
    }
    let md = FileMetadata {
        path: path.clone(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(DIRECTORY_MIMETYPE).unwrap(),
        permissions: Permissions::full(),
    };
    let ev = StorageEvent::DirCreated {
        storage_id: storage_id.into(),
        path: path.clone(),
        metadata: md,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Write a small file under `uid`'s home on disk + filecache so the public
/// link's `download` can stream it back.
async fn seed_file(state: &AppState, uid: &str, path: &str, body: &[u8]) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let sp = StoragePath::new(path.trim_start_matches('/').to_string()).unwrap();
    let bytes = body.to_vec();
    let reader: Pin<Box<dyn AsyncRead + Send>> = Box::pin(std::io::Cursor::new(bytes));
    let meta = storage.put_file(&sp, reader, &NoopEventSink).await.unwrap();
    let storage_id = storage.id().to_string();
    // Mirror the storage `put_file` into the filecache so `View::stat` finds it.
    let ev = StorageEvent::Written {
        storage_id: storage_id.clone(),
        path: sp.clone(),
        metadata: meta,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Create a public-link share via `Shares::create` directly. This sidesteps
/// the OCS handler (which doesn't accept password/expireDate on Batch E's
/// `sp8/e-public-surface` branch — the OCS wiring lives on the sibling
/// `sp8/e-ocs-link-shape` branch and merges separately). Returns the
/// 15-char token.
async fn create_link(
    state: &AppState,
    requester: &str,
    path: &str,
    permissions: u32,
    password: Option<&str>,
) -> String {
    use crabcloud_sharing::{CreateShareRequest, ShareType};
    let home_sid = state
        .storage_factory
        .home_storage(&UserId::new(requester).unwrap())
        .await
        .unwrap()
        .id()
        .to_string();
    let req = CreateShareRequest {
        requester: requester.to_string(),
        home_storage_id: home_sid,
        path: path.to_string(),
        share_type: ShareType::Link,
        share_with: String::new(),
        permissions,
        password: password.map(|s| s.to_string()),
        expire_date: None,
    };
    let row = state.shares.create(req).await.expect("create_link");
    row.token.expect("link rows carry a token")
}

// --- download / read --------------------------------------------------------

#[tokio::test]
async fn download_read_link_returns_body() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("d.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Photos").await;
    seed_file(&state, "alice", "Photos/cat.jpg", b"meow-meow").await;
    let token = create_link(&state, "alice", "/Photos", 1, None).await;
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
    let token = create_link(&state, "alice", "/Drop", 4, None).await;
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
    let token = create_link(&state, "alice", "/Range", 1, None).await;
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
    let token = create_link(&state, "alice", "/Secret", 1, Some("correct-horse")).await;
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
    let token = create_link(&state, "alice", "/Secret", 1, Some("hunter2")).await;
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
    let token = create_link(&state, "alice", "/Vault", 1, Some("hunter2")).await;
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
    let token = create_link(&state, "alice", "/Drop", 5, None).await;
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
    assert!(storage.exists(&sp).await.unwrap(), "file written under Drop/");
}

#[tokio::test]
async fn upload_unsafe_filename_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("uns.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "Drop").await;
    let token = create_link(&state, "alice", "/Drop", 4, None).await;
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
    let token = create_link(&state, "alice", "/RO", 1, None).await;
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
    let token = create_link(&state, "alice", "/C", 5, None).await;
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
