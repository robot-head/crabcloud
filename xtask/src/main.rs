//! Crabcloud workspace automation. Wraps common multi-step developer
//! workflows (`check-all`, `build`, `up`/`down`, `release`) so contributors
//! don't have to remember the underlying cargo + dx + docker-compose
//! incantations.

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
    /// Build the Dioxus fullstack bundle (server binary + WASM client + assets).
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
    // Dioxus fullstack: a single `dx build` invocation produces the server
    // binary, the WASM client bundle, and the generated index.html. The
    // legacy two-step (dx → cargo) flow doesn't apply once SSR runs through
    // `dioxus::server::router(App)` instead of rust-embed.
    //
    // We omit `--platform` because the dx 0.7 CLI no longer accepts the old
    // `fullstack` value; left empty it defaults to the platform inferred
    // from `Dioxus.toml` / the crate's [[bin]] config, which for this crate
    // produces the same dual output (server bin + web bundle).
    run_in_dir("crates/crabcloud-ui", "dx", &["build", "--release"])?;
    // Server binary builds standalone too, for non-dx deploys (uses the
    // bundle dx produced above for asset serving at runtime).
    run("cargo", &["build", "--release", "-p", "crabcloud-server"])?;
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
