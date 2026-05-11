mod cli;
mod tracing;

use ::tracing::info;
use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Cmd};

#[tokio::main]
async fn main() -> Result<()> {
    crate::tracing::init();
    let cli = Cli::parse();

    match cli.command() {
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
