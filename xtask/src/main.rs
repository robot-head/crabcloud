use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(name = "xtask", about = "Project automation commands")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run `cargo fmt --check && cargo clippy && cargo test`.
    CheckAll,
    /// Start MySQL + Postgres via docker compose.
    Up,
    /// Stop the dev docker compose stack.
    Down,
    /// Build WASM client (`dx build --release`) then server (`cargo build --release`).
    Build,
    /// Stub — implemented in a later phase.
    Dev,
    /// Stub — implemented in a later phase (no query! macros yet).
    Prepare,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::CheckAll => check_all(),
        Cmd::Up => compose(&["up", "-d", "--wait"]),
        Cmd::Down => compose(&["down", "-v"]),
        Cmd::Build => build_all(),
        Cmd::Dev => bail!("`dev` is implemented in a later phase"),
        Cmd::Prepare => bail!("`prepare` is implemented in a later phase"),
    }
}

fn compose(args: &[&str]) -> Result<()> {
    let mut all = vec!["compose", "-f", "dev/docker-compose.yml"];
    all.extend_from_slice(args);
    run("docker", &all)
}

fn check_all() -> Result<()> {
    run("cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", &["test", "--workspace"])?;
    Ok(())
}

fn build_all() -> Result<()> {
    // 1. Build the WASM client + bundle assets.
    run_in_dir("crates/rustcloud-ui", "dx", &["build", "--release"])?;
    // 2. Build the server binary (which embeds the assets via rust-embed).
    run("cargo", &["build", "--release", "-p", "rustcloud-server"])?;
    Ok(())
}

fn run_in_dir(dir: &str, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).current_dir(dir).status()?;
    if !status.success() {
        bail!(
            "`(cd {dir} && {program} {})` exited with status {status}",
            args.join(" ")
        );
    }
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        bail!(
            "`{} {}` exited with status {}",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}
