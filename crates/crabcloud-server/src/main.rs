//! `crabcloud-server` binary: parses the CLI, initializes tracing, and
//! dispatches to subcommands (`serve`, `migrate`, `status`, `version`).

mod cli;
mod telemetry;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use crate::cli::{Cli, Cmd};

fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    let pw = rpassword::prompt_password(prompt)?;
    if pw.is_empty() {
        anyhow::bail!("password cannot be empty");
    }
    Ok(pw)
}

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

#[tokio::main]
async fn main() -> Result<()> {
    crate::telemetry::init();
    let cli = Cli::parse();

    match cli.selected() {
        Cmd::Version => {
            println!(
                "crabcloud-server {pkg_ver}\n\
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
            // `#[server]` function endpoints declared in `crabcloud-ui`.
            // We merge OCS routes + shared middleware on top via `build_router`.
            let app_router = dioxus::server::router(crabcloud_ui::App);
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
        } => {
            let config = crabcloud_config::load(&cli.config, &[])?;
            let pw = prompt_password("New password: ")?;
            let confirm = prompt_password("Confirm: ")?;
            if pw != confirm {
                anyhow::bail!("passwords didn't match");
            }
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
    }
}
