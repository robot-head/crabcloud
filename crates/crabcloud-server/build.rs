//! Build script for `crabcloud-server`. Captures git SHA via `git rev-parse HEAD`
//! and emits it as a `cargo:rustc-env=CRABCLOUD_GIT_SHA=...` line so
//! `option_env!()` in main.rs can read it.
//!
//! `vergen-gix` would be the preferred path but its 1.x line transitively
//! requires rustc 1.88 (via `vergen 9.1.0` / `time 0.3.47`); our MSRV is
//! 1.85. The Command-based fallback is documented in the Phase 5 plan.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CRABCLOUD_GIT_SHA={}", sha);
    println!("cargo:rerun-if-changed=.git/HEAD");
}
