//! File-metadata search service for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-search-design.md`.
//!
//! Public entry points: [`Search`] (query + write API), [`SearchFanout`]
//! (trait used by `crabcloud-sharing` to drive share-lifecycle fan-out),
//! and the value types in [`types`]. SQL dispatch mirrors the
//! `crabcloud-activity` / `crabcloud-versions` pattern.

mod error;
mod parse;
mod service;
mod sql;
mod types;

pub use error::SearchError;
pub use parse::parse_query;
pub use service::{NoopSearchFanout, Search, SearchFanout};
pub use types::{SearchHit, SearchQuery};
