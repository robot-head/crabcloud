//! End-to-end Phase 3 HTTP flow: /status.php → capabilities → login → use session.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use rustcloud_config::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
use rustcloud_core::AppStateBuilder;
use rustcloud_http::build_router;
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;
use tower::ServiceExt;

fn cfg(path: PathBuf, hash: &str) -> FileConfig {
    FileConfig {
        instanceid: "e2e".into(),
        secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
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
        datadirectory: PathBuf::from("/tmp"),
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
        bootstrap_admin: Some(BootstrapAdminConfig {
            username: "admin".into(),
            password_hash: hash.into(),
        }),
    }
}

#[tokio::test]
async fn phase3_full_flow() {
    let dir = tempdir().unwrap();
    let hash = bcrypt::hash("hunter2", bcrypt::DEFAULT_COST).unwrap();
    let state = AppStateBuilder::new(cfg(dir.path().join("e2e.db"), &hash))
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let app = build_router(state);

    // 1. status.php returns Nextcloud shape.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/status.php")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 8192).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["productname"], "Nextcloud");
    assert_eq!(parsed["version"], "31.0.0.0");

    // 2. capabilities returns the core namespace.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 32 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(
        parsed["ocs"]["data"]["capabilities"]["core"]["pollinterval"],
        60
    );
    assert_eq!(parsed["ocs"]["data"]["version"]["major"], 31);

    // 3. login with correct creds → 303 + Set-Cookie.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index.php/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let setc = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cookie = setc.split(';').next().unwrap().to_string();
    assert!(cookie.starts_with("oc_sessionPassphrase="));

    // 4. login with wrong creds → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index.php/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=WRONG"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 5. capabilities again with cookie → still 200 (auth-optional route).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 6. Security headers present on status.php.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/status.php")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let h = resp.headers();
    assert!(h.get("strict-transport-security").is_some(), "HSTS missing");
    assert!(h.get("x-content-type-options").is_some(), "XCTO missing");
    assert!(h.get("content-security-policy").is_some(), "CSP missing");
}
