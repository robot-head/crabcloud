//! HTTP-level integration tests for the versions server fns
//! (`/api/files/versions/{list,restore,delete}`). Mirrors the
//! `server_fns_trash.rs` scaffold — drives the full `build_router`
//! stack so requests travel through the production auth middleware
//! and the dx fullstack server-fn handler.
//!
//! Versions are seeded by calling the `Versions` service directly
//! rather than by routing a `View::write_file` through the file-DAV
//! PUT path; the higher-level snapshot trigger is covered in the
//! crabcloud-fs + crabcloud-versions suites. Keeping these tests
//! close to the service boundary makes failures easy to attribute
//! when a refactor moves either layer.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_storage::{NoopEventSink, StorageEvent, StoragePath};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use dioxus::server::{DioxusRouterExt, FullstackState};
use std::pin::Pin;
use tempfile::tempdir;
use tokio::io::AsyncRead;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
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

async fn make_user(state: &AppState, uid: &str) {
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new(uid).unwrap(),
                display_name: uid.into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
}

async fn bearer_for(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "UI",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

fn build_app(state: AppState) -> axum::Router {
    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    crabcloud_http::build_router(state, dioxus_router)
}

/// Materialise a small file under `uid`'s home on disk + filecache so
/// the versions service has something to snapshot.
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

async fn fileid_of(state: &AppState, uid: &str, path: &str) -> i64 {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let sp = StoragePath::new(path.trim_start_matches('/').to_string()).unwrap();
    state
        .filecache
        .lookup(storage.id(), &sp)
        .await
        .unwrap()
        .unwrap()
        .fileid
}

async fn storage_id_num(state: &AppState, uid: &str) -> i64 {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    state.filecache.intern_storage(storage.id()).await.unwrap()
}

/// Snapshot `path` directly via the versions service. Returns the new
/// row's id. `now_secs` is the version_mtime suffix on disk.
async fn snapshot(state: &AppState, uid: &str, path: &str, size: i64, now_secs: i64) -> i64 {
    let fileid = fileid_of(state, uid, path).await;
    let sid = storage_id_num(state, uid).await;
    state
        .versions
        .snapshot_if_needed(uid, sid, fileid, path, size, now_secs, 0, 64 * 1024 * 1024)
        .await
        .unwrap()
        .unwrap()
}

async fn post_json(
    app: &axum::Router,
    token: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn decode_bytes(resp: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn list_versions_returns_seeded_row() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/a.txt", b"v1-bytes").await;
    let id = snapshot(&state, "alice", "/a.txt", 8, 1_716_000_000).await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/versions/list",
        serde_json::json!({ "fileid": fileid }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let entries: Vec<crabcloud_app::VersionDto> =
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "decode VersionDto list: {e} body={:?}",
                String::from_utf8_lossy(&body)
            )
        });
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!(e.id, id);
    assert_eq!(e.version_mtime, 1_716_000_000);
    assert_eq!(e.size, 8);
}

#[tokio::test]
async fn restore_version_overwrites_current_and_returns_ok() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("r.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/a.txt", b"v1-content").await;
    let v1_id = snapshot(&state, "alice", "/a.txt", 10, 1_716_000_000).await;
    seed_file(&state, "alice", "/a.txt", b"v2-content").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = build_app(state.clone());

    let resp = post_json(
        &app,
        &token,
        "/api/files/versions/restore",
        serde_json::json!({ "version_id": v1_id }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );

    let current = data.path().join("alice").join("files").join("a.txt");
    let bytes = tokio::fs::read(&current).await.unwrap();
    assert_eq!(bytes, b"v1-content");

    // Snapshot-before-restore created a second row.
    let rows = state.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
}

#[tokio::test]
async fn delete_version_removes_row_and_on_disk_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("d.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/a.txt", b"v1-bytes").await;
    let id = snapshot(&state, "alice", "/a.txt", 8, 1_716_000_000).await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = build_app(state.clone());

    let resp = post_json(
        &app,
        &token,
        "/api/files/versions/delete",
        serde_json::json!({ "version_id": id }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );

    let rows = state.versions.list_for("alice", fileid).await.unwrap();
    assert!(rows.is_empty(), "{rows:?}");
    let on_disk = data
        .path()
        .join("alice")
        .join("files_versions")
        .join("a.txt.v1716000000");
    assert!(
        !on_disk.exists(),
        "version file still on disk at {on_disk:?}"
    );
}

#[tokio::test]
async fn restore_other_users_version_returns_non_ok() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("rx.db"), data.path().to_path_buf()).await;
    make_user(&state, "bob").await;
    let bob_token = bearer_for(&state, "bob").await;
    seed_file(&state, "alice", "/secret.txt", b"hi").await;
    let id = snapshot(&state, "alice", "/secret.txt", 2, 1_716_000_000).await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &bob_token,
        "/api/files/versions/restore",
        serde_json::json!({ "version_id": id }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    // ServerFnError surfaces as 500; the body string is "forbidden".
    assert_ne!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("forbidden"), "body={text}");
}

#[tokio::test]
async fn list_versions_unauthenticated_returns_non_ok() {
    // Same contract as `server_fns_trash`: AuthLayer only 401s when an
    // auth header is present-but-invalid. With no auth at all the
    // request falls through anonymous; the server fn body returns
    // `unauthorized` (mapped to 500). Either way it's not 200.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/versions/list")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from("{\"fileid\":1}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}
