//! End-to-end tests for `GET /dav/versions/{uid}/{fileid}/{version_mtime}`.
//!
//! Coverage:
//! - 200 + body == snapshotted bytes + `Content-Length` matches.
//! - 404 on unknown `version_mtime`.
//! - Cross-user URL → 403.
//! - Both surface prefixes reach the same handler.

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

/// Snapshot a fresh version row + on-disk bytes. Returns
/// `(fileid, version_mtime, size)`.
async fn seed_version(
    state: &AppState,
    uid: &str,
    path: &str,
    body: &[u8],
    mtime: i64,
) -> (i64, i64, i64) {
    seed_file(state, uid, path, body).await;
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id_str = storage.id().to_string();
    let sp = crabcloud_storage::StoragePath::new(path.trim_start_matches('/').to_string()).unwrap();
    let row = state
        .filecache
        .lookup(&storage_id_str, &sp)
        .await
        .unwrap()
        .unwrap();
    let storage_id_num = state
        .filecache
        .intern_storage(&storage_id_str)
        .await
        .expect("intern storage_id");
    state
        .versions
        .snapshot_if_needed(
            uid,
            storage_id_num,
            row.fileid,
            path,
            body.len() as i64,
            mtime,
            0,
            64 * 1024 * 1024,
        )
        .await
        .expect("snapshot_if_needed")
        .expect("snapshot returned a row id");
    (row.fileid, mtime, body.len() as i64)
}

#[tokio::test]
async fn get_streams_version_bytes_with_content_length() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vg0.db"), data.path().to_path_buf()).await;
    let payload = b"version-one-bytes";
    let (fileid, mtime, sz) = seed_version(&state, "alice", "/a.txt", payload, 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/dav/versions/alice/{fileid}/{mtime}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        &sz.to_string()
    );
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], payload);
}

#[tokio::test]
async fn get_works_via_remote_php_alias() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) =
        make_alice(dir.path().join("vg_alias.db"), data.path().to_path_buf()).await;
    let payload = b"hello";
    let (fileid, mtime, _sz) =
        seed_version(&state, "alice", "/a.txt", payload, 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/remote.php/dav/versions/alice/{fileid}/{mtime}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], payload);
}

#[tokio::test]
async fn get_unknown_version_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vg404.db"), data.path().to_path_buf()).await;
    let (fileid, _mtime, _sz) = seed_version(&state, "alice", "/a.txt", b"hi", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri(format!("/dav/versions/alice/{fileid}/99999999"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_wrong_uid_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vg403.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri("/dav/versions/bob/1/1716000000")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
