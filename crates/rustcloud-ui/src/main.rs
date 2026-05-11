//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

#[cfg(target_arch = "wasm32")]
fn main() {
    // Implemented in Task 8.
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // Stub so `cargo build` on the host target doesn't fail. The native binary
    // does nothing; the server crate is `rustcloud-server`.
    eprintln!("rustcloud-ui-web is a WASM-only entry point");
}
