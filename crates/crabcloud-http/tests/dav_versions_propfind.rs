//! End-to-end tests for `PROPFIND /dav/versions/{uid}/{fileid}/...`
//! (and the `/remote.php/dav/versions/...` alias).
//!
//! Coverage:
//! - PROPFIND root (Depth 0 + 1): collection-only on 0, collection +
//!   per-version responses on 1.
//! - PROPFIND per-entry: 207 with a single response; 404 on unknown
//!   `version_mtime`.
//! - Cross-user URL → 403.
//! - Both surface prefixes (`/dav/...` and `/remote.php/dav/...`) reach
//!   the same handler.

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

/// Snapshot a fresh version row directly via the versions service.
/// Returns `(fileid, version_mtime, size)` for the resulting row.
async fn seed_version(
    state: &AppState,
    uid: &str,
    path: &str,
    body: &[u8],
    mtime: i64,
) -> (i64, i64, i64) {
    // Seed the current file on disk + filecache so `snapshot_if_needed`
    // has a source to copy from.
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
            // disable throttle for fixtures
            0,
            64 * 1024 * 1024,
        )
        .await
        .expect("snapshot_if_needed")
        .expect("snapshot returned a row id");
    (row.fileid, mtime, body.len() as i64)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn propfind_root_depth_0_returns_collection_only() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp0.db"), data.path().to_path_buf()).await;
    let (fileid, _mtime, _sz) =
        seed_version(&state, "alice", "/a.txt", b"hello", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/dav/versions/alice/{fileid}/"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    // Exactly one response — the collection itself, no entries on depth 0.
    assert_eq!(body.matches("<d:response>").count(), 1, "{body}");
}

#[tokio::test]
async fn propfind_root_depth_1_lists_versions() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp1.db"), data.path().to_path_buf()).await;
    let (fileid, mtime, sz) =
        seed_version(&state, "alice", "/a.txt", b"hello", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/dav/versions/alice/{fileid}/"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    // Two responses: root collection + one version.
    assert_eq!(body.matches("<d:response>").count(), 2, "{body}");
    assert!(
        body.contains(&format!("/remote.php/dav/versions/alice/{fileid}/{mtime}")),
        "missing version href: {body}"
    );
    assert!(
        body.contains(&format!("<d:getcontentlength>{sz}</d:getcontentlength>")),
        "missing size: {body}"
    );
    assert!(
        body.contains("<d:displayname>a.txt</d:displayname>"),
        "missing displayname: {body}"
    );
}

#[tokio::test]
async fn propfind_per_entry_returns_single_response() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp2.db"), data.path().to_path_buf()).await;
    let (fileid, mtime, _sz) = seed_version(&state, "alice", "/a.txt", b"hi", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/dav/versions/alice/{fileid}/{mtime}"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert_eq!(body.matches("<d:response>").count(), 1, "{body}");
}

#[tokio::test]
async fn propfind_unknown_version_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp3.db"), data.path().to_path_buf()).await;
    let (fileid, _mtime, _sz) = seed_version(&state, "alice", "/a.txt", b"hi", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/dav/versions/alice/{fileid}/9999999"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn propfind_wrong_uid_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp4.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/versions/bob/123/")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn propfind_works_via_remote_php_alias() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp5.db"), data.path().to_path_buf()).await;
    let (fileid, _mtime, _sz) = seed_version(&state, "alice", "/a.txt", b"hi", 1_716_000_000).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/remote.php/dav/versions/alice/{fileid}/"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
}

#[tokio::test]
async fn put_returns_405() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp_put.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("PUT")
        .uri("/dav/versions/alice/1/1716000000")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("nope"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn options_advertises_dav_capability() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("vp_opt.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/versions/alice/1/")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("dav").unwrap().to_str().unwrap(),
        "1, 2, 3"
    );
}
