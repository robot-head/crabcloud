use axum::body::Body;
use axum::http::{Method, Request};
use rustcloud_config::{CacheConfig, DbType, FileConfig};
use rustcloud_core::AppStateBuilder;
use rustcloud_http::build_router;
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;
use tower::ServiceExt;

fn cfg(path: PathBuf) -> FileConfig {
    FileConfig {
        instanceid: "cors".into(),
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
        trusted_domains: vec!["cloud.example.com".into()],
        trusted_proxies: vec![],
        overwrite_cli_url: None,
        overwrite_protocol: None,
        overwrite_host: None,
        loglevel: "info".into(),
        logfile: None,
        default_language: "en".into(),
        bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        cache: CacheConfig::default(),
        bootstrap_admin: None,
    }
}

#[tokio::test]
async fn cors_allows_both_http_and_https_origins_for_trusted_domain() {
    let dir = tempdir().unwrap();
    let state = AppStateBuilder::new(cfg(dir.path().join("cors.db")))
        .build()
        .await
        .unwrap();
    let app = build_router(state);

    for scheme in ["http", "https"] {
        let origin = format!("{scheme}://cloud.example.com");
        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/status.php")
            .header("origin", &origin)
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let acao = resp.headers().get("access-control-allow-origin");
        assert!(
            acao.is_some(),
            "missing access-control-allow-origin for Origin: {origin}"
        );
        assert_eq!(acao.unwrap().to_str().unwrap(), origin);
    }
}
