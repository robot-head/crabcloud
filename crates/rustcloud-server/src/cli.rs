use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "rustcloud-server", version, about = "Rustcloud server")]
pub struct Cli {
    /// Path to the main config file.
    #[arg(
        long,
        env = "RUSTCLOUD_CONFIG",
        default_value = "config/config.toml",
        global = true
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Start the HTTP server (implemented in a later phase).
    Serve,
    /// Run pending migrations and exit (implemented in Task 10).
    Migrate,
    /// Print version information.
    Version,
}

impl Cli {
    pub fn command(&self) -> Cmd {
        // Default subcommand is `serve` when none specified.
        match &self.command {
            Some(c) => match c {
                Cmd::Serve => Cmd::Serve,
                Cmd::Migrate => Cmd::Migrate,
                Cmd::Version => Cmd::Version,
            },
            None => Cmd::Serve,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_command_factory_is_valid() {
        // `debug_assert` panics on invalid clap configuration.
        <Cli as CommandFactory>::command().debug_assert();
    }

    #[test]
    fn default_subcommand_is_serve() {
        let cli = Cli::parse_from(["rustcloud-server"]);
        assert!(matches!(cli.command(), Cmd::Serve));
    }

    #[test]
    fn version_subcommand_parses() {
        let cli = Cli::parse_from(["rustcloud-server", "version"]);
        assert!(matches!(cli.command(), Cmd::Version));
    }

    #[test]
    fn config_flag_overrides_default() {
        let cli = Cli::parse_from([
            "rustcloud-server",
            "--config",
            "/tmp/custom.toml",
            "version",
        ]);
        assert_eq!(cli.config, std::path::PathBuf::from("/tmp/custom.toml"));
    }
}
