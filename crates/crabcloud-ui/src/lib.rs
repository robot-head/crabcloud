//! Crabcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

// Integration-test fixtures pull in many sibling crates; their deps appear in
// `[dev-dependencies]` and surface as `unused_crate_dependencies` for the lib's
// own test build. Quiet those here so the genuine signal stays visible.
#![cfg_attr(test, allow(unused_crate_dependencies))]

// Wasm32 target pulls in deps consumed only by the WASM bin or by SSR code
// that's cfg-gated out for this target; the lib build flags them as unused
// extern crates. Silence them on the wasm32 target only.
#[cfg(target_arch = "wasm32")]
#[allow(unused_extern_crates)]
mod _wasm_lint_silencer {
    use console_error_panic_hook as _;
    use dioxus_history as _;
    use dioxus_web as _;
    use rust_embed as _;
    use web_sys as _;
}

// `dioxus_router` is referenced indirectly via `dioxus::prelude::*` re-exports.
// Keep an explicit crate-level use so the unused-crate-dependencies lint
// recognizes the dependency.
use dioxus_router as _;

mod app;
mod context;
mod hydration;
pub mod pages;

// SSR helpers + asset serving compile only for the host (server) target.
// The browser WASM bundle never touches `dioxus-ssr` or `axum`.
#[cfg(not(target_arch = "wasm32"))]
pub mod assets;
#[cfg(not(target_arch = "wasm32"))]
mod ssr;

pub use app::{App, Route};
pub use context::RequestContext;
pub use hydration::render_hydration_script;

#[cfg(not(target_arch = "wasm32"))]
pub use ssr::{
    html_escape, is_not_found, render_app_html, render_head_html, resolve_route, HTML_DOCTYPE,
};
