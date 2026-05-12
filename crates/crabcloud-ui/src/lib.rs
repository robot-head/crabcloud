//! Crabcloud UI — Dioxus 0.7 fullstack application. The same component tree
//! is rendered on the server (SSR) and hydrated on the client (WASM). Per-
//! request data (user id, locale) flows through `FullstackContext` on the
//! server, is replayed into the hydration payload via `use_server_cached`, and
//! reaches components through `use_context`. The CSRF token is emitted as a
//! `<meta name="requesttoken">` tag from the App root so existing client code
//! that reads it from the DOM continues to work.

#![cfg_attr(test, allow(unused_crate_dependencies))]

// `crabcloud-cache` is reached for indirectly through `Arc<dyn Cache>` method
// calls inside `#[server]` function bodies, which doesn't require the trait
// to be in scope. Keep the dep listed and silence the lint.
#[cfg(feature = "server")]
use crabcloud_cache as _;

mod app;
mod context;
pub mod pages;
mod server_fns;

#[cfg(feature = "server")]
pub mod server;

pub use app::{App, Route};
pub use context::RequestContext;
pub use server_fns::{login, status, StatusInfo};
