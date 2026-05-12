//! `AppState` — the clone-cheap composition handle.

use crate::appconfig::AppConfigService;
use crate::bootstrap::BootstrapRegistry;
use crate::error::{CoreResult, Error};
use crabcloud_cache::{Cache, MemoryCache};
use crabcloud_config::FileConfig;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_fs::{HomeMountResolver, LocalStorageFactory, MountResolver, Uploads, View};
use crabcloud_i18n::I18n;
use crabcloud_ocs::CapabilityProvider;
use crabcloud_storage::ChannelEventSink;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Application-wide composition handle. All fields are `Arc`- or `Clone`-backed
/// so cloning is cheap.
#[derive(Clone)]
pub struct AppState {
    /// Loaded, validated configuration.
    pub config: Arc<FileConfig>,
    /// Database connection pool.
    pub pool: DbPool,
    /// Shared cache backend.
    pub cache: Arc<dyn Cache>,
    /// Translation service.
    pub i18n: Arc<I18n>,
    /// Cached read/write access to the `appconfig` table.
    pub appconfig: AppConfigService,
    /// Mutable registry of capability providers (filled by bootstrap hooks).
    pub capability_providers: Arc<Mutex<Vec<Arc<dyn CapabilityProvider>>>>,
    /// Composed users service (lookup, verify, password, groups, prefs).
    pub users: crabcloud_users::UsersService,
    /// Broadcast sink storage backends publish `StorageEvent`s into. Cloneable
    /// `Arc`; subscribers (the scanner consumer, future indexes) take their
    /// own receiver via `subscribe`.
    pub storage_sink: Arc<ChannelEventSink>,
    /// DB-backed file cache (`oc_filecache` + `oc_storages` + `oc_mimetypes`).
    pub filecache: Arc<FileCache>,
    /// Scanner: continuous event consumer + on-demand `full_scan` + drift
    /// recovery on `RecvError::Lagged`. The consumer task is spawned during
    /// `AppStateBuilder::build` iff `config.filecache.enabled` is true;
    /// `register_storage` / `full_scan` work regardless.
    pub scanner: Arc<Scanner>,
    /// Resolves per-user mounts. 4c default: `HomeMountResolver` over
    /// `LocalStorageFactory` (which uses `config.datadirectory`). Later
    /// sub-projects (sharing, external storage) layer additional resolvers.
    pub mount_resolver: Arc<dyn MountResolver>,
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
    /// Convenience: register a capability provider at runtime. Subsequent
    /// `/ocs/.../capabilities` requests will include its contribution.
    pub async fn register_capability_provider(&self, p: Arc<dyn CapabilityProvider>) {
        self.capability_providers.lock().await.push(p);
    }

    /// Construct a per-request `View` for `uid`. Resolves the user's
    /// mounts via `mount_resolver`.
    pub async fn view_for(&self, uid: &crabcloud_users::UserId) -> crabcloud_fs::FsResult<View> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(View::new(
            uid.clone(),
            mounts,
            self.filecache.clone(),
            self.storage_sink.clone(),
        ))
    }

    /// Construct a per-request `Uploads` façade for `uid`.
    pub async fn uploads_for(
        &self,
        uid: &crabcloud_users::UserId,
    ) -> crabcloud_fs::FsResult<Uploads> {
        let mounts = self.mount_resolver.mounts_for(uid).await?;
        Ok(Uploads::new(
            uid.clone(),
            mounts,
            self.storage_sink.clone(),
            self.filecache.clone(),
        ))
    }
}

/// Builder that loads / connects everything and produces an `AppState`.
pub struct AppStateBuilder {
    config: Arc<FileConfig>,
    catalog_root: Option<std::path::PathBuf>,
    cache: Option<Arc<dyn Cache>>,
    custom_users: Option<crabcloud_users::UsersService>,
    registry: BootstrapRegistry,
}

impl AppStateBuilder {
    /// Start a builder from a parsed configuration.
    pub fn new(config: FileConfig) -> Self {
        Self {
            config: Arc::new(config),
            catalog_root: None,
            cache: None,
            custom_users: None,
            registry: BootstrapRegistry::new(),
        }
    }

    /// Override the i18n catalog root (defaults to "no catalogs").
    pub fn with_catalog_root(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.catalog_root = Some(p.into());
        self
    }

    /// Override the cache backend (defaults to `MemoryCache`).
    pub fn with_cache(mut self, c: Arc<dyn Cache>) -> Self {
        self.cache = Some(c);
        self
    }

    /// Override the `UsersService` (defaults to SQL-backed stores, optionally
    /// wrapped in `BootstrapAdminBackend` if `config.bootstrap_admin` is set).
    pub fn with_users(mut self, service: crabcloud_users::UsersService) -> Self {
        self.custom_users = Some(service);
        self
    }

    /// Register a bootstrap hook to run during `build`.
    pub fn with_hook(mut self, hook: crate::bootstrap::BootstrapHook) -> Self {
        self.registry.register(hook);
        self
    }

    /// Register the default `CoreCapabilities` provider on bootstrap so the
    /// `core` namespace is non-empty at the `/ocs/.../capabilities` route.
    pub fn with_core_capabilities(self) -> Self {
        use crabcloud_ocs::CoreCapabilities;
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
                let catalogs = crabcloud_i18n::load_all(root)
                    .map_err(|e| Error::Internal(anyhow::anyhow!("i18n load: {e}")))?;
                Arc::new(I18n::new(
                    catalogs,
                    crabcloud_i18n::Locale::new(&self.config.default_language),
                ))
            }
            None => Arc::new(I18n::new(
                std::collections::HashMap::new(),
                crabcloud_i18n::Locale::new(&self.config.default_language),
            )),
        };

        let cache = self.cache.unwrap_or_else(|| Arc::new(MemoryCache::new()));

        let appconfig = AppConfigService::new(
            pool.clone(),
            cache.clone(),
            &self.config.dbtableprefix,
            &self.config.instanceid,
        );

        let users = if let Some(svc) = self.custom_users.take() {
            svc
        } else {
            use crabcloud_users::{
                AppPasswordService, BcryptVerifier, GroupStore, PreferenceStore, SqlGroupStore,
                SqlPreferenceStore, SqlTokenStore, SqlUserStore, TokenAuthCache, TokenStore,
                UserStore, UsersService,
            };
            let sql_users: Arc<dyn UserStore> = Arc::new(SqlUserStore::new(pool.clone()));
            let sql_groups: Arc<dyn GroupStore> = Arc::new(SqlGroupStore::new(pool.clone()));
            let sql_prefs: Arc<dyn PreferenceStore> =
                Arc::new(SqlPreferenceStore::new(pool.clone()));
            let user_store: Arc<dyn UserStore> = match &self.config.bootstrap_admin {
                Some(admin) => Arc::new(crabcloud_users::BootstrapAdminBackend::new(
                    sql_users.clone(),
                    sql_groups.clone(),
                    admin.clone(),
                )),
                None => sql_users,
            };
            let group_store: Arc<dyn GroupStore> = match &self.config.bootstrap_admin {
                Some(admin) => Arc::new(crabcloud_users::BootstrapAdminGroupBackend::new(
                    sql_groups.clone(),
                    admin.username.clone(),
                )),
                None => sql_groups,
            };
            let token_store: Arc<dyn TokenStore> = Arc::new(SqlTokenStore::new(pool.clone()));
            let token_cache = Arc::new(TokenAuthCache::new(
                token_store,
                cache.clone(),
                &self.config.instanceid,
            ));
            let app_passwords = Arc::new(AppPasswordService::new(
                token_cache,
                self.config.secret.clone(),
            ));
            UsersService::new(
                user_store,
                group_store,
                sql_prefs,
                Arc::new(BcryptVerifier::new()),
            )
            .with_app_passwords(app_passwords)
        };

        let storage_sink = Arc::new(ChannelEventSink::new(
            self.config.filecache.event_channel_capacity,
        ));
        let filecache = Arc::new(FileCache::new(pool.clone()));
        let scanner = Arc::new(Scanner::new(filecache.clone(), storage_sink.clone()));
        if self.config.filecache.enabled {
            // The consumer task owns its receiver and runs for the process
            // lifetime; the `JoinHandle` is intentionally dropped (graceful
            // shutdown lives outside 4b's scope).
            std::mem::drop(scanner.clone().spawn());
        }

        // Mount resolver: 4c ships home-only via LocalStorageFactory.
        let factory = Arc::new(LocalStorageFactory::new(self.config.datadirectory.clone()));
        let mount_resolver: Arc<dyn MountResolver> = Arc::new(HomeMountResolver::new(factory));

        let state = AppState {
            config: self.config.clone(),
            pool,
            cache,
            i18n,
            appconfig,
            capability_providers: Arc::new(Mutex::new(Vec::new())),
            users,
            storage_sink,
            filecache,
            scanner,
            mount_resolver,
        };

        self.registry.run(&state).await?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
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
    async fn users_service_assembled_with_bootstrap_admin() {
        use crabcloud_users::{BcryptVerifier, PasswordVerifier};
        let dir = tempdir().unwrap();
        let mut cfg = minimal_sqlite_config(dir.path().join("u.db"));
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        cfg.bootstrap_admin = Some(crabcloud_config::BootstrapAdminConfig {
            username: "admin".into(),
            password_hash: hash,
        });
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        let admin = state.users.lookup_by_login("admin").await.unwrap();
        assert!(admin.is_some());
    }

    #[tokio::test]
    async fn register_capability_provider_appends() {
        use crabcloud_ocs::CoreCapabilities;
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
