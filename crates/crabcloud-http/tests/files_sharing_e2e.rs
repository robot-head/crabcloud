//! End-to-end tests for the OCS `apps/files_sharing/api/v1/` endpoints.
//!
//! Each test drives the full `build_router` so requests travel through the
//! real auth + middleware stack (Bearer + `OCS-APIRequest` header — matches
//! how desktop clients hit the surface). The filecache scanner is disabled
//! in the test config; the test seeds the share target via
//! `FileCache::apply` directly so `Shares::create` finds the path.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::DIRECTORY_MIMETYPE;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, NoopEventSink, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::tempdir;
use tower::ServiceExt;

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
                display_name: format!("{} display", capitalize(uid)),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

async fn issue_bearer(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "test",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

/// Seed `path` (folder) under `uid`'s home in the filecache. Mirrors the
/// helper used in the sharing crate's integration tests but inlined here so
/// the crate doesn't grow a circular dev-dep.
async fn seed_folder(state: &AppState, uid: &str, path: &str) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();

    // Root dir first (idempotent).
    apply_dir(state, &storage_id, &StoragePath::root()).await;

    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    let segments: Vec<&str> = stripped.split('/').collect();
    let mut cur = String::new();
    for seg in segments {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(seg);
        let sp = StoragePath::new(cur.clone()).unwrap();
        apply_dir(state, &storage_id, &sp).await;
    }
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

const BASE: &str = "/ocs/v2.php/apps/files_sharing/api/v1";

fn ocs_post(uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
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

fn ocs_put(uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
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
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

// --- POST /shares ----------------------------------------------------------

#[tokio::test]
async fn post_shares_creates_user_share_and_returns_wire_shape() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("s.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(v["ocs"]["data"]["share_with"], "bob");
    assert_eq!(v["ocs"]["data"]["permissions"], 3);
    assert_eq!(v["ocs"]["data"]["share_type"], 0);
    assert_eq!(v["ocs"]["data"]["uid_owner"], "alice");
    assert_eq!(v["ocs"]["data"]["item_type"], "folder");
    assert!(v["ocs"]["data"]["id"].is_string());
    assert_eq!(v["ocs"]["data"]["mail_send"], 0);
}

#[tokio::test]
async fn post_shares_with_link_type_returns_501() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("link.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "X").await;
    let token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &token,
            "path=/X&shareType=3&permissions=1",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

// --- GET /shares -----------------------------------------------------------

#[tokio::test]
async fn get_shares_by_path_lists_outgoing() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("g.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Create.
    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares?path=/X&format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK);
    let arr = v["ocs"]["data"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["share_with"], "bob");
}

#[tokio::test]
async fn get_shares_shared_with_me_lists_incoming_only() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("inc.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let bob_token = issue_bearer(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares?shared_with_me=true&format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK);
    let arr = v["ocs"]["data"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["uid_owner"], "alice");
}

#[tokio::test]
async fn get_shares_subfiles_returns_501() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("sf.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares?subfiles=true&format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn get_share_by_id_visible_to_owner_recipient_404_to_others() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("byid.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_user(&state, "carol").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let bob_token = issue_bearer(&state, "bob").await;
    let carol_token = issue_bearer(&state, "carol").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    // Owner.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Recipient.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/shares/{id}?format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Third party — 404 (not 403, to avoid existence leak).
    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares/{id}?format=json"),
            &carol_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- PUT /shares/{id} ------------------------------------------------------

#[tokio::test]
async fn put_shares_permissions_flip_updates_row() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("put.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .clone()
        .oneshot(ocs_put(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
            "permissions=15",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    assert_eq!(v["ocs"]["data"]["permissions"], 15);
}

#[tokio::test]
async fn put_shares_as_non_owner_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("p403.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let bob_token = issue_bearer(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .oneshot(ocs_put(
            &format!("{BASE}/shares/{id}?format=json"),
            &bob_token,
            "permissions=1",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn put_shares_password_returns_501() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("pwd.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .oneshot(ocs_put(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
            "password=hunter2",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

// --- DELETE /shares/{id} ---------------------------------------------------

#[tokio::test]
async fn delete_shares_owner_removes_row() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("del.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .clone()
        .oneshot(ocs_delete(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_shares_recipient_unaccepts_but_owner_still_sees() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("recd.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let bob_token = issue_bearer(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .clone()
        .oneshot(ocs_delete(
            &format!("{BASE}/shares/{id}?format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Owner's outgoing list still has it.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let arr = v["ocs"]["data"].as_array().unwrap();
    assert_eq!(arr.len(), 1);

    // Recipient's incoming list does not.
    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/shares?shared_with_me=true&format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let arr = v["ocs"]["data"].as_array().unwrap();
    assert!(arr.is_empty());
}

#[tokio::test]
async fn delete_shares_third_party_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("d403.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;
    seed_user(&state, "carol").await;
    seed_folder(&state, "alice", "X").await;
    let alice_token = issue_bearer(&state, "alice").await;
    let carol_token = issue_bearer(&state, "carol").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=3",
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let resp = app
        .oneshot(ocs_delete(
            &format!("{BASE}/shares/{id}?format=json"),
            &carol_token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- DAV-level: share is reachable through the recipient's view ----------
//
// `View::list` (and therefore PROPFIND) doesn't currently enumerate
// child-mount roots — surfacing the share at the recipient's home root in
// PROPFIND will require a View-level fix (out of Batch D's scope). Until
// then this test exercises the next-best property: bob can PUT directly
// into the share path because `View::resolve` routes the request to the
// `ShareMountResolver`-produced mount. Read-only enforcement is verified
// by recreating the share with perms=1 and confirming the PUT is rejected.

#[tokio::test]
async fn dav_put_through_share_mount_honors_recipient_permissions() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("dav.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_user(&state, "bob").await;

    // Materialize alice's home + the `X` folder on disk so the
    // ShareMountResolver can resolve the share's owner path and writes go
    // through real storage.
    let alice_storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    alice_storage
        .mkdir(&StoragePath::new("X").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    seed_folder(&state, "alice", "X").await;

    // bob's home — needed so HomeMountResolver can stat bob's root.
    let _bob_storage = state
        .storage_factory
        .home_storage(&UserId::new("bob").unwrap())
        .await
        .unwrap();

    let alice_token = issue_bearer(&state, "alice").await;
    let bob_token = issue_bearer(&state, "bob").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Read + update + create (7) → bob can create new files inside the share.
    let resp = app
        .clone()
        .oneshot(ocs_post(
            &format!("{BASE}/shares?format=json"),
            &alice_token,
            "path=/X&shareType=0&shareWith=bob&permissions=7",
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK);
    let id: i64 = v["ocs"]["data"]["id"].as_str().unwrap().parse().unwrap();

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/bob/X/new.txt")
        .header("authorization", format!("Bearer {bob_token}"))
        .body(Body::from("hello"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "expected 201/204 for PUT into writable share, got {}",
        resp.status()
    );

    // Flip to read-only (perms=1, bit 1 only) and expect the next PUT to 403.
    let resp = app
        .clone()
        .oneshot(ocs_put(
            &format!("{BASE}/shares/{id}?format=json"),
            &alice_token,
            "permissions=1",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("PUT")
        .uri("/dav/files/bob/X/new2.txt")
        .header("authorization", format!("Bearer {bob_token}"))
        .body(Body::from("nope"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "expected 403 on PUT to read-only share"
    );
}
