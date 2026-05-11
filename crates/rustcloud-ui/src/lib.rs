//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod context;

pub use context::RequestContext;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
