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
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use rustcloud_core::AppStateBuilder;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
        }
    }

    #[tokio::test]
    async fn status_returns_nextcloud_shape() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("status.db"));
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
