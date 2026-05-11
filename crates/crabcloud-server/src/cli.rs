use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "crabcloud-server", version, about = "Crabcloud server")]
pub struct Cli {
    /// Path to the main config file.
    #[arg(
        long,
        env = "CRABCLOUD_CONFIG",
        default_value = "config/config.toml",
        global = true
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Start the HTTP server (implemented in a later phase).
    Serve,
    /// Run pending migrations and exit (implemented in Task 10).
    Migrate,
    /// Print version information.
    Version,
    /// Create a user (prompts for password on stdin).
    UserAdd {
        uid: String,
        #[arg(long)]
        admin: bool,
        #[arg(long)]
        email: Option<String>,
        #[arg(long = "display-name")]
        display_name: Option<String>,
    },
    /// Reset a user's password (prompts on stdin).
    UserSetPassword { uid: String },
    /// Delete a user (irreversible; prompts for confirmation).
    UserDelete { uid: String },
    /// Add a user to a group.
    GroupAddMember { gid: String, uid: String },
    /// Remove a user from a group.
    GroupRemoveMember { gid: String, uid: String },
}

impl Cli {
    pub fn selected(&self) -> Cmd {
        self.command.clone().unwrap_or(Cmd::Serve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_command_factory_is_valid() {
        // `debug_assert` panics on invalid clap configuration.
        Cli::command().debug_assert();
    }

    #[test]
    fn default_subcommand_is_serve() {
        let cli = Cli::parse_from(["crabcloud-server"]);
        assert!(matches!(cli.selected(), Cmd::Serve));
    }

    #[test]
    fn version_subcommand_parses() {
        let cli = Cli::parse_from(["crabcloud-server", "version"]);
        assert!(matches!(cli.selected(), Cmd::Version));
    }

    #[test]
    fn config_flag_overrides_default() {
        let cli = Cli::parse_from([
            "crabcloud-server",
            "--config",
            "/tmp/custom.toml",
            "version",
        ]);
        assert_eq!(cli.config, std::path::PathBuf::from("/tmp/custom.toml"));
    }

    #[test]
    fn user_add_subcommand_parses() {
        let cli = Cli::parse_from([
            "crabcloud-server",
            "user-add",
            "alice",
            "--admin",
            "--email",
            "alice@example.com",
        ]);
        match cli.selected() {
            Cmd::UserAdd {
                uid,
                admin,
                email,
                display_name,
            } => {
                assert_eq!(uid, "alice");
                assert!(admin);
                assert_eq!(email.as_deref(), Some("alice@example.com"));
                assert!(display_name.is_none());
            }
            _ => panic!("expected UserAdd"),
        }
    }

    #[test]
    fn group_add_member_subcommand_parses() {
        let cli = Cli::parse_from(["crabcloud-server", "group-add-member", "admin", "bob"]);
        match cli.selected() {
            Cmd::GroupAddMember { gid, uid } => {
                assert_eq!(gid, "admin");
                assert_eq!(uid, "bob");
            }
            _ => panic!("expected GroupAddMember"),
        }
    }
}
