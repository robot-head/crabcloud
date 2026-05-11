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
            Ok(())
        }
        Cmd::Serve => {
            info!(config = %cli.config.display(), "serve subcommand not yet implemented");
            anyhow::bail!("`serve` is implemented in a later phase");
        }
        Cmd::Migrate => {
            info!(config = %cli.config.display(), "migrate subcommand not yet implemented");
            anyhow::bail!("`migrate` is implemented in Task 10");
        }
    }
}
