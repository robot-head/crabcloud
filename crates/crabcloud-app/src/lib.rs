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
#[cfg(feature = "server")]
use crabcloud_fs as _;

// Native-target binary deps. `main.rs` (CLI + serve) consumes these directly,
// but the lib doesn't — anchor them here on non-wasm so the workspace's
// `unused_crate_dependencies = warn` lint stays quiet.
#[cfg(not(target_arch = "wasm32"))]
use anyhow as _;
#[cfg(not(target_arch = "wasm32"))]
use clap as _;
#[cfg(not(target_arch = "wasm32"))]
use crabcloud_config as _;
// Native-only: `main.rs` reads `IP` + `PORT` via
// `dioxus_cli_config::fullstack_address_or_localhost()` when bound under
// `dx serve`. Anchored here so lib-only `cargo check` keeps the lint quiet.
#[cfg(not(target_arch = "wasm32"))]
use dioxus_cli_config as _;
#[cfg(not(target_arch = "wasm32"))]
use rpassword as _;
#[cfg(not(target_arch = "wasm32"))]
use tokio as _;
#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber as _;

// `js-sys` is consumed only by the wasm32 `current_time_ms` helper in
// `pages::files::row`. The dependency must still be declared for the WASM
// build, so silence the unused-crate lint on native targets.
#[cfg(not(target_arch = "wasm32"))]
use js_sys as _;

// Web-platform deps used by the chunked-upload state machine in
// `pages::files::upload`. They're only referenced under
// `#[cfg(target_arch = "wasm32")]` so the native (server) build doesn't
// actually touch them — anchor them here to silence
// `unused_crate_dependencies` on the server.
#[cfg(not(target_arch = "wasm32"))]
use gloo_net as _;
#[cfg(not(target_arch = "wasm32"))]
use wasm_bindgen as _;
#[cfg(not(target_arch = "wasm32"))]
use wasm_bindgen_futures as _;
#[cfg(not(target_arch = "wasm32"))]
use web_sys as _;

mod app;
mod context;
pub mod pages;
mod server_fns;

#[cfg(feature = "server")]
pub mod server;

#[cfg(target_arch = "wasm32")]
pub use app::install_csrf_fetch_interceptor;
pub use app::{App, Route};
pub use context::RequestContext;
// re-exported for integration tests (tests/server_fns_public_link.rs)
pub use server_fns::public_link::PublicLinkMeta;
// re-exported for integration tests (tests/server_fns_activity.rs) and
// the Dioxus UI (SP14 Batch D).
pub use server_fns::activity::{
    get_activity_settings, list_activity, set_activity_setting, ActivityRowDto, ActivitySettingDto,
    ListActivityResponse,
};
// re-exported for integration tests (tests/server_fns_trash.rs) and the
// Dioxus UI (Batch D).
pub use server_fns::trash::{
    empty_trash, list_trash, purge_trash, restore_trash, RestoredDto, TrashEntryDto,
};
// re-exported for integration tests (tests/server_fns_versions.rs) and the
// Dioxus UI (Batch D).
pub use server_fns::versions::{delete_version, list_versions, restore_version, VersionDto};
pub use server_fns::{
    count_incoming_shares, delete, list_dir, login, mkdir, move_paths, rename,
    share_recipient_search, status, upload_begin, FileEntry, RecipientCandidate, StatusInfo,
    UploadBeginResponse,
};
