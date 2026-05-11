//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

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
pub use ssr::{html_escape, render_app_html, render_head_html, HTML_DOCTYPE};
