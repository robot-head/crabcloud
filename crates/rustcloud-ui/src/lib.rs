//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod context;
mod hydration;
pub mod pages;

pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
