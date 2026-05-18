//! HTTP-level integration tests for the activity server fns
//! (`/api/files/activity/{list,settings,settings/put}`). Mirrors the
//! `server_fns_versions.rs` scaffold — drives the full `build_router`
//! stack so requests travel through the production auth middleware
//! and the dx fullstack server-fn handler.
//!
//! Activity rows are seeded by calling the `ActivityEmitter` directly
//! (same pattern as the OCS `ocs_activity.rs` suite). The wider
//! emit-hook + coalesce semantics are exercised by the
//! `crabcloud-activity` unit / e2e suites and the per-emitter crate
//! tests — these tests stay focused on the server-fn wire surface.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_activity::{ActivityEmitter, ActivityEvent, EventType, ObjectType};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
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

async fn bearer_for(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "UI",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

fn build_app(state: AppState) -> axum::Router {
    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    crabcloud_http::build_router(state, dioxus_router)
}

/// Seed one activity row for `recipient` with `actor` + `event_type` +
/// `object_id`. Each call varies `object_id` so successive emits
/// sidestep the 600-second coalesce window and produce distinct rows.
async fn emit_one(
    state: &AppState,
    recipient: &str,
    actor: &str,
    event_type: EventType,
    object_id: i64,
    occurred_at: i64,
) {
    state
        .activity
        .emit(ActivityEvent {
            actor: actor.to_string(),
            event_type,
            subject_id_actor: "file_updated_you".to_string(),
            subject_id_recipient: "file_updated_by".to_string(),
            subject_params: serde_json::json!({
                "actor": actor,
                "file": format!("/file{object_id}.txt"),
            }),
            object_type: ObjectType::File,
            object_id: Some(object_id),
            recipients: vec![UserId::new(recipient).unwrap()],
            occurred_at,
        })
        .await
        .expect("emit");
}

async fn post_json(
    app: &axum::Router,
    token: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn decode_bytes(resp: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn list_activity_empty_returns_empty_items() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("e.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/list",
        serde_json::json!({}),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let resp: crabcloud_app::ListActivityResponse =
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "decode ListActivityResponse: {e} body={:?}",
                String::from_utf8_lossy(&body)
            )
        });
    assert!(resp.items.is_empty(), "{:?}", resp.items);
    assert!(resp.next_since.is_none(), "{:?}", resp.next_since);
}

#[tokio::test]
async fn list_activity_returns_seeded_rows_descending_with_rendered_subject() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    // Two rows on Alice's feed — actor "alice" (own action) and a
    // different actor "bob" (so subject_id_recipient is what renders).
    emit_one(&state, "alice", "alice", EventType::FileUpdated, 1, 1_000).await;
    emit_one(&state, "alice", "bob", EventType::FileUpdated, 2, 2_000).await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/list",
        serde_json::json!({}),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let resp: crabcloud_app::ListActivityResponse =
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "decode ListActivityResponse: {e} body={:?}",
                String::from_utf8_lossy(&body)
            )
        });
    assert_eq!(resp.items.len(), 2, "{:?}", resp.items);
    // Descending by id — the bob-actor row was inserted second so it
    // surfaces first.
    assert_eq!(resp.items[0].actor, "bob");
    assert_eq!(resp.items[0].event_type, "file_updated");
    assert_eq!(resp.items[0].subject_id, "file_updated_by");
    assert_eq!(resp.items[0].subject, "bob updated /file2.txt");
    assert_eq!(resp.items[0].count, 1);
    assert_eq!(resp.items[0].occurred_at, 2_000);
    assert_eq!(resp.items[0].object_type, "file");
    assert_eq!(resp.items[0].object_id, Some(2));
    // Alice's own row uses subject_id_actor.
    assert_eq!(resp.items[1].actor, "alice");
    assert_eq!(resp.items[1].subject_id, "file_updated_you");
    assert_eq!(resp.items[1].subject, "You updated /file1.txt");
    // next_since is the smallest id on the page — the alice row.
    let smallest = resp.items[1].id;
    assert_eq!(resp.next_since, Some(smallest));
}

#[tokio::test]
async fn list_activity_respects_since_and_limit() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    for i in 0..5 {
        emit_one(
            &state,
            "alice",
            "alice",
            EventType::FileUpdated,
            100 + i,
            1_000 + i,
        )
        .await;
    }
    let app = build_app(state);

    // Page 1: limit 2, no since → top 2 (largest ids).
    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/list",
        serde_json::json!({ "limit": 2 }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let page1: crabcloud_app::ListActivityResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page1.items.len(), 2);
    let cursor = page1.next_since.expect("next_since on a full page");
    // ids are descending — page1[0].id > page1[1].id == cursor.
    assert!(page1.items[0].id > page1.items[1].id);
    assert_eq!(page1.items[1].id, cursor);

    // Page 2: since = cursor → strictly less than cursor.
    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/list",
        serde_json::json!({ "since": cursor, "limit": 10 }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let page2: crabcloud_app::ListActivityResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page2.items.len(), 3, "{:?}", page2.items);
    for it in &page2.items {
        assert!(
            it.id < cursor,
            "id {} not strictly less than {cursor}",
            it.id
        );
    }
}

#[tokio::test]
async fn settings_round_trip_set_then_get() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("s.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    let app = build_app(state.clone());

    // Empty initial state.
    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/settings",
        serde_json::json!({}),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let settings: Vec<crabcloud_app::ActivitySettingDto> = serde_json::from_slice(&body).unwrap();
    assert!(settings.is_empty(), "{settings:?}");

    // Upsert two toggles via the PUT-equivalent server fn.
    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/settings/put",
        serde_json::json!({ "event_type": "file_updated", "stream": false }),
    )
    .await;
    assert_eq!(decode_bytes(resp).await.0, StatusCode::OK);

    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/settings/put",
        serde_json::json!({ "event_type": "share_created", "stream": true }),
    )
    .await;
    assert_eq!(decode_bytes(resp).await.0, StatusCode::OK);

    // Read them back. Order isn't part of the contract — match by event_type.
    let resp = post_json(
        &app,
        &token,
        "/api/files/activity/settings",
        serde_json::json!({}),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let settings: Vec<crabcloud_app::ActivitySettingDto> = serde_json::from_slice(&body).unwrap();
    assert_eq!(settings.len(), 2, "{settings:?}");
    let by_type: std::collections::HashMap<_, _> = settings
        .iter()
        .map(|s| (s.event_type.as_str(), s.stream))
        .collect();
    assert_eq!(by_type.get("file_updated"), Some(&false));
    assert_eq!(by_type.get("share_created"), Some(&true));

    // The underlying service should reflect the write too.
    let stored = state
        .activity_settings
        .get_all_for_user("alice")
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
}

#[tokio::test]
async fn list_activity_unauthenticated_returns_non_ok() {
    // Same contract as `server_fns_versions`: AuthLayer only 401s when
    // an auth header is present-but-invalid. With no auth at all the
    // request falls through anonymous; the server fn body returns
    // `unauthorized` (mapped to 500). Either way it's not 200.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/activity/list")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_activity_settings_unauthenticated_returns_non_ok() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u2.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/activity/settings")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}
