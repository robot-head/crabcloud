//! End-to-end tests for the OCS `apps/activity/api/v2/` endpoints.
//!
//! Drives the full `build_router` so requests travel through the real
//! auth + middleware stack (Bearer + `OCS-APIRequest` header — matches
//! how desktop / third-party OCS clients hit the surface). Each test
//! seeds activity rows by calling `ActivityEmitter::emit` directly so
//! the coverage here stays focused on the OCS wire surface; the
//! underlying coalesce / settings semantics are exercised by the
//! crabcloud-activity unit + integration suites.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_activity::{ActivityEmitter, ActivityEvent, EventType, ObjectType};
use crabcloud_core::AppState;
use crabcloud_users::UserId;
use support::{bearer, make_state, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

const BASE: &str = "/ocs/v2.php/apps/activity/api/v2";

async fn make_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> (AppState, String) {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    (state, token)
}

/// Seed one activity row for `recipient` with `actor` + `event_type` +
/// `object_id`. Each call inserts a fresh row because callers vary
/// `object_id` per emit; coalesce probes match on
/// `(recipient, actor, event_type, object_id)` so a distinct
/// `object_id` always sidesteps the 600-second test-config window.
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

fn ocs_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap()
}

fn ocs_put_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
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
async fn list_empty_returns_empty_items() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("ae.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/activity?format=json"), &token))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    let items = v["ocs"]["data"]["items"]
        .as_array()
        .expect("items is array");
    assert!(items.is_empty(), "{v}");
    assert!(v["ocs"]["data"]["next_since"].is_null(), "{v}");
}

#[tokio::test]
async fn list_returns_rows_descending_by_id_with_rendered_subject() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("al.db"), data.path().to_path_buf()).await;
    // Seed 3 rows for alice with monotonically increasing occurred_at +
    // object_id so the page is unambiguously ordered.
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        1,
        1_700_000_001,
    )
    .await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        2,
        1_700_000_002,
    )
    .await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        3,
        1_700_000_003,
    )
    .await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/activity?format=json"), &token))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    let items = v["ocs"]["data"]["items"]
        .as_array()
        .expect("items is array");
    assert_eq!(items.len(), 3, "{v}");
    // Descending id order — newest first.
    let ids: Vec<i64> = items.iter().map(|it| it["id"].as_i64().unwrap()).collect();
    assert!(
        ids[0] > ids[1] && ids[1] > ids[2],
        "ids not descending: {ids:?}"
    );
    // Rendered `subject` is alongside the raw subject_id / params. The
    // actor row uses `file_updated_you` → "You updated {file}".
    assert_eq!(items[0]["subject_id"], "file_updated_you");
    assert_eq!(items[0]["subject"], "You updated /file3.txt");
    assert_eq!(items[0]["event_type"], "file_updated");
    assert_eq!(items[0]["object_type"], "file");
    assert_eq!(items[0]["object_id"], 3);
    // next_since == id of the oldest row in the page (smallest id when
    // sorted descending).
    assert_eq!(v["ocs"]["data"]["next_since"], ids[2]);
}

#[tokio::test]
async fn list_with_since_and_limit_returns_one_older_row() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("ap.db"), data.path().to_path_buf()).await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        1,
        1_700_000_001,
    )
    .await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        2,
        1_700_000_002,
    )
    .await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        3,
        1_700_000_003,
    )
    .await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // First grab the freshest page to discover the ids.
    let resp = app
        .clone()
        .oneshot(ocs_get(&format!("{BASE}/activity?format=json"), &token))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let ids: Vec<i64> = v["ocs"]["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["id"].as_i64().unwrap())
        .collect();
    // Use the middle id as `since`; expect exactly one row with id < since
    // (the oldest of the three) because limit=1.
    let since = ids[1];
    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/activity?since={since}&limit=1&format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let items = v["ocs"]["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    let id = items[0]["id"].as_i64().unwrap();
    assert!(id < since, "id {id} should be < since {since}");
    assert_eq!(v["ocs"]["data"]["next_since"], id);
}

#[tokio::test]
async fn list_isolates_per_user() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, alice_token) =
        make_alice(dir.path().join("ai.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let bob_token = bearer(&state, "bob").await;
    // Two rows for alice, one for bob.
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        1,
        1_700_000_001,
    )
    .await;
    emit_one(
        &state,
        "alice",
        "alice",
        EventType::FileUpdated,
        2,
        1_700_000_002,
    )
    .await;
    emit_one(
        &state,
        "bob",
        "bob",
        EventType::FileUpdated,
        99,
        1_700_000_003,
    )
    .await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/activity?format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let items = v["ocs"]["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "alice sees only her rows: {v}");

    let resp = app
        .oneshot(ocs_get(&format!("{BASE}/activity?format=json"), &bob_token))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let items = v["ocs"]["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "bob sees only his row: {v}");
    assert_eq!(items[0]["object_id"], 99);
}

#[tokio::test]
async fn settings_get_empty_then_upsert_visible() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("as.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // GET — no rows yet, settings array is empty (missing rows default
    // to stream=true in the service).
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/activity/settings?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let settings = v["ocs"]["data"]["settings"].as_array().unwrap();
    assert!(settings.is_empty(), "{v}");

    // PUT — turn off the stream for `file_updated`.
    let resp = app
        .clone()
        .oneshot(ocs_put_json(
            &format!("{BASE}/activity/settings?format=json"),
            &token,
            serde_json::json!({ "event_type": "file_updated", "stream": false }),
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);

    // GET again — the upsert is visible.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/activity/settings?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let settings = v["ocs"]["data"]["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 1, "{v}");
    assert_eq!(settings[0]["event_type"], "file_updated");
    assert_eq!(settings[0]["stream"], false);

    // PUT again — flip back to true, re-GET sees the new value (not a
    // duplicate row).
    let resp = app
        .clone()
        .oneshot(ocs_put_json(
            &format!("{BASE}/activity/settings?format=json"),
            &token,
            serde_json::json!({ "event_type": "file_updated", "stream": true }),
        ))
        .await
        .unwrap();
    let (status, _) = decode(resp).await;
    assert_eq!(status, StatusCode::OK);

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/activity/settings?format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let settings = v["ocs"]["data"]["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 1, "upsert (not insert): {v}");
    assert_eq!(settings[0]["stream"], true);
}
