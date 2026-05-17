//! `AppState` — the clone-cheap composition handle.

use crate::appconfig::AppConfigService;
use crate::bootstrap::BootstrapRegistry;
use crate::error::{CoreResult, Error};
use crate::mail_queue::MailQueue;
use crate::mail_queue_cleanup::MailQueueCleanup;
use crate::mail_worker::MailWorker;
use crate::preview_cache_cleanup::PreviewCacheCleanup;
use crate::publiclinks::SharesTokenLookup;
use crate::trash_sweeper::TrashSweeper;
use crabcloud_cache::{Cache, MemoryCache};
use crabcloud_config::FileConfig;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_fs::{
    HomeMountResolver, LocalStorageFactory, MountResolver, ShareMountResolver, StorageFactory,
    Uploads, View,
};
use crabcloud_i18n::I18n;
use crabcloud_ocs::CapabilityProvider;
use crabcloud_preview::PreviewCache;
use crabcloud_publiclinks::{Passwords, PublicLinkAuthState, RateLimiter, TokenLookup};
use crabcloud_storage::ChannelEventSink;
use dashmap::DashMap;
use secrecy::ExposeSecret;
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
    /// Resolves per-user mounts. SP7 default: `ShareMountResolver` wrapping
    /// `HomeMountResolver` over `LocalStorageFactory` so accepted incoming
    /// shares show up as extra mounts alongside the user's home.
    pub mount_resolver: Arc<dyn MountResolver>,
    /// Per-user home storage factory. Exposed so OCS / server-fn layers can
    /// resolve `home_storage(uid).id()` for filecache lookups without
    /// reconstructing the factory.
    pub storage_factory: Arc<dyn StorageFactory>,
    /// User + group sharing service. Reads / writes `oc_share` and resolves
    /// recipient memberships via `users`.
    pub shares: Arc<crabcloud_sharing::Shares>,
    /// Public-link auth state: token lookup, password verifier, per-token
    /// rate limiter, and unlock-cookie HMAC key. Mounted by the public-link
    /// axum middleware (Batch E wires the routes). The HMAC key reuses
    /// `FileConfig::secret` — cleanly domain-separated by cookie name
    /// (`pl_<token>`) and by the token being included in the MAC input, so
    /// no extra secret material is needed.
    pub publiclinks_auth: Arc<PublicLinkAuthState>,
    /// Preview cache for thumbnail / first-page-PDF previews. Keyed on
    /// `(storage_id, fileid, size, source_etag)` with a per-key dedup
    /// lock so concurrent first-request renders share a single task.
    pub preview: Arc<PreviewCache>,
    /// In-process map from the client-chosen URL-segment `upload_id` (the
    /// `{upload_id}` path component in `/dav/uploads/{user}/{upload_id}/...`)
    /// to the server-encoded `upload_id` returned by `Uploads::begin`. Holds
    /// for the duration of a chunked upload (MKCOL → PUT × N → MOVE/DELETE);
    /// dropped on commit or abort. Process-local; chunked uploads do not
    /// survive a restart (matches Nextcloud's behavior).
    pub upload_id_map: Arc<DashMap<String, String>>,
    /// Mail transport. Wired by `AppStateBuilder` from `FileConfig.mail`.
    /// The `MailWorker` is the consumer; application code generally
    /// enqueues via `mail_queue` instead of calling this directly.
    pub mailer: Arc<crabcloud_mail::Mailer>,
    /// Persistent outbound-mail queue (`oc_mail_queue`).
    pub mail_queue: MailQueue,
    /// Per-user notification opt-out service
    /// (`oc_user_notification_prefs`).
    pub notification_prefs: crabcloud_users::NotificationPrefs,
    /// Mail worker shutdown handle. Always present; only meaningful
    /// when a worker was actually spawned (i.e. `mail.transport != "disabled"`).
    /// Tests signal `notify_one` here to drain the worker between runs.
    pub mail_worker_shutdown: Arc<tokio::sync::Notify>,
    /// Expiration-warning sweeper shutdown handle. Same shape as
    /// `mail_worker_shutdown` — present unconditionally; the task is
    /// only spawned when mail transport is not Disabled.
    pub expiration_sweeper_shutdown: Arc<tokio::sync::Notify>,
    /// Mail-queue cleanup shutdown handle. Always present; only
    /// meaningful when the task was spawned (i.e. mail transport is not
    /// Disabled — same gate as `mail_worker_shutdown`).
    pub mail_queue_cleanup_shutdown: Arc<tokio::sync::Notify>,
    /// Preview-cache cleanup shutdown handle. Always present and the
    /// task is spawned unconditionally — preview files accumulate
    /// regardless of mail transport, and tests can `notify_one()` here
    /// in teardown if needed.
    pub preview_cache_cleanup_shutdown: Arc<tokio::sync::Notify>,
    /// Trash bin service. Cheap to clone.
    pub trash: Arc<crabcloud_trash::Trash>,
    /// Trash sweeper shutdown handle. Always present; spawned
    /// unconditionally in `AppStateBuilder::build`.
    pub trash_sweeper_shutdown: Arc<tokio::sync::Notify>,
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

        // Mount resolver: SP7 wraps HomeMountResolver with ShareMountResolver
        // so accepted incoming shares surface as extra mounts.
        let storage_factory: Arc<dyn StorageFactory> =
            Arc::new(LocalStorageFactory::new(self.config.datadirectory.clone()));

        // Mail wiring needs to be built BEFORE `Shares` because the share
        // service owns an `Arc<dyn MailEnqueuer>` (impl'd by `MailQueue`).
        // `Shares::new` also takes `NotificationPrefs` and the instance URL
        // for templated link generation.
        let mail_queue = MailQueue::new(Arc::new(pool.clone()));
        let notification_prefs = crabcloud_users::NotificationPrefs::new(Arc::new(pool.clone()));
        let instance_url = self.config.overwrite_cli_url.clone().unwrap_or_default();

        let shares = Arc::new(crabcloud_sharing::Shares::new(
            crabcloud_sharing::SharesConfig {
                pool: Arc::new(pool.clone()),
                users: Arc::new(users.clone()),
                filecache: filecache.clone(),
                mail: Arc::new(mail_queue.clone()),
                prefs: notification_prefs.clone(),
                instance_url,
            },
        ));
        let mount_resolver: Arc<dyn MountResolver> = Arc::new(ShareMountResolver::new(
            HomeMountResolver::new(storage_factory.clone()),
            shares.clone(),
            storage_factory.clone(),
            filecache.clone(),
        ));

        let upload_id_map = Arc::new(DashMap::new());

        // Public-link auth: assembled after `shares` so the token-lookup
        // adapter can borrow it. Reuses `config.secret` for unlock-cookie
        // HMAC (rotation can split later if needed).
        let publiclinks_lookup: Arc<dyn TokenLookup> = Arc::new(SharesTokenLookup {
            shares: shares.clone(),
        });
        let publiclinks_auth = Arc::new(PublicLinkAuthState {
            lookup: publiclinks_lookup,
            passwords: Arc::new(Passwords::new()),
            rate_limiter: Arc::new(RateLimiter::new()),
            secret: self.config.secret.expose_secret().as_bytes().to_vec(),
        });

        // Preview cache: rooted at `<datadirectory>/appdata/preview` by
        // default; operators can override via `preview_root`. The cache
        // owns its on-disk layout (`<root>/<storage_id>/<fileid>/...`),
        // so we just ensure the root exists.
        let preview_root = self
            .config
            .preview_root
            .clone()
            .unwrap_or_else(|| self.config.datadirectory.join("appdata").join("preview"));
        tokio::fs::create_dir_all(&preview_root).await.ok();
        let preview = Arc::new(PreviewCache::new(
            preview_root.clone(),
            self.config.preview_max_pixels,
        ));

        // Preview cache cleanup: spawned unconditionally — the cache
        // accumulates files regardless of mail-transport status.
        let (preview_cache_cleanup, preview_cache_cleanup_shutdown) =
            PreviewCacheCleanup::new(preview_root, self.config.preview_retention_days);
        std::mem::drop(tokio::spawn(
            async move { preview_cache_cleanup.run().await },
        ));

        // Trash service + daily sweeper. The sweeper is spawned
        // unconditionally (trash exists regardless of mail transport);
        // tests can `notify_one()` on `trash_sweeper_shutdown` in
        // teardown.
        let trash = Arc::new(crabcloud_trash::Trash::new(
            Arc::new(pool.clone()),
            self.config.datadirectory.clone(),
        ));
        let (trash_sweeper, trash_sweeper_shutdown) =
            TrashSweeper::new(trash.clone(), self.config.trash_retention_days);
        std::mem::drop(tokio::spawn(async move { trash_sweeper.run().await }));

        // Mail wiring: build transport, queue, prefs, and (when not
        // disabled) spawn the worker. The transport kind is mapped from
        // the string in `config.mail.transport`; unknown values degrade
        // to `Disabled` rather than failing boot.
        let mail_cfg = &self.config.mail;
        let mail_transport_cfg = crabcloud_mail::TransportConfig {
            kind: match mail_cfg.transport.as_str() {
                "smtp" => crabcloud_mail::TransportKind::Smtp,
                "log" => crabcloud_mail::TransportKind::Log,
                _ => crabcloud_mail::TransportKind::Disabled,
            },
            smtp_host: mail_cfg.smtp_host.clone(),
            smtp_port: mail_cfg.smtp_port,
            smtp_username: mail_cfg.smtp_username.clone(),
            smtp_password: mail_cfg.smtp_password.clone(),
            smtp_security: match mail_cfg.smtp_security.as_str() {
                "tls" => crabcloud_mail::SmtpSecurity::Tls,
                "none" => crabcloud_mail::SmtpSecurity::None,
                _ => crabcloud_mail::SmtpSecurity::Starttls,
            },
            mail_from: mail_cfg.mail_from.clone(),
            mail_from_name: mail_cfg.mail_from_name.clone(),
        };
        let mailer = Arc::new(
            crabcloud_mail::Mailer::from_config(&mail_transport_cfg).map_err(Error::Mail)?,
        );
        // `mail_queue` + `notification_prefs` were built above (Shares needs
        // them at construction time); reuse those clones here.
        let (mail_worker, mail_worker_shutdown) =
            MailWorker::new(mail_queue.clone(), mailer.clone());
        let (expiration_sweeper, expiration_sweeper_shutdown) =
            crate::expiration_sweeper::ExpirationWarningSweeper::new(
                shares.clone(),
                mail_queue.clone(),
                users.clone(),
                notification_prefs.clone(),
                self.config.overwrite_cli_url.clone().unwrap_or_default(),
            );
        let (mail_queue_cleanup, mail_queue_cleanup_shutdown) = MailQueueCleanup::new(
            Arc::new(pool.clone()),
            self.config.mail_queue_retention_days,
        );
        if !matches!(
            mail_transport_cfg.kind,
            crabcloud_mail::TransportKind::Disabled
        ) {
            // The `JoinHandle` is intentionally dropped; the worker
            // terminates when `mail_worker_shutdown.notify_one()` is
            // called (typically at process shutdown / test teardown).
            std::mem::drop(tokio::spawn(async move { mail_worker.run().await }));
            // Same shape for the expiration sweeper. Skipped under
            // Disabled transport so unit tests with the default config
            // don't spin a background task they have to drain.
            std::mem::drop(tokio::spawn(async move { expiration_sweeper.run().await }));
            // Same gate as the rest of mail bg tasks — tests with the
            // default (disabled) transport don't have to drain it.
            std::mem::drop(tokio::spawn(async move { mail_queue_cleanup.run().await }));
        }

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
            storage_factory,
            shares,
            publiclinks_auth,
            preview,
            upload_id_map,
            mailer,
            mail_queue,
            notification_prefs,
            mail_worker_shutdown,
            expiration_sweeper_shutdown,
            mail_queue_cleanup_shutdown,
            preview_cache_cleanup_shutdown,
            trash,
            trash_sweeper_shutdown,
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
