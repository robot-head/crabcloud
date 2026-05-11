//! `GET /status.php` — Nextcloud-compatible probe used by clients to identify
//! the server and decide whether to keep talking to it.
//!
//! See spec §7.7.

use axum::extract::State;
use axum::response::Json;
use rustcloud_core::AppState;
use serde::Serialize;

#[derive(Serialize)]
pub struct StatusResponse {
    pub installed: bool,
    pub maintenance: bool,
    #[serde(rename = "needsDbUpgrade")]
    pub needs_db_upgrade: bool,
    pub version: String,
    pub versionstring: String,
    pub edition: String,
    pub productname: String,
    #[serde(rename = "extendedSupport")]
    pub extended_support: bool,
}

pub async fn handler(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        installed: state.config.installed,
        maintenance: false,
        needs_db_upgrade: false,
        version: state.config.version.clone(),
        versionstring: state.config.versionstring.clone(),
        edition: String::new(),
        productname: "Nextcloud".to_string(),
        extended_support: false,
    })
}

#[cfg(test)]
mod tests {
    use crate::router::build_router;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use rustcloud_config::test_support::minimal_sqlite_config;
    use rustcloud_core::AppStateBuilder;
    use tempfile::tempdir;
    use tower::ServiceExt;

    #[tokio::test]
    async fn status_returns_nextcloud_shape() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("status.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let app = build_router(state);

        let req = Request::builder()
            .uri("/status.php")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["installed"], true);
        assert_eq!(parsed["maintenance"], false);
        assert_eq!(parsed["needsDbUpgrade"], false);
        assert_eq!(parsed["version"], "31.0.0.0");
        assert_eq!(parsed["versionstring"], "31.0.0");
        assert_eq!(parsed["productname"], "Nextcloud");
        assert_eq!(parsed["extendedSupport"], false);
    }
}
