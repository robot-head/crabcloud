//! End-to-end tests for the OCS `/search/providers/files/search` endpoint.
//!
//! Drives the full `build_router` so requests travel through the real
//! auth + middleware stack (Bearer + `OCS-APIRequest` header — matches
//! how third-party OCS clients hit the surface). Each test seeds rows
//! by calling `Search::upsert_for_file` directly so coverage stays
//! focused on the OCS wire surface; the underlying query / parser
//! semantics are exercised by the crabcloud-search unit + integration
//! suites.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use support::{bearer, make_state, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

const BASE: &str = "/ocs/v2.php/search/providers/files";

async fn make_alice(db: std::path::PathBuf, data: std::path::PathBuf) -> (AppState, String) {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    let token = bearer(&state, "alice").await;
    (state, token)
}

/// Seed one search row for `viewer` against fileid + basename + mime +
/// path. Uses `Search::upsert_for_file` directly because the
/// `SearchIndexer` background task is the cross-crate integration path
/// and would require materialising filecache rows + driving storage
/// events through the sink — the OCS wire surface only cares about
/// what the service emits.
async fn seed_search_row(
    state: &AppState,
    viewer: &str,
    fileid: i64,
    basename: &str,
    path: &str,
    mime: &str,
    size: i64,
) {
    state
        .search
        .upsert_for_file(
            viewer,
            fileid,
            "local::test",
            basename,
            path,
            mime,
            1_700_000_000,
            size,
        )
        .await
        .expect("upsert");
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

async fn decode(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn empty_query_returns_empty_entries_and_is_last() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("se.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=&format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 200);
    let entries = v["ocs"]["data"]["entries"]
        .as_array()
        .expect("entries is array");
    assert!(entries.is_empty(), "{v}");
    assert!(v["ocs"]["data"]["cursor"].is_null(), "{v}");
    assert_eq!(v["ocs"]["data"]["isLast"], true);
    assert_eq!(v["ocs"]["data"]["name"], "Files");
    assert_eq!(v["ocs"]["data"]["isPaginated"], true);
}

#[tokio::test]
async fn matching_query_returns_entries_in_unified_search_shape() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("sm.db"), data.path().to_path_buf()).await;

    seed_search_row(
        &state,
        "alice",
        1,
        "alpha.txt",
        "/docs/alpha.txt",
        "text/plain",
        100,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        2,
        "alpha-notes.txt",
        "/notes/alpha-notes.txt",
        "text/plain",
        200,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        3,
        "beta.txt",
        "/docs/beta.txt",
        "text/plain",
        300,
    )
    .await;

    let app = crabcloud_http::build_router(state, axum::Router::new());
    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=alpha&format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let entries = v["ocs"]["data"]["entries"]
        .as_array()
        .expect("entries is array");
    assert_eq!(entries.len(), 2, "two rows match `alpha`: {v}");

    // Shape: title = basename, subline = path, resourceUrl = /files{path},
    // attributes carry stringified fileid / mime / size / mtime.
    let titles: Vec<&str> = entries
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"alpha.txt"));
    assert!(titles.contains(&"alpha-notes.txt"));
    for e in entries {
        let title = e["title"].as_str().unwrap();
        let subline = e["subline"].as_str().unwrap();
        assert!(
            subline.ends_with(title),
            "subline {subline} should end with title {title}"
        );
        assert_eq!(
            e["resourceUrl"].as_str().unwrap(),
            format!("/files{subline}")
        );
        assert_eq!(e["rounded"], false);
        // Numeric attributes are stringified to match Nextcloud's wire.
        assert!(e["attributes"]["fileid"].is_string());
        assert!(e["attributes"]["mime"].is_string());
        assert!(e["attributes"]["size"].is_string());
        assert!(e["attributes"]["mtime"].is_string());
    }

    // With 2 entries returned under the default limit of 20, isLast is
    // true and cursor is present (carries the last hit's rank+fileid).
    assert_eq!(v["ocs"]["data"]["isLast"], true);
    assert!(v["ocs"]["data"]["cursor"].is_string(), "{v}");
}

#[tokio::test]
async fn limit_one_paginates_via_cursor() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("sp.db"), data.path().to_path_buf()).await;

    // Seed 3 rows that all match `report`. With limit=1 we expect three
    // pages: hit / hit / empty (or isLast on the second page).
    seed_search_row(
        &state,
        "alice",
        10,
        "report-one.txt",
        "/docs/report-one.txt",
        "text/plain",
        100,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        20,
        "report-two.txt",
        "/docs/report-two.txt",
        "text/plain",
        200,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        30,
        "report-three.txt",
        "/docs/report-three.txt",
        "text/plain",
        300,
    )
    .await;

    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Page 1.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=report&limit=1&format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let entries = v["ocs"]["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "page1 has 1 entry: {v}");
    assert_eq!(v["ocs"]["data"]["isLast"], false, "page1 not last: {v}");
    let cursor_p1 = v["ocs"]["data"]["cursor"]
        .as_str()
        .expect("page1 cursor")
        .to_string();
    let page1_fileid = entries[0]["attributes"]["fileid"].as_str().unwrap();

    // Page 2 — passes the cursor from page 1, expects a different row.
    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!(
                "{BASE}/search?query=report&limit=1&cursor={}&format=json",
                urlencoding::encode(&cursor_p1)
            ),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let entries = v["ocs"]["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "page2 has 1 entry: {v}");
    let page2_fileid = entries[0]["attributes"]["fileid"].as_str().unwrap();
    assert_ne!(
        page1_fileid, page2_fileid,
        "page2 fileid differs from page1: {page1_fileid} vs {page2_fileid}"
    );
}

#[tokio::test]
async fn mime_filter_narrows_results() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("sf.db"), data.path().to_path_buf()).await;

    // Two rows match `vacation`: one image/jpeg, one text/plain. The
    // `mime:image/*` filter should isolate the image.
    seed_search_row(
        &state,
        "alice",
        100,
        "vacation.jpg",
        "/pics/vacation.jpg",
        "image/jpeg",
        50_000,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        101,
        "vacation.txt",
        "/notes/vacation.txt",
        "text/plain",
        500,
    )
    .await;

    let app = crabcloud_http::build_router(state, axum::Router::new());
    let resp = app
        .oneshot(ocs_get(
            &format!(
                "{BASE}/search?query={}&format=json",
                urlencoding::encode("vacation mime:image/*")
            ),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::OK, "body={v}");
    let entries = v["ocs"]["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "only the image matches: {v}");
    assert_eq!(entries[0]["attributes"]["mime"], "image/jpeg");
    assert_eq!(entries[0]["title"], "vacation.jpg");
}

#[tokio::test]
async fn bad_cursor_returns_400_envelope() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, token) = make_alice(dir.path().join("sb.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=hi&cursor=not-base64!!&format=json"),
            &token,
        ))
        .await
        .unwrap();
    let (status, v) = decode(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={v}");
    assert_eq!(v["ocs"]["meta"]["statuscode"], 400);
}

#[tokio::test]
async fn isolates_per_user() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let (state, alice_token) =
        make_alice(dir.path().join("si.db"), data.path().to_path_buf()).await;
    seed_user(&state, "bob").await;
    let bob_token = bearer(&state, "bob").await;

    // Two rows for alice, one for bob — all match `secret`.
    seed_search_row(
        &state,
        "alice",
        201,
        "alice-secret.txt",
        "/docs/alice-secret.txt",
        "text/plain",
        100,
    )
    .await;
    seed_search_row(
        &state,
        "alice",
        202,
        "alice-secret-2.txt",
        "/docs/alice-secret-2.txt",
        "text/plain",
        100,
    )
    .await;
    seed_search_row(
        &state,
        "bob",
        301,
        "bob-secret.txt",
        "/from-alice/bob-secret.txt",
        "text/plain",
        100,
    )
    .await;

    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .clone()
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=secret&format=json"),
            &alice_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let entries = v["ocs"]["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "alice sees only her rows: {v}");

    let resp = app
        .oneshot(ocs_get(
            &format!("{BASE}/search?query=secret&format=json"),
            &bob_token,
        ))
        .await
        .unwrap();
    let (_, v) = decode(resp).await;
    let entries = v["ocs"]["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "bob sees only his row: {v}");
    assert_eq!(entries[0]["title"], "bob-secret.txt");
}
