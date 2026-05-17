//! HTTP-level integration tests for the trash server fns
//! (`/api/files/trash/{list,restore,purge,empty}`). Mirrors the
//! `server_fns_files.rs` scaffold — drives the full `build_router`
//! stack so requests travel through the production auth middleware
//! and the dx fullstack server-fn handler.
//!
//! Entries are seeded by calling the trash service directly rather
//! than by routing a `View::delete` through the file-DAV PUT path;
//! the higher-level reroute is covered in the crabcloud-fs +
//! crabcloud-trash suites. Keeping these tests close to the
//! service boundary makes failures easy to attribute when a
//! refactor moves either layer.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_storage::{NoopEventSink, StorageEvent, StoragePath};
use crabcloud_trash::TrashType;
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
/// the trash service's `rename`-based soft-delete has something to move.
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

/// Soft-delete `path` for `uid` and return the trash row id.
async fn soft_delete(state: &AppState, uid: &str, path: &str) -> i64 {
    state
        .trash
        .soft_delete(uid, path, TrashType::File, None)
        .await
        .expect("soft_delete")
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
async fn list_trash_returns_seeded_entry() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/a.txt").await;
    let app = build_app(state);

    let resp = post_json(&app, &token, "/api/files/trash/list", serde_json::json!({})).await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let entries: Vec<crabcloud_app::TrashEntryDto> =
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!("decode TrashEntryDto list: {e} body={:?}", String::from_utf8_lossy(&body))
        });
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!(e.id, id);
    assert_eq!(e.basename, "a.txt");
    assert_eq!(e.location, "/");
    assert_eq!(e.r#type, "file");
    assert!(e.suffix.starts_with('d'), "{e:?}");
}

#[tokio::test]
async fn restore_trash_returns_path_and_empties_list() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("r.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/n.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/n.txt").await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/trash/restore",
        serde_json::json!({ "id": id }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let restored: crabcloud_app::RestoredDto = serde_json::from_slice(&body).unwrap();
    assert_eq!(restored.path, "/n.txt");

    // Subsequent list is empty.
    let resp = post_json(&app, &token, "/api/files/trash/list", serde_json::json!({})).await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let entries: Vec<crabcloud_app::TrashEntryDto> = serde_json::from_slice(&body).unwrap();
    assert!(entries.is_empty(), "{entries:?}");

    // File back on disk at original location.
    let restored_path = data.path().join("alice").join("files").join("n.txt");
    assert!(restored_path.exists(), "missing at {restored_path:?}");
}

#[tokio::test]
async fn purge_trash_removes_single_entry() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_file(&state, "alice", "/x.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/x.txt").await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/trash/purge",
        serde_json::json!({ "id": id }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );

    let resp = post_json(&app, &token, "/api/files/trash/list", serde_json::json!({})).await;
    let (_, body) = decode_bytes(resp).await;
    let entries: Vec<crabcloud_app::TrashEntryDto> = serde_json::from_slice(&body).unwrap();
    assert!(entries.is_empty(), "{entries:?}");
}

#[tokio::test]
async fn empty_trash_returns_count_and_clears_bin() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("e.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    for name in ["one.txt", "two.txt", "three.txt"] {
        seed_file(&state, "alice", &format!("/{name}"), b"x").await;
        soft_delete(&state, "alice", &format!("/{name}")).await;
    }
    let app = build_app(state);

    let resp = post_json(&app, &token, "/api/files/trash/empty", serde_json::json!({})).await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let n: u64 = serde_json::from_slice(&body).unwrap();
    assert_eq!(n, 3);

    let resp = post_json(&app, &token, "/api/files/trash/list", serde_json::json!({})).await;
    let (_, body) = decode_bytes(resp).await;
    let entries: Vec<crabcloud_app::TrashEntryDto> = serde_json::from_slice(&body).unwrap();
    assert!(entries.is_empty(), "{entries:?}");
}

#[tokio::test]
async fn list_trash_unauthenticated_returns_non_ok() {
    // Same contract as `server_fns_files`: AuthLayer only 401s when an
    // auth header is present-but-invalid. With no auth at all the
    // request falls through anonymous; the server fn body returns
    // `unauthorized` (mapped to 500). Either way it's not 200.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/trash/list")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}
