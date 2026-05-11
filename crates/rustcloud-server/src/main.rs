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
                instanceid = %config.instanceid,
                dbtype = %config.dbtype.as_str(),
                "loaded config; migrate subcommand not yet implemented"
            );
            anyhow::bail!("`migrate` is implemented in Task 10");
        }
    }
}
