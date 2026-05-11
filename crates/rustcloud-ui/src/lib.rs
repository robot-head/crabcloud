//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.
//!
//! Phase 4 mounts the SSR handler at the catch-all fall-through for the HTTP
//! router. See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §8.

// Sub-modules are added incrementally in subsequent tasks.

use axum::Router;
use rustcloud_core::AppState;

/// Phase 4 placeholder. Returns an empty `Router<AppState>` so downstream
/// crates compile while Tasks 2-7 fill in the SSR handler.
pub fn ui_router() -> Router<AppState> {
    Router::new()
}
