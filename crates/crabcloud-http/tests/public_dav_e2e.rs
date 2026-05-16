//! End-to-end tests for the anonymous public-link WebDAV surface mounted
//! under `/public.php/dav/files/{token}/...`. Drives the full
//! `build_router` so each request travels through the new
//! `public_dav_gate` middleware (HTTP Basic against the link's bcrypt
//! hash) before reaching the surface-neutral DAV handlers.
//!
//! Fixture mirrors `dav_basic.rs` (filecache scanner disabled) plus the
//! seed/create-link helpers from `public_link_e2e.rs`: seed an owner home
//! on disk, materialise a small subtree, create the link directly via
//! `Shares::create` (sidesteps the OCS handler), then drive the public
//! DAV URLs anonymously.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
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
    // Disable the filecache scanner: under `cargo test --workspace` on
    // Linux CI the scanner's async event-apply races our handler's
    // follow-up `view.stat` calls. Matches the workaround in
    // `dav_basic.rs` and `public_link_e2e.rs`.
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

/// Materialise a folder under `uid`'s home on disk and ensure the
/// filecache has the chain `/`, `/seg1`, `/seg1/seg2`, … so
/// `Shares::create_link` can locate it.
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

/// Write a small file under `uid`'s home on disk + filecache so the
/// public DAV link's GET / PROPFIND can serve it back.
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
    let ev = StorageEvent::Written {
        storage_id: storage_id.clone(),
        path: sp.clone(),
        metadata: meta,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Create a public-link share via `Shares::create` directly. Mirrors the
/// helper in `public_link_e2e.rs`; supports password + expiration.
async fn create_link(
    state: &AppState,
    requester: &str,
    path: &str,
    permissions: u32,
    password: Option<&str>,
    expire_date: Option<chrono::NaiveDate>,
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
        expire_date,
    };
    let row = state.shares.create(req).await.expect("create_link");
    row.token.expect("link rows carry a token")
}

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
