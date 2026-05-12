//! Integration tests for Batch E: PROPPATCH.
//!
//! Covers the writable favorite path (set + read back via PROPFIND), the
//! protected-prop rejection path (per-prop 403 in propstat), and the path
//! rewrite that follows a MOVE so a custom prop survives a rename.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // The async filecache scanner races our PUT-then-PROPFIND/PROPPATCH
    // populate path under SQLite. Disabling it forces all writes through
    // the on-demand populate code instead.
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

async fn alice_bearer(state: &AppState) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "DAV",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Seed a file via PUT. Returns the resulting status to make assertions
/// at the call site if needed.
async fn put_file(app: axum::Router, uri: &str, token: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

const SET_FAVORITE_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propertyupdate xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
  <d:set>
    <d:prop>
      <oc:favorite>1</oc:favorite>
    </d:prop>
  </d:set>
</d:propertyupdate>"#;

const SET_PROTECTED_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propertyupdate xmlns:d="DAV:">
  <d:set>
    <d:prop>
      <d:getetag>"forced"</d:getetag>
    </d:prop>
  </d:set>
</d:propertyupdate>"#;

#[tokio::test]
async fn proppatch_sets_oc_favorite_and_propfind_reads_it_back() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ppset.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/fav.txt", &token, "x").await,
        StatusCode::CREATED
    );

    // PROPPATCH sets oc:favorite to "1".
    let req = Request::builder()
        .method("PROPPATCH")
        .uri("/dav/files/alice/fav.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/xml")
        .body(Body::from(SET_FAVORITE_BODY))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert!(
        body.contains("HTTP/1.1 200 OK"),
        "expected 200 OK propstat for writable prop: {body}"
    );
    assert!(
        body.contains("oc:favorite"),
        "expected oc:favorite in echo: {body}"
    );

    // PROPFIND must now reflect favorite=1.
    let pf = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice/fav.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(pf).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert!(
        body.contains("<oc:favorite>1</oc:favorite>"),
        "expected favorite=1 after PROPPATCH, body: {body}"
    );
}

#[tokio::test]
async fn proppatch_protected_prop_returns_403_in_propstat() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ppprot.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/p.txt", &token, "y").await,
        StatusCode::CREATED
    );

    let req = Request::builder()
        .method("PROPPATCH")
        .uri("/dav/files/alice/p.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/xml")
        .body(Body::from(SET_PROTECTED_BODY))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Overall response is 207; the per-prop status is 403.
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert!(
        body.contains("HTTP/1.1 403 Forbidden"),
        "expected per-prop 403 propstat for protected prop: {body}"
    );
    assert!(
        body.contains("d:getetag"),
        "expected echoed prop name in body: {body}"
    );
}

#[tokio::test]
async fn proppatch_paths_follow_move() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ppmove.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    assert_eq!(
        put_file(app.clone(), "/dav/files/alice/before.txt", &token, "z").await,
        StatusCode::CREATED
    );

    // 1) PROPPATCH the source.
    let req = Request::builder()
        .method("PROPPATCH")
        .uri("/dav/files/alice/before.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/xml")
        .body(Body::from(SET_FAVORITE_BODY))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);

    // 2) MOVE before.txt → after.txt.
    let mv = Request::builder()
        .method("MOVE")
        .uri("/dav/files/alice/before.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("destination", "/dav/files/alice/after.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(mv).await.unwrap();
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "move expected 201/204, got {}",
        resp.status()
    );

    // 3) PROPFIND on the new path must show favorite=1.
    let pf = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice/after.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(pf).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert!(
        body.contains("<oc:favorite>1</oc:favorite>"),
        "expected favorite=1 to follow MOVE, body: {body}"
    );

    // The new path must also reflect the favorite was actually copied
    // via PropertyStore::rename_path — not merely surviving because the
    // cache still pointed at the old row. PROPFIND with depth 1 on the
    // parent collection exercises the bulk get_property_many lookup,
    // which keys by the storage path of each child.
    let pf_parent = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(pf_parent).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;
    assert!(
        body.contains("after.txt"),
        "expected after.txt in directory listing: {body}"
    );
    // The favorite=1 prop must appear exactly once — for after.txt.
    // (Before the rename_path call lands, the old path keeps the prop and
    // the new path defaults to 0 → assertion would fail.)
    assert_eq!(
        body.matches("<oc:favorite>1</oc:favorite>").count(),
        1,
        "favorite=1 must appear exactly once after MOVE, body: {body}"
    );
}
