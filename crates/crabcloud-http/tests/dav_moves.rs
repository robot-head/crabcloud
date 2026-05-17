//! Integration tests for batch C: MOVE + COPY + Destination + Overwrite.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use support::{bearer, make_state, seed_file, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

/// Build state + seed `alice`. Thin wrapper over the shared `make_state` +
/// `seed_user` so the existing call sites stay one-liners.
async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    state
}

/// Mint a Bearer token for `alice` against the live token store.
async fn alice_bearer(state: &AppState) -> String {
    bearer(state, "alice").await
}

async fn seed(app: &axum::Router, token: &str, path: &str, body: &[u8]) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/dav/files/alice/{path}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let rbody = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert!(
        status.is_success(),
        "seed put {path} failed: {status} body: {}",
        String::from_utf8_lossy(&rbody)
    );
}

#[tokio::test]
async fn move_renames_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "from.txt", b"data").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/from.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/to.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Source gone.
    let src = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/from.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(src).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // Dest present with the body.
    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/to.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"data");
}

#[tokio::test]
async fn move_overwrite_f_blocks_when_dest_exists() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "a.txt", b"A").await;
    seed(&app, &token, "b.txt", b"B").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/b.txt")
        .header("overwrite", "F")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::PRECONDITION_FAILED,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn move_overwrite_t_replaces_dest_returns_204() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "a.txt", b"AAA").await;
    seed(&app, &token, "b.txt", b"old").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/b.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "body: {}",
        String::from_utf8_lossy(&body)
    );

    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/b.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"AAA");
}

#[tokio::test]
async fn copy_duplicates_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "src.txt", b"copy-me").await;

    let req = Request::builder()
        .method("COPY")
        .uri("/dav/files/alice/src.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/dst.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "body: {}",
        String::from_utf8_lossy(&body)
    );

    for path in ["src.txt", "dst.txt"] {
        let r = Request::builder()
            .method("GET")
            .uri(format!("/dav/files/alice/{path}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let b = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&b[..], b"copy-me");
    }
}

#[tokio::test]
async fn move_to_other_user_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    seed(&app, &token, "x.txt", b"X").await;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/x.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/bob/x.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// SP13 regression: a DAV MOVE that overwrites an existing file must
/// snapshot the destination's pre-overwrite bytes into the versions
/// table BEFORE removing them. Prior to the `rename_force_overwrite`
/// fix, the handler ran `view.delete(&to)` first (routing the prior
/// bytes through trash), then `view.rename(from, &to)` whose snapshot
/// hook no-oped on the now-missing destination — losing the version.
#[tokio::test]
async fn move_overwrite_snapshots_destination_bytes() {
    use crabcloud_users::UserId;

    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("mvs.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;

    // Snapshot the destination's fileid BEFORE the move so we can query
    // `versions.list_for` after the fact. We need to resolve fileid via
    // the filecache directly because the destination row goes away during
    // the MOVE.
    // Seed both files via the support helper (which applies the
    // filecache event directly) rather than DAV PUT — the scanner is
    // disabled in tests and view.put_file would race the lookup below.
    seed_file(&state, "alice", "/src.txt", b"NEWBYTES").await;
    seed_file(&state, "alice", "/dst.txt", b"PRIORBYTES").await;
    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();
    let dst_sp = crabcloud_storage::StoragePath::new("dst.txt".to_string()).unwrap();
    let dst_row = state
        .filecache
        .lookup(&storage_id, &dst_sp)
        .await
        .unwrap()
        .expect("dst.txt row should exist after seed");
    let dst_fileid_before = dst_row.fileid;

    let req = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/src.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/dst.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Destination should now have the source's bytes…
    let dst = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/dst.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(dst).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
    assert_eq!(&b[..], b"NEWBYTES");

    // …and a version row for the destination's PRIOR bytes must exist.
    let rows = state
        .versions
        .list_for("alice", dst_fileid_before)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one version row for dst pre-overwrite bytes, got: {rows:?}"
    );
    assert_eq!(rows[0].size, b"PRIORBYTES".len() as i64);
    assert_eq!(rows[0].user, "alice");
}
