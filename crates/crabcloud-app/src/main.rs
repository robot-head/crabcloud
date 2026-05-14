//! Crabcloud binary. On wasm32 this compiles to the dioxus client entrypoint
//! that boots WASM hydration; on native targets this is the full axum server
//! plus CLI subcommands (`serve`, `migrate`, `version`, user / group / app-
//! password management, file-cache scanner).

#![allow(unused_crate_dependencies)]

#[cfg(target_arch = "wasm32")]
fn main() {
    // Patch window.fetch to inject the CSRF requesttoken header on outgoing
    // /api/ calls. Must run before dioxus::launch so the patch is in place
    // before any server-fn future is polled. See app.rs for rationale.
    crabcloud_app::install_csrf_fetch_interceptor();
    dioxus::launch(crabcloud_app::App);
}

#[cfg(not(target_arch = "wasm32"))]
mod cli;

#[cfg(not(target_arch = "wasm32"))]
mod telemetry;

#[cfg(not(target_arch = "wasm32"))]
use crate::cli::{Cli, Cmd, FilesCmd};
#[cfg(not(target_arch = "wasm32"))]
use anyhow::Result;
#[cfg(not(target_arch = "wasm32"))]
use clap::Parser;
#[cfg(not(target_arch = "wasm32"))]
use tracing::info;

#[cfg(not(target_arch = "wasm32"))]
fn read_password_from_stdin() -> anyhow::Result<String> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let pw = line.trim_end_matches(['\r', '\n']).to_string();
    if pw.is_empty() {
        anyhow::bail!("empty password");
    }
    Ok(pw)
}

#[cfg(not(target_arch = "wasm32"))]
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    let pw = rpassword::prompt_password(prompt)?;
    if pw.is_empty() {
        anyhow::bail!("password cannot be empty");
    }
    Ok(pw)
}

#[cfg(not(target_arch = "wasm32"))]
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("received Ctrl-C, shutting down"),
        _ = terminate => info!("received SIGTERM, shutting down"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<()> {
    crate::telemetry::init();
    let cli = Cli::parse();

    match cli.selected() {
        Cmd::Version => {
            println!(
                "crabcloud-app {pkg_ver}\n\
                 git:       {git_sha}\n\
                 dialects:  sqlite, mysql, postgres\n\
                 subproject: platform-core",
                pkg_ver = env!("CARGO_PKG_VERSION"),
                // `CRABCLOUD_GIT_SHA` is set by `build.rs` via `git rev-parse`.
                // `VERGEN_GIT_SHA` fallback was dropped: `vergen-gix` requires
                // a Rust 1.88+ transitive, above our 1.85 MSRV.
                git_sha = option_env!("CRABCLOUD_GIT_SHA").unwrap_or("unknown"),
            );
            return Ok(());
        }
        Cmd::Serve => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let bind = config.bind_address;
            info!(
                dbtype = %config.dbtype.as_str(),
                bind = %bind,
                "starting Crabcloud server"
            );

            let state = crabcloud_core::AppStateBuilder::new(config)
                .with_core_capabilities()
                .build()
                .await?;

            // Dioxus fullstack: `dioxus::server::router(App)` wires the SSR
            // fallback, generated index.html, static assets, and the
            // `#[server]` function endpoints declared in `crabcloud-app`.
            // We merge OCS routes + shared middleware on top via `build_router`.
            let app_router = dioxus::server::router(crabcloud_app::App);
            let router = crabcloud_http::build_router(state.clone(), app_router);

            let listener = tokio::net::TcpListener::bind(bind).await?;
            let local_addr = listener.local_addr()?;
            info!(addr = %local_addr, "listening");

            let res = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await;

            info!("server stopped");
            state.pool.close().await;
            res?;
            Ok(())
        }
        Cmd::Migrate => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            info!(
                dbtype = %config.dbtype.as_str(),
                "assembling AppState (this runs migrations)"
            );

            // The builder runs migrations internally; we don't need to call the
            // MigrationRunner separately. Build, then close the pool and exit.
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            info!(
                dialect = state.pool.dialect(),
                "AppState ready; closing pool"
            );
            state.pool.close().await;
            info!("migrate complete");
            Ok(())
        }
        Cmd::UserAdd {
            uid,
            admin,
            email,
            display_name,
            password_stdin,
        } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let pw = if password_stdin {
                read_password_from_stdin()?
            } else {
                let pw = prompt_password("New password: ")?;
                let confirm = prompt_password("Confirm: ")?;
                if pw != confirm {
                    anyhow::bail!("passwords didn't match");
                }
                pw
            };
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_add(
                &state.users,
                &uid,
                &pw,
                display_name.as_deref(),
                email.as_deref(),
                admin,
            )
            .await?;
            info!(uid, admin, "user created");
            state.pool.close().await;
            Ok(())
        }
        Cmd::UserSetPassword { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let pw = prompt_password("New password: ")?;
            let confirm = prompt_password("Confirm: ")?;
            if pw != confirm {
                anyhow::bail!("passwords didn't match");
            }
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_set_password(&state.users, &uid, &pw).await?;
            info!(uid, "password reset");
            state.pool.close().await;
            Ok(())
        }
        Cmd::UserDelete { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            eprint!("Delete user {uid} and all their preferences? (yes/no): ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim() != "yes" {
                anyhow::bail!("aborted");
            }
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::user_delete(&state.users, &uid).await?;
            info!(uid, "user deleted");
            state.pool.close().await;
            Ok(())
        }
        Cmd::GroupAddMember { gid, uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::group_add_member(&state.users, &gid, &uid).await?;
            info!(gid, uid, "added to group");
            state.pool.close().await;
            Ok(())
        }
        Cmd::GroupRemoveMember { gid, uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            crabcloud_users::cli::group_remove_member(&state.users, &gid, &uid).await?;
            info!(gid, uid, "removed from group");
            state.pool.close().await;
            Ok(())
        }
        Cmd::AppPasswordAdd { uid, name } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            let (id, raw) = crabcloud_users::cli::app_password_add(&ap, &uid, &name).await?;
            println!("id={id}");
            println!("token={raw}");
            info!(uid, name, id, "app password created");
            state.pool.close().await;
            Ok(())
        }
        Cmd::AppPasswordList { uid } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            for (id, name, kind, last) in crabcloud_users::cli::app_password_list(&ap, &uid).await?
            {
                let kind_str = match kind {
                    crabcloud_users::AuthTokenType::Session => "session",
                    crabcloud_users::AuthTokenType::AppPassword => "app",
                };
                println!("{id}\t{kind_str}\t{last}\t{name}");
            }
            state.pool.close().await;
            Ok(())
        }
        Cmd::AppPasswordRevoke { id } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let ap = state
                .users
                .app_passwords()
                .ok_or_else(|| anyhow::anyhow!("app_passwords not wired"))?
                .clone();
            crabcloud_users::cli::app_password_revoke(&ap, id).await?;
            info!(id, "app password revoked");
            state.pool.close().await;
            Ok(())
        }
        Cmd::Files(FilesCmd::Scan { storage_id }) => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let state = crabcloud_core::AppStateBuilder::new(config).build().await?;
            let storage = state
                .scanner
                .storage_for(&storage_id)
                .ok_or_else(|| anyhow::anyhow!("unknown storage_id: {storage_id}"))?;
            let count = state.scanner.full_scan(&storage).await?;
            info!(count, storage_id = %storage_id, "files:scan complete");
            println!("scanned {count} entries for storage '{storage_id}'");
            state.pool.close().await;
            Ok(())
        }
    }
}
