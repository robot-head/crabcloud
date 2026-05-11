//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod app;
mod context;
mod hydration;
pub mod pages;
mod ssr;

pub use app::{App, Route};
pub use context::RequestContext;
pub use hydration::render_hydration_script;
pub use ssr::{html_escape, render_app_html, render_head_html, HTML_DOCTYPE};
