//! Entry point used by `dx` to compile the WASM client bundle. The native
//! build of this binary is a no-op stub — the real server binary lives in
//! `crabcloud-server`, which assembles the full axum stack (OCS routes +
//! middleware + Dioxus fullstack router) itself.

#![allow(unused_crate_dependencies)]

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(crabcloud_ui::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "crabcloud-ui is a WASM-only entry point; run `crabcloud-server` for the native binary."
    );
}
