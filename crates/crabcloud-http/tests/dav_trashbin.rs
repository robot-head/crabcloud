//! End-to-end tests for the DAV trashbin surface mounted at
//! `/dav/trashbin/{uid}/...` and `/remote.php/dav/trashbin/{uid}/...`.
//!
//! Coverage:
//! - PROPFIND root (Depth 0 + 1): root collection plus per-entry
//!   responses with `<nc:trashbin-original-location>`.
//! - PROPFIND per-entry: 207 with single response; 404 on unknown name.
//! - DELETE: 204 + on-disk file gone + row gone; 404 on unknown name.
//! - MOVE: implicit restore (no Destination) returns 201 + restores at
//!   original location; explicit Destination puts the file where asked;
//!   collision suffixes ` (restored)`; cross-user Destination → 400.
//! - 405s for unsupported methods (PUT/MKCOL).
//! - Both surface prefixes (`/dav/...` and `/remote.php/dav/...`) reach
//!   the same handler.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use crabcloud_trash::TrashType;
use support::{bearer, make_state, seed_file, seed_folder, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> (AppState, String) {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    (state, token)
}

/// Soft-delete a file via the trash service directly. Returns the
/// resulting row's `(basename, suffix)` pair (split from the `id`
/// the service returns and a follow-up list).
async fn soft_delete(state: &AppState, uid: &str, path: &str) -> (String, String) {
    let id = state
        .trash
        .soft_delete(uid, path, TrashType::File, None)
        .await
        .expect("soft_delete");
    // Locate the row to recover (basename, suffix).
    let entries = state.trash.list(uid).await.expect("list");
    let row = entries
        .into_iter()
        .find(|e| e.id == id)
        .expect("row appears in list");
    (row.basename, row.suffix)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ----- PROPFIND --------------------------------------------------------------

#[tokio::test]
async fn propfind_root_depth_0_returns_collection_only() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("p0.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    soft_delete(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/trashbin/alice/")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    // Exactly one response — the collection itself, no entries on depth 0.
    assert_eq!(body.matches("<d:response>").count(), 1, "{body}");
    assert!(
        body.contains("<d:displayname>trash</d:displayname>"),
        "{body}"
    );
}

#[tokio::test]
async fn propfind_root_depth_1_lists_entries_with_original_location() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("p1.db"), data.path().to_path_buf()).await;
    seed_folder(&state, "alice", "notes").await;
    seed_file(&state, "alice", "/notes/x.txt", b"body").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/notes/x.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/trashbin/alice/")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    // Two responses: trash root + one entry.
    assert_eq!(body.matches("<d:response>").count(), 2, "{body}");
    assert_eq!(basename, "x.txt");
    assert!(suffix.starts_with("d"), "{suffix}");
    assert!(
        body.contains(&format!(
            "/remote.php/dav/trashbin/alice/trash/x.txt.{suffix}"
        )),
        "missing entry href: {body}"
    );
    assert!(
        body.contains(
            "<nc:trashbin-original-location>/notes/x.txt</nc:trashbin-original-location>"
        ),
        "missing original-location: {body}"
    );
    assert!(
        body.contains("<d:displayname>x.txt</d:displayname>"),
        "missing displayname: {body}"
    );
    assert!(
        body.contains("<d:getcontentlength>4</d:getcontentlength>"),
        "missing size: {body}"
    );
}

#[tokio::test]
async fn propfind_per_entry_returns_single_response() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("p2.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert_eq!(body.matches("<d:response>").count(), 1, "{body}");
    assert!(
        body.contains("<nc:trashbin-original-location>/a.txt</nc:trashbin-original-location>"),
        "{body}"
    );
}

#[tokio::test]
async fn propfind_per_entry_unknown_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("p3.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/trashbin/alice/trash/nope.d12345")
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
    let (state, token) = make_alice(dir.path().join("p4.db"), data.path().to_path_buf()).await;
    // Also create a second user to put in the URL.
    seed_user(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/trashbin/bob/")
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
    let (state, token) = make_alice(dir.path().join("p5.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    soft_delete(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/remote.php/dav/trashbin/alice/")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
}

// ----- DELETE ---------------------------------------------------------------

#[tokio::test]
async fn delete_purges_entry_and_removes_bytes() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("d.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let trash_file = data
        .path()
        .join("alice")
        .join("files_trashbin")
        .join("files")
        .join(format!("{basename}.{suffix}"));
    assert!(
        trash_file.exists(),
        "fixture: soft_delete should have moved file"
    );

    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let remaining = state.trash.list("alice").await.unwrap();
    assert!(remaining.is_empty(), "row should be gone");
    assert!(!trash_file.exists(), "bytes should be gone");
}

#[tokio::test]
async fn delete_works_via_remote_php_alias() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("d_alias.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let trash_file = data
        .path()
        .join("alice")
        .join("files_trashbin")
        .join("files")
        .join(format!("{basename}.{suffix}"));
    assert!(
        trash_file.exists(),
        "fixture: soft_delete should have moved file"
    );

    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/remote.php/dav/trashbin/alice/trash/{basename}.{suffix}"
        ))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let remaining = state.trash.list("alice").await.unwrap();
    assert!(remaining.is_empty(), "row should be gone");
    assert!(!trash_file.exists(), "bytes should be gone");
}

#[tokio::test]
async fn delete_unknown_entry_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("du.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("DELETE")
        .uri("/dav/trashbin/alice/trash/missing.d1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ----- MOVE -----------------------------------------------------------------

#[tokio::test]
async fn move_without_destination_restores_to_original_location() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("m1.db"), data.path().to_path_buf()).await;
    seed_folder(&state, "alice", "notes").await;
    seed_file(&state, "alice", "/notes/x.txt", b"body").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/notes/x.txt").await;
    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("MOVE")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let restored = data
        .path()
        .join("alice")
        .join("files")
        .join("notes")
        .join("x.txt");
    assert!(restored.exists(), "file restored to original location");
    assert!(state.trash.list("alice").await.unwrap().is_empty());
}

#[tokio::test]
async fn move_with_destination_restores_to_explicit_path() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("m2.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hello").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("MOVE")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/restored/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let restored = data
        .path()
        .join("alice")
        .join("files")
        .join("restored")
        .join("a.txt");
    assert!(restored.exists(), "file at explicit destination");
}

#[tokio::test]
async fn move_works_via_remote_php_alias() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) =
        make_alice(dir.path().join("m_alias.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hello").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("MOVE")
        .uri(format!(
            "/remote.php/dav/trashbin/alice/trash/{basename}.{suffix}"
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/remote.php/dav/files/alice/restored/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let restored = data
        .path()
        .join("alice")
        .join("files")
        .join("restored")
        .join("a.txt");
    assert!(restored.exists(), "file at explicit destination");
    assert!(state.trash.list("alice").await.unwrap().is_empty());
}

#[tokio::test]
async fn move_with_collision_suffixes_restored() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("m3.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hello").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    // Pre-create a colliding file at the restore target.
    seed_file(&state, "alice", "/a.txt", b"newer").await;
    let state_for_app = state.clone();
    let app = crabcloud_http::build_router(state_for_app, axum::Router::new());

    let req = Request::builder()
        .method("MOVE")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let original = data.path().join("alice").join("files").join("a.txt");
    let restored_with_suffix = data
        .path()
        .join("alice")
        .join("files")
        .join("a.txt (restored)");
    assert!(original.exists(), "newer file untouched");
    assert!(
        restored_with_suffix.exists(),
        "restored copy got ` (restored)` suffix"
    );
}

#[tokio::test]
async fn move_destination_for_other_user_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("m4.db"), data.path().to_path_buf()).await;
    seed_file(&state, "alice", "/a.txt", b"hi").await;
    let (basename, suffix) = soft_delete(&state, "alice", "/a.txt").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("MOVE")
        .uri(format!("/dav/trashbin/alice/trash/{basename}.{suffix}"))
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/bob/a.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ----- Unsupported methods --------------------------------------------------

#[tokio::test]
async fn put_returns_405() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("PUT")
        .uri("/dav/trashbin/alice/trash/x.d1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("nope"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn options_returns_dav_capability() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("o.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/dav/trashbin/alice/")
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
