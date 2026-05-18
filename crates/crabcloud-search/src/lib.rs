//! File-metadata search service for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-search-design.md`.
//!
//! Public entry points: [`Search`] (query + write API), [`SearchFanout`]
//! (trait used by `crabcloud-sharing` to drive share-lifecycle fan-out),
//! and the value types in [`types`]. SQL dispatch mirrors the
//! `crabcloud-activity` / `crabcloud-versions` pattern.

// Dev-dependency anchors. The lib's own `cargo test` target sees these
// crates declared in `[dev-dependencies]` but doesn't reference them
// from the lib's own (non-integration) tests. Silence the
// `unused_crate_dependencies` lint here so the workspace clippy run
// stays green.
#[cfg(test)]
use crabcloud_config as _;
#[cfg(test)]
use crabcloud_storage as _;
#[cfg(test)]
use tempfile as _;
#[cfg(test)]
use tokio as _;

mod error;
mod parse;
mod service;
mod sql;
mod types;

pub use error::SearchError;
pub use parse::parse_query;
pub use service::{NoopSearchFanout, Search, SearchFanout};
pub use types::{SearchHit, SearchQuery};
