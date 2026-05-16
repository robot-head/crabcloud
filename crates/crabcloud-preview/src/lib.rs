//! On-demand thumbnail generation for image and PDF source files.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-previews-design.md`.
//!
//! Public entry point will be `PreviewCache::get_or_render`; providers
//! dispatch by source mime through `provider_for_mime`. This file lands
//! incrementally across the Batch A tasks; for now only the error type
//! is materialized.

mod error;
mod ladder;

pub use error::PreviewError;
pub use ladder::{round_up_to_ladder, LADDER};

// Anchors for dev-deps only referenced from `#[cfg(test)]` modules across
// other files — keeps `unused_crate_dependencies` quiet for the lib test
// binary. Matches the pattern used by `crabcloud-zip/src/lib.rs`.
#[cfg(test)]
use crabcloud_config as _;
#[cfg(test)]
use crabcloud_db as _;
#[cfg(test)]
use crabcloud_filecache as _;
#[cfg(test)]
use crabcloud_users as _;
#[cfg(test)]
use tempfile as _;

// Workspace deps wired in via Cargo.toml for the foundation tasks; real
// call sites land in later tasks. Anchor here so the unused-deps lint
// stays quiet on intermediate commits.
use async_trait as _;
use crabcloud_fs as _;
use crabcloud_storage as _;
use dashmap as _;
use hayro as _;
use image as _;
use tokio as _;
use tokio_util as _;
use tracing as _;
