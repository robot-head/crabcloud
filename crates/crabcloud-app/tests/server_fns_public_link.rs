//! HTTP-level regression test for the public-link viewer server fns.
//!
//! Reproduces a bug where `meta_public_link` / `list_public_link` (mounted
//! under `/api/...` by the dx fullstack router) blew up with
//! `"public_link_context_missing"` because the `public_link_auth`
//! middleware only runs on `/s/{token}` and `/public.php/dav/files/...`.
//! The viewer at `/s/{token}` renders SSR then the hydrated client calls
//! `/api/public_link/meta` directly (token in JSON body — see encoding
//! note below) — that call never hit the middleware, so no
//! `PublicLinkAuthContext` extension was present.
//!
//! The fix is for the server fns to self-resolve the context from
//! `AppState.publiclinks_auth`, the token (received as a fn arg, decoded
//! from the request body by the dx fullstack macro), and the cookie
//! header(s). This test would fail on master and passes on the fix
//! branch.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_publiclinks::UnlockCookie;
use crabcloud_sharing::{CreateShareRequest, ShareRow, ShareType};
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
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

fn build_app(state: AppState) -> axum::Router {
    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    crabcloud_http::build_router(state, dioxus_router)
}

/// Seed a /Photos folder owned by alice in the filecache, then create a
/// public link share against it. Returns the resulting `ShareRow` (whose
/// `token` is what the viewer page passes to `/api/public_link/meta`).
async fn seed_link_share(state: &AppState, password: Option<&str>) -> ShareRow {
    let alice_storage = state
        .storage_factory
        .home_storage(&UserId::new("alice").unwrap())
        .await
        .unwrap();
    let home_sid = alice_storage.id().to_string();
    // Materialise /Photos on the underlying storage so the filecache stat
    // succeeds (Shares::create_link looks the path up in the cache).
    let photos = crabcloud_storage::StoragePath::new("Photos").unwrap();
    alice_storage
        .mkdir(&photos, state.storage_sink.as_ref())
        .await
        .unwrap();
    state.filecache.stat(&alice_storage, &photos).await.unwrap();
    state
        .shares
        .create(CreateShareRequest {
            requester: "alice".into(),
            home_storage_id: home_sid,
            path: "/Photos".into(),
            share_type: ShareType::Link,
            // Plain Link shares ignore `share_with`; pass empty.
            share_with: String::new(),
            // Read-only.
            permissions: 1,
            password: password.map(|s| s.to_string()),
            expire_date: None,
        })
        .await
        .expect("link share creation")
}

// Encoding note: the dx 0.7 fullstack macro for
// `#[get("/api/public_link/meta")] async fn meta_public_link(token: String)`
// does NOT put `token` in the URL query string — the route literal has no
// `?token` placeholder, so the function arg falls through to the JSON
// "body bucket". The generated client therefore sends `GET
// /api/public_link/meta` with `Content-Type: application/json` and body
// `{"token":"..."}` (see `dioxus-fullstack-macro-0.7.9/src/lib.rs`
// `remaining_pattypes_named` + `body_struct_impl`, and
// `dioxus-fullstack-0.7.9/src/magic.rs::EncodeRequest::fetch_client` which
// calls `ctx.send_json(&data)` for `Serialize + DeserializeOwned` args).
// These tests deliberately mirror that exact wire shape — body-on-GET
// looks suspicious but is what production hits.

#[tokio::test]
async fn meta_public_link_resolves_context_without_middleware() {
    // Regression: this exact call path bypasses the `/s/{token}` nest
    // entirely. On master it returns 500 with body containing
    // `public_link_context_missing`.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_alice(dir.path().join("m.db"), data.path().to_path_buf()).await;
    let row = seed_link_share(&state, None).await;
    let token = row.token.expect("link share carries token");
    let app = build_app(state);

    let req = Request::builder()
        .method("GET")
        .uri("/api/public_link/meta")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "token": token }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200 from /api/public_link/meta, got {} body={:?}",
        status,
        String::from_utf8_lossy(&body)
    );
    let meta: crabcloud_app::PublicLinkMeta = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "decode PublicLinkMeta: {e} body={:?}",
            String::from_utf8_lossy(&body)
        )
    });
    assert!(meta.can_read, "read permission expected: {meta:?}");
    assert!(!meta.password_required, "no password on this share");
    assert_eq!(meta.root_name, "Photos");
}

#[tokio::test]
async fn meta_public_link_password_required_when_no_cookie() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_alice(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let row = seed_link_share(&state, Some("hunter2")).await;
    let token = row.token.expect("link share carries token");
    let app = build_app(state);

    let req = Request::builder()
        .method("GET")
        .uri("/api/public_link/meta")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "token": token }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let meta: crabcloud_app::PublicLinkMeta = serde_json::from_slice(&body).unwrap();
    assert!(
        meta.password_required,
        "expected password gate flagged: {meta:?}"
    );
}

#[tokio::test]
async fn meta_public_link_password_satisfied_by_valid_cookie() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_alice(dir.path().join("c.db"), data.path().to_path_buf()).await;
    let row = seed_link_share(&state, Some("hunter2")).await;
    let token = row.token.expect("link share carries token");

    // Build a valid unlock cookie matching the auth state's secret.
    let secret = state.publiclinks_auth.secret.clone();
    let exp = chrono::Utc::now().timestamp() + 3600;
    let cookie_value = UnlockCookie::sign(&secret, &token, exp);
    let cookie_name = UnlockCookie::cookie_name_for(&token);

    let app = build_app(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/public_link/meta")
        .header("cookie", format!("{cookie_name}={cookie_value}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "token": token }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let meta: crabcloud_app::PublicLinkMeta = serde_json::from_slice(&body).unwrap();
    assert!(
        !meta.password_required,
        "cookie should satisfy the gate: {meta:?}"
    );
    assert!(meta.can_read);
}
