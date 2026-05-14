//! Entry point used by `dx` to compile the WASM client bundle. The native
//! build of this binary is a no-op stub — the real server binary lives in
//! `crabcloud-server`, which assembles the full axum stack (OCS routes +
//! middleware + Dioxus fullstack router) itself.

#![allow(unused_crate_dependencies)]

#[cfg(target_arch = "wasm32")]
fn main() {
    // Patch window.fetch to inject the CSRF requesttoken header on outgoing
    // /api/ calls. Must run before dioxus::launch so the patch is in place
    // before any server-fn future is polled. See app.rs for rationale.
    crabcloud_app::install_csrf_fetch_interceptor();
    dioxus::launch(crabcloud_app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "crabcloud-app is a WASM-only entry point; run `crabcloud-server` for the native binary."
    );
}
