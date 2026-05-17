//! End-to-end tests for the OCS `apps/files_trashbin/api/v1/` endpoints.
//!
//! Drives the full `build_router` so requests travel through the real
//! auth + middleware stack (Bearer + `OCS-APIRequest` header — matches
//! how desktop clients hit the surface). Each test seeds entries by
//! calling the trash service directly so we don't depend on the higher
//! `View::delete` reroute path here (covered separately in the
//! crabcloud-trash + crabcloud-fs suites).

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use crabcloud_trash::TrashType;
use support::{bearer, make_state, seed_file, seed_folder, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

const BASE: &str = "/ocs/v2.php/apps/files_trashbin/api/v1";

async fn make_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> (AppState, String) {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    (state, token)
}

/// Soft-delete a file via the trash service directly. Returns the
/// resulting row id (the only thing callers need for restore / purge).
async fn soft_delete(state: &AppState, uid: &str, path: &str) -> i64 {
    state
        .trash
        .soft_delete(uid, path, TrashType::File, None)
        .await
        .expect("soft_delete")
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
async fn list_returns_entries_in_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("l.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/trashbin?format=json"), &token))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    let data = v["ocs"]["data"].as_array().expect("data is array");
    assert_eq!(data.len(), 1, "{v}");
    assert_eq!(data[0]["id"], id);
    assert_eq!(data[0]["basename"], "a.txt");
    assert_eq!(data[0]["location"], "/");
    assert_eq!(data[0]["type"], "file");
    assert!(data[0]["suffix"].as_str().unwrap().starts_with('d'));
}

#[tokio::test]
async fn restore_returns_path_and_clears_row() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("r.db"), data.path().to_path_buf()).await;
    seed_folder(&state, "alice", "notes").await;
    seed_file(&state, "alice", "/notes/n.txt", b"body").await;
    let id = soft_delete(&state, "alice", "/notes/n.txt").await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/restore/{id}?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(v["ocs"]["data"]["path"], "/notes/n.txt");

    // List is now empty.
    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/trashbin?format=json"), &token))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    assert!(v["ocs"]["data"].as_array().unwrap().is_empty(), "{v}");

    // File back on disk at the original location.
    let restored = data.path().join("alice").join("files").join("notes/n.txt");
    assert!(restored.exists(), "restored file missing at {restored:?}");
}

#[tokio::test]
async fn purge_one_clears_entry() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("p.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/x.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/x.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_delete(
            &format!("{BASE}/trash/{id}?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);

    // List is now empty.
    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/trashbin?format=json"), &token))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    assert!(v["ocs"]["data"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test]
async fn empty_purges_all_entries_and_returns_count() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("e.db"), data.path().to_path_buf()).await;
    // Seed three files. Use distinct basenames so suffix collisions
    // (`d<secs>_2`) don't confuse the assertion on `purged: 3` if two
    // soft-deletes share a second.
    for name in ["one.txt", "two.txt", "three.txt"] {
        seed_file(&state, "alice", &format!("/{name}"), b"x").await;
        soft_delete(&state, "alice", &format!("/{name}")).await;
    }
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_delete(&format!("{BASE}/trash?format=json"), &token))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(v["ocs"]["data"]["purged"], 3);

    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/trashbin?format=json"), &token))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    assert!(v["ocs"]["data"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test]
async fn restore_not_found_returns_404_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("nf.db"), data.path().to_path_buf()).await;
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
async fn restore_other_users_entry_is_forbidden() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, _alice_token) =
        make_alice(dir.path().join("ow.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let bob_token = bearer(&state, "bob").await;
    seed_file(&state, "alice", "/secret.txt", b"hi").await;
    let id = soft_delete(&state, "alice", "/secret.txt").await;
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
