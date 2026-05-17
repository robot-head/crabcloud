//! End-to-end tests for the OCS `apps/files_versions/api/v1/` endpoints.
//!
//! Drives the full `build_router` so requests travel through the real
//! auth + middleware stack (Bearer + `OCS-APIRequest` header — matches
//! how desktop / third-party OCS clients hit the surface). Each test
//! seeds versions by calling the `Versions` service directly so the
//! coverage here stays focused on the OCS wire surface; the underlying
//! snapshot/restore semantics are exercised by the crabcloud-versions
//! suite and the DAV COPY tests in `dav_versions_copy.rs`.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use crabcloud_users::UserId;
use support::{bearer, make_state, seed_file, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

const BASE: &str = "/ocs/v2.php/apps/files_versions/api/v1";

async fn make_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> (AppState, String) {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    (state, token)
}

async fn fileid_of(state: &AppState, uid: &str, path: &str) -> i64 {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let sp = crabcloud_storage::StoragePath::new(path.trim_start_matches('/').to_string()).unwrap();
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

/// Snapshot `path` directly via the Versions service. Returns the new
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

fn ocs_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap()
}

fn ocs_post(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap()
}

fn ocs_delete(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap()
}

async fn decode(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn list_returns_versions_in_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vl.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"v1-bytes").await;
    let id = snapshot(&state, "alice", "/a.txt", 8, 1_716_000_000).await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/versions/{fileid}?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    let data = v["ocs"]["data"].as_array().expect("data is array");
    assert_eq!(data.len(), 1, "{v}");
    assert_eq!(data[0]["id"], id);
    assert_eq!(data[0]["fileid"], fileid);
    assert_eq!(data[0]["version_mtime"], 1_716_000_000);
    assert_eq!(data[0]["size"], 8);
}

#[tokio::test]
async fn list_unknown_fileid_returns_empty_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vl0.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/versions/9999?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    assert!(v["ocs"]["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn restore_overwrites_current_and_snapshots_prerestore_bytes() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vr.db"), data.path().to_path_buf()).await;

    // Write v1, snapshot it, then overwrite current with v2 bytes.
    seed_file(&state, "alice", "/a.txt", b"v1-content").await;
    let v1_id = snapshot(&state, "alice", "/a.txt", 10, 1_716_000_000).await;
    seed_file(&state, "alice", "/a.txt", b"v2-content").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let resp = app
        .oneshot(ocs_post(
            &format!("{BASE}/restore/{v1_id}?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);

    // Current bytes now match v1.
    let current = data.path().join("alice").join("files").join("a.txt");
    let bytes = tokio::fs::read(&current).await.unwrap();
    assert_eq!(bytes, b"v1-content");

    // A NEW row should exist for the pre-restore v2 bytes — total 2.
    let rows = state.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(rows.len(), 2, "expected pre-restore snapshot row: {rows:?}");
}

#[tokio::test]
async fn restore_unknown_version_id_returns_404_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vr0.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_post(
            &format!("{BASE}/restore/9999?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={v}");
    // OCS's NotFound status maps to envelope statuscode 998.
    assert_eq!(v["ocs"]["meta"]["statuscode"], 998);
}

#[tokio::test]
async fn restore_other_users_version_is_forbidden() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, _alice_token) =
        make_alice(dir.path().join("vrx.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let bob_token = bearer(&state, "bob").await;
    seed_file(&state, "alice", "/secret.txt", b"hi").await;
    let id = snapshot(&state, "alice", "/secret.txt", 2, 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_post(
            &format!("{BASE}/restore/{id}?format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 403);
}

#[tokio::test]
async fn delete_one_removes_row_and_on_disk_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vd.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"v1-bytes").await;
    let id = snapshot(&state, "alice", "/a.txt", 8, 1_716_000_000).await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let resp = app
        .oneshot(ocs_delete(
            &format!("{BASE}/version/{id}?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);

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
async fn delete_unknown_version_id_returns_404_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vd0.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_delete(
            &format!("{BASE}/version/9999?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 998);
}

#[tokio::test]
async fn delete_other_users_version_is_forbidden() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, _alice_token) =
        make_alice(dir.path().join("vdx.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let bob_token = bearer(&state, "bob").await;
    seed_file(&state, "alice", "/secret.txt", b"hi").await;
    let id = snapshot(&state, "alice", "/secret.txt", 2, 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_delete(
            &format!("{BASE}/version/{id}?format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 403);
}
