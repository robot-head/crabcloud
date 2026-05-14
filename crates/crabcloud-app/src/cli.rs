#![cfg(not(target_arch = "wasm32"))]

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "crabcloud-app", version, about = "Crabcloud server")]
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
    /// Create a user. Reads the password interactively from the TTY by
    /// default; pass `--password-stdin` to read a single line of password
    /// from stdin instead (suitable for automation / CI).
    UserAdd {
        uid: String,
        #[arg(long)]
        admin: bool,
        #[arg(long)]
        email: Option<String>,
        #[arg(long = "display-name")]
        display_name: Option<String>,
        /// Read the password as one line from stdin (no confirmation prompt).
        /// Required for non-interactive use; rpassword's TTY-only read fails
        /// when piped.
        #[arg(long = "password-stdin")]
        password_stdin: bool,
    },
    /// Reset a user's password (prompts on stdin).
    UserSetPassword { uid: String },
    /// Delete a user (irreversible; prompts for confirmation).
    UserDelete { uid: String },
    /// Add a user to a group.
    GroupAddMember { gid: String, uid: String },
    /// Remove a user from a group.
    GroupRemoveMember { gid: String, uid: String },
    /// Create a new app password for a user. Prints the plaintext exactly once.
    AppPasswordAdd { uid: String, name: String },
    /// List a user's tokens (id, kind, last_activity, name).
    AppPasswordList { uid: String },
    /// Revoke an app password by row id.
    AppPasswordRevoke { id: i64 },
    /// File-cache scanner commands.
    #[command(subcommand)]
    Files(FilesCmd),
}

/// Subcommands under `crabcloud-app files …`.
#[derive(Subcommand, Debug, Clone)]
pub enum FilesCmd {
    /// Walk a registered storage from root, reconciling cache state.
    Scan {
        /// `Storage::id()` of a registered storage. 4b ships no storage
        /// registrations by default; mounts arrive in 4c.
        storage_id: String,
    },
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
        let cli = Cli::parse_from(["crabcloud-app"]);
        assert!(matches!(cli.selected(), Cmd::Serve));
    }

    #[test]
    fn version_subcommand_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "version"]);
        assert!(matches!(cli.selected(), Cmd::Version));
    }

    #[test]
    fn config_flag_overrides_default() {
        let cli = Cli::parse_from(["crabcloud-app", "--config", "/tmp/custom.toml", "version"]);
        assert_eq!(cli.config, std::path::PathBuf::from("/tmp/custom.toml"));
    }

    #[test]
    fn user_add_subcommand_parses() {
        let cli = Cli::parse_from([
            "crabcloud-app",
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
                password_stdin,
            } => {
                assert_eq!(uid, "alice");
                assert!(admin);
                assert_eq!(email.as_deref(), Some("alice@example.com"));
                assert!(display_name.is_none());
                assert!(!password_stdin);
            }
            _ => panic!("expected UserAdd"),
        }
    }

    #[test]
    fn user_add_password_stdin_flag_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "user-add", "alice", "--password-stdin"]);
        assert!(matches!(
            cli.selected(),
            Cmd::UserAdd {
                password_stdin: true,
                ..
            }
        ));
    }

    #[test]
    fn app_password_add_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "app-password-add", "alice", "DAV"]);
        match cli.selected() {
            Cmd::AppPasswordAdd { uid, name } => {
                assert_eq!(uid, "alice");
                assert_eq!(name, "DAV");
            }
            _ => panic!("expected AppPasswordAdd"),
        }
    }

    #[test]
    fn app_password_revoke_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "app-password-revoke", "42"]);
        match cli.selected() {
            Cmd::AppPasswordRevoke { id } => assert_eq!(id, 42),
            _ => panic!("expected AppPasswordRevoke"),
        }
    }

    #[test]
    fn app_password_list_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "app-password-list", "alice"]);
        match cli.selected() {
            Cmd::AppPasswordList { uid } => assert_eq!(uid, "alice"),
            _ => panic!("expected AppPasswordList"),
        }
    }

    #[test]
    fn files_scan_subcommand_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "files", "scan", "local::/srv/data"]);
        match cli.selected() {
            Cmd::Files(FilesCmd::Scan { storage_id }) => {
                assert_eq!(storage_id, "local::/srv/data");
            }
            _ => panic!("expected Files(Scan)"),
        }
    }

    #[test]
    fn group_add_member_subcommand_parses() {
        let cli = Cli::parse_from(["crabcloud-app", "group-add-member", "admin", "bob"]);
        match cli.selected() {
            Cmd::GroupAddMember { gid, uid } => {
                assert_eq!(gid, "admin");
                assert_eq!(uid, "bob");
            }
            _ => panic!("expected GroupAddMember"),
        }
    }
}
