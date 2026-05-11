//! Ensure the `target/dx/rustcloud-ui/release/web/public/` directory exists
//! before `rust-embed`'s proc macro inspects it. On a fresh checkout that
//! never ran `dx build`, the directory wouldn't exist and `#[derive(RustEmbed)]`
//! would error at compile time. Creating an empty placeholder lets the macro
//! produce an empty asset set; the asset handler then returns 404 for every
//! path until `cargo xtask build` (Task 10) populates the directory.

use std::path::PathBuf;

fn main() {
    // Walk up from `CARGO_MANIFEST_DIR` (`crates/rustcloud-ui/`) to the
    // workspace root, then mirror the path encoded in `assets.rs`.
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    let mut dir = PathBuf::from(manifest_dir);
    dir.push("..");
    dir.push("..");
    dir.push("target");
    dir.push("dx");
    dir.push("rustcloud-ui");
    dir.push("release");
    dir.push("web");
    dir.push("public");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        // Don't fail the build; report a warning so contributors see it.
        println!(
            "cargo:warning=rustcloud-ui: could not create {}: {}",
            dir.display(),
            e
        );
    }
    // We don't need to rerun unless this script itself changes.
    println!("cargo:rerun-if-changed=build.rs");
}
