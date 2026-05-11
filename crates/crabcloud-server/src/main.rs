//! `crabcloud-server` binary: parses the CLI, initializes tracing, and
//! dispatches to subcommands (`serve`, `migrate`, `status`, `version`).

mod cli;
mod telemetry;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use crate::cli::{Cli, Cmd};

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

            let router = crabcloud_http::build_router(state.clone());

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
    }
}
