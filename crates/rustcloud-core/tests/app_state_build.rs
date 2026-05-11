//! End-to-end assembly proof for `AppStateBuilder`.

use rustcloud_config::{CacheConfig, DbType, FileConfig};
use rustcloud_core::{AppState, AppStateBuilder};
use rustcloud_i18n::Locale;
use rustcloud_ocs::{aggregate, CapabilityContext, CoreCapabilities};
use secrecy::SecretString;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

fn cfg_sqlite(path: PathBuf) -> FileConfig {
    FileConfig {
        instanceid: "it".into(),
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

fn seed_de_po(root: &std::path::Path) {
    let app = root.join("core");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("de.po"),
        r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Welcome"
msgstr "Willkommen"
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn full_assembly_works_end_to_end() {
    let dir = tempdir().unwrap();
    let l10n_dir = dir.path().join("l10n");
    seed_de_po(&l10n_dir);

    let cfg = cfg_sqlite(dir.path().join("it.db"));

    // Hook writes a sentinel that future tests can rely on. `boxed_hook`
    // wraps an async closure into the `BootstrapHook` shape.
    let hook = rustcloud_core::boxed_hook(|state: AppState| async move {
        state.appconfig.set("core", "phase2_built", "1").await?;
        Ok(())
    });

    let state = AppStateBuilder::new(cfg)
        .with_catalog_root(&l10n_dir)
        .with_hook(hook)
        .build()
        .await
        .unwrap();

    // DbPool is connected and migrations applied — appconfig works.
    assert_eq!(
        state.appconfig.get("core", "phase2_built").await.unwrap(),
        Some("1".into())
    );

    // i18n catalogs loaded; lookup hits German translation.
    let de = Locale::new("de");
    let s = state.i18n.t("core", &de, "Welcome", &[]);
    assert_eq!(s, "Willkommen");
    let s_fallback = state.i18n.t("core", &de, "Bye", &[]);
    assert_eq!(s_fallback, "Bye"); // fallback to source

    // Capability provider registration + aggregator end-to-end.
    state
        .register_capability_provider(Arc::new(CoreCapabilities::default()))
        .await;
    let providers = state.capability_providers.lock().await.clone();
    let payload = aggregate(
        &providers,
        &CapabilityContext::default(),
        state.cache.clone(),
        &state.config.versionstring,
        &state.config.instanceid,
    )
    .await
    .unwrap();
    assert_eq!(payload.body["capabilities"]["core"]["pollinterval"], 60);

    // Cache is shared and writable.
    state.cache.set("smoke", b"ok", None).await.unwrap();
    assert_eq!(
        state.cache.get("smoke").await.unwrap(),
        Some(b"ok".to_vec())
    );
}
