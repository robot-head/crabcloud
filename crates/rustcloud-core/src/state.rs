//! `AppState` — the clone-cheap composition handle.

use crate::appconfig::AppConfigService;
use crate::bootstrap::BootstrapRegistry;
use crate::error::{CoreResult, Error};
use rustcloud_cache::{Cache, MemoryCache};
use rustcloud_config::FileConfig;
use rustcloud_db::{core_set, DbPool, MigrationRunner};
use rustcloud_i18n::I18n;
use rustcloud_ocs::CapabilityProvider;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Application-wide composition handle. All fields are `Arc`- or `Clone`-backed
/// so cloning is cheap.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<FileConfig>,
    pub pool: DbPool,
    pub cache: Arc<dyn Cache>,
    pub i18n: Arc<I18n>,
    pub appconfig: AppConfigService,
    pub capability_providers: Arc<Mutex<Vec<Arc<dyn CapabilityProvider>>>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("instance_id", &self.config.instanceid)
            .field("dbtype", &self.config.dbtype.as_str())
            .finish()
    }
}

impl AppState {
    /// Convenience: register a capability provider at runtime.
    pub async fn register_capability_provider(&self, p: Arc<dyn CapabilityProvider>) {
        self.capability_providers.lock().await.push(p);
    }
}

/// Builder that loads / connects everything and produces an `AppState`.
pub struct AppStateBuilder {
    config: Arc<FileConfig>,
    catalog_root: Option<std::path::PathBuf>,
    cache: Option<Arc<dyn Cache>>,
    registry: BootstrapRegistry,
}

impl AppStateBuilder {
    pub fn new(config: FileConfig) -> Self {
        Self {
            config: Arc::new(config),
            catalog_root: None,
            cache: None,
            registry: BootstrapRegistry::new(),
        }
    }

    pub fn with_catalog_root(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.catalog_root = Some(p.into());
        self
    }

    pub fn with_cache(mut self, c: Arc<dyn Cache>) -> Self {
        self.cache = Some(c);
        self
    }

    pub fn with_hook(mut self, hook: crate::bootstrap::BootstrapHook) -> Self {
        self.registry.register(hook);
        self
    }

    /// Register the default `CoreCapabilities` provider on bootstrap so the
    /// `core` namespace is non-empty at the `/ocs/.../capabilities` route.
    pub fn with_core_capabilities(self) -> Self {
        use rustcloud_ocs::CoreCapabilities;
        let core = std::sync::Arc::new(CoreCapabilities::default());
        self.with_hook(crate::bootstrap::boxed_hook(move |state| async move {
            state.register_capability_provider(core).await;
            Ok(())
        }))
    }

    /// Build the `AppState`:
    /// 1. Connect the DB pool.
    /// 2. Run core migrations.
    /// 3. Load i18n catalogs (no-op if `catalog_root` unset or missing).
    /// 4. Construct cache (default: `MemoryCache`).
    /// 5. Construct `AppConfigService`.
    /// 6. Run registered hooks (each gets a cheap `AppState` clone).
    pub async fn build(mut self) -> CoreResult<AppState> {
        let pool = DbPool::connect(&self.config).await?;

        let mut runner = MigrationRunner::new(&pool, &self.config.dbtableprefix);
        runner.register(core_set());
        runner.run().await?;

        let i18n = match &self.catalog_root {
            Some(root) => {
                let catalogs = rustcloud_i18n::load_all(root)
                    .map_err(|e| Error::Internal(anyhow::anyhow!("i18n load: {e}")))?;
                Arc::new(I18n::new(
                    catalogs,
                    rustcloud_i18n::Locale::new(&self.config.default_language),
                ))
            }
            None => Arc::new(I18n::new(
                std::collections::HashMap::new(),
                rustcloud_i18n::Locale::new(&self.config.default_language),
            )),
        };

        let cache = self.cache.unwrap_or_else(|| Arc::new(MemoryCache::new()));

        let appconfig = AppConfigService::new(
            pool.clone(),
            cache.clone(),
            &self.config.dbtableprefix,
            &self.config.instanceid,
        );

        let state = AppState {
            config: self.config.clone(),
            pool,
            cache,
            i18n,
            appconfig,
            capability_providers: Arc::new(Mutex::new(Vec::new())),
        };

        self.registry.run(&state).await?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_config::test_support::minimal_sqlite_config;
    use tempfile::tempdir;

    #[tokio::test]
    async fn build_assembles_state_from_minimal_config() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("state.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        assert_eq!(state.config.instanceid, "test");
        assert_eq!(state.pool.dialect(), "sqlite");
        assert!(state.i18n.available_locales().is_empty());
        // appconfig should be usable.
        state.appconfig.set("test", "k", "v").await.unwrap();
        assert_eq!(
            state.appconfig.get("test", "k").await.unwrap(),
            Some("v".into())
        );
    }

    #[tokio::test]
    async fn build_runs_registered_hooks() {
        use crate::boxed_hook;
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("state.db"));
        // Hook receives an owned AppState clone and writes a sentinel.
        let hook = boxed_hook(|state: AppState| async move {
            state.appconfig.set("core", "bootstrapped", "yes").await?;
            Ok(())
        });
        let state = AppStateBuilder::new(cfg)
            .with_hook(hook)
            .build()
            .await
            .unwrap();
        assert_eq!(
            state.appconfig.get("core", "bootstrapped").await.unwrap(),
            Some("yes".to_string())
        );
    }

    #[tokio::test]
    async fn with_core_capabilities_registers_the_provider() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg)
            .with_core_capabilities()
            .build()
            .await
            .unwrap();
        let guard = state.capability_providers.lock().await;
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].namespace(), "core");
    }

    #[tokio::test]
    async fn register_capability_provider_appends() {
        use rustcloud_ocs::CoreCapabilities;
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("state.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        state
            .register_capability_provider(Arc::new(CoreCapabilities::default()))
            .await;
        let guard = state.capability_providers.lock().await;
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].namespace(), "core");
    }
}
