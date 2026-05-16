//! End-to-end tests for the authed folder-zip endpoint
//! (`GET /api/files/zip/{*path}`). Drives the full `build_router` so each
//! request runs through the real `AuthLayer` and our handler picks up the
//! `Extension<AuthContext>` via Bearer auth.

#![allow(unused_crate_dependencies)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::DIRECTORY_MIMETYPE;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, NoopEventSink, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use std::io::Cursor;
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::tempdir;
use tokio::io::AsyncRead;
use tower::ServiceExt;

const BODY_LIMIT: usize = 16 * 1024 * 1024;

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

async fn bearer(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "ZIP",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
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

async fn seed_folder(state: &AppState, uid: &str, path: &str) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();
    apply_dir(state, &storage_id, &StoragePath::root()).await;
    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    if stripped.is_empty() {
        return;
    }
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
        storage_id,
        path: sp,
        metadata: meta,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Seed `<root>/cat.txt`, `<root>/dog.txt`, and
/// `<root>/vacation/beach.txt` under the user's home with deterministic
/// byte contents. Mirrors the helper proposed by the plan.
async fn seed_zip_tree(state: &AppState, uid: &str, root: &str) {
    seed_folder(state, uid, root).await;
    seed_folder(state, uid, &format!("{root}/vacation")).await;
    seed_file(state, uid, &format!("{root}/cat.txt"), b"cat-text").await;
    seed_file(state, uid, &format!("{root}/dog.txt"), b"dog-text").await;
    seed_file(
        state,
        uid,
        &format!("{root}/vacation/beach.txt"),
        b"beach-text-bytes",
    )
    .await;
}

#[tokio::test]
async fn authed_zip_returns_200_application_zip() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("ok.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/zip"
    );
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("filename=\"Photos.zip\""), "got: {cd}");
    let body = to_bytes(resp.into_body(), BODY_LIMIT).await.unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(body.to_vec())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert!(
        names.iter().any(|n| n == "Photos/cat.txt"),
        "missing Photos/cat.txt in {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Photos/dog.txt"),
        "missing Photos/dog.txt in {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Photos/vacation/beach.txt"),
        "missing Photos/vacation/beach.txt in {names:?}"
    );
}

#[tokio::test]
async fn authed_zip_over_cap_returns_413_with_summary() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(dir.path().join("cap.db"));
    cfg.datadirectory = data.path().to_path_buf();
    cfg.filecache.enabled = false;
    // Force the cap to 1 entry — the tree has 5, so the walk overflows.
    cfg.folder_zip_max_entries = 1;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/json"
    );
    let body = to_bytes(resp.into_body(), BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "folder too large");
    assert!(v["entries"].as_u64().unwrap() >= 2);
    assert_eq!(v["limits"]["max_entries"], 1);
    assert!(v["limits"]["max_bytes"].as_u64().is_some());
}

#[tokio::test]
async fn authed_zip_of_regular_file_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("file.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "/").await;
    seed_file(&state, "alice", "/note.txt", b"hello").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/note.txt")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn authed_zip_unknown_path_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("nx.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "/").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/does_not_exist")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn authed_zip_root_uses_uid_basename() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("root.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("filename=\"alice.zip\""), "got: {cd}");
}
