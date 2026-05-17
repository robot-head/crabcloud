//! End-to-end tests for `COPY /dav/versions/{uid}/{fileid}/{version_mtime}`.
//!
//! Coverage:
//! - Happy path: snapshot v1 + write v2 over current; COPY-restore to
//!   v1 → current bytes match v1 + a NEW version row appears covering
//!   the v2 bytes (snapshot-before-restore).
//! - 400 when Destination header is missing.
//! - 400 when Destination doesn't match the file's current path.
//! - 404 when `version_mtime` doesn't exist.
//! - Works via `/remote.php/dav/...` alias.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use crabcloud_users::UserId;
use support::{bearer, make_state, seed_file, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

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
        .lookup(&storage.id().to_string(), &sp)
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
    state
        .filecache
        .intern_storage(&storage.id().to_string())
        .await
        .unwrap()
}

#[tokio::test]
async fn copy_restores_to_chosen_version_and_snapshots_current() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vc0.db"), data.path().to_path_buf()).await;

    // Step 1: write v1 bytes and snapshot.
    seed_file(&state, "alice", "/a.txt", b"v1-content").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let sid = storage_id_num(&state, "alice").await;
    state
        .versions
        .snapshot_if_needed(
            "alice",
            sid,
            fileid,
            "/a.txt",
            "v1-content".len() as i64,
            1_716_000_000,
            0,
            64 * 1024 * 1024,
        )
        .await
        .unwrap()
        .unwrap();

    // Step 2: overwrite current with v2 bytes (no snapshot yet — we
    // simulate the latest in-place bytes). Seed-file overwrites the
    // on-disk file directly.
    seed_file(&state, "alice", "/a.txt", b"v2-content").await;

    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    // Step 3: COPY-restore to v1.
    let req = Request::builder()
        .method("COPY")
        .uri(format!("/dav/versions/alice/{fileid}/1716000000"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify current bytes on disk now match v1.
    let current = data.path().join("alice").join("files").join("a.txt");
    let bytes = tokio::fs::read(&current).await.unwrap();
    assert_eq!(bytes, b"v1-content");

    // A NEW version row should exist for the v2 bytes that were on
    // disk at restore time (snapshot-before-restore). Total versions
    // for this fileid should now be 2: original v1 + the pre-restore
    // snapshot of v2.
    let rows = state.versions.list_for("alice", fileid).await.unwrap();
    assert_eq!(rows.len(), 2, "expected snapshot-before-restore row: {rows:?}");
    // The newer row should have the v2 size.
    let newest = &rows[0]; // list_for returns newest-first
    assert_eq!(newest.size, "v2-content".len() as i64);
}

#[tokio::test]
async fn copy_missing_destination_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vc1.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let sid = storage_id_num(&state, "alice").await;
    state
        .versions
        .snapshot_if_needed(
            "alice",
            sid,
            fileid,
            "/a.txt",
            2,
            1_716_000_000,
            0,
            64 * 1024 * 1024,
        )
        .await
        .unwrap()
        .unwrap();
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("COPY")
        .uri(format!("/dav/versions/alice/{fileid}/1716000000"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn copy_destination_mismatch_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vc2.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let sid = storage_id_num(&state, "alice").await;
    state
        .versions
        .snapshot_if_needed(
            "alice",
            sid,
            fileid,
            "/a.txt",
            2,
            1_716_000_000,
            0,
            64 * 1024 * 1024,
        )
        .await
        .unwrap()
        .unwrap();
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("COPY")
        .uri(format!("/dav/versions/alice/{fileid}/1716000000"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/wrong.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn copy_unknown_version_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vc3.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("COPY")
        .uri(format!("/dav/versions/alice/{fileid}/9999999"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn copy_works_via_remote_php_alias() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vc_alias.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"v1-bytes").await;
    let fileid = fileid_of(&state, "alice", "/a.txt").await;
    let sid = storage_id_num(&state, "alice").await;
    state
        .versions
        .snapshot_if_needed(
            "alice",
            sid,
            fileid,
            "/a.txt",
            "v1-bytes".len() as i64,
            1_716_000_000,
            0,
            64 * 1024 * 1024,
        )
        .await
        .unwrap()
        .unwrap();
    seed_file(&state, "alice", "/a.txt", b"v2-bytes").await;
    let app = crabcloud_http::build_router(state.clone(), axum::Router::new());

    let req = Request::builder()
        .method("COPY")
        .uri(format!(
            "/remote.php/dav/versions/alice/{fileid}/1716000000"
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/remote.php/dav/files/alice/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let bytes = tokio::fs::read(data.path().join("alice").join("files").join("a.txt"))
        .await
        .unwrap();
    assert_eq!(bytes, b"v1-bytes");
}
