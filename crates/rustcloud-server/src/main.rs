mod cli;
mod telemetry;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use crate::cli::{Cli, Cmd};

#[tokio::main]
async fn main() -> Result<()> {
    crate::telemetry::init();
    let cli = Cli::parse();

    match cli.selected() {
        Cmd::Version => {
            println!(
                "rustcloud-server {} (build target subproject: platform-core)",
                env!("CARGO_PKG_VERSION")
            );
            return Ok(());
        }
        Cmd::Serve => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                instanceid = %config.instanceid,
                dbtype = %config.dbtype.as_str(),
                "loaded config; serve subcommand not yet implemented"
            );
            anyhow::bail!("`serve` is implemented in a later phase");
        }
        Cmd::Migrate => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                dbtype = %config.dbtype.as_str(),
                "assembling AppState (this runs migrations)"
            );

            // The builder runs migrations internally; we don't need to call the
            // MigrationRunner separately. Build, then close the pool and exit.
            let state = rustcloud_core::AppStateBuilder::new(config).build().await?;
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
