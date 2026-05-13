//! User and group sharing for Crabcloud.
//!
//! Schema lives in `migrations/core/0006_shares`. Design spec:
//! `docs/superpowers/specs/2026-05-13-sharing-user-group-and-virtual-mount-design.md`.

mod error;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use permissions::SharePermissions;
pub use types::{CreateShareRequest, ItemType, ShareRow, ShareType, UpdateShareFields};
// `Shares` re-export lands when the service is implemented in Batch B.

// Anchors for crates whose first real call site lands in later tasks/batches
// (e.g. `async-trait` traits in Batch B; `crabcloud-storage` integrations in
// Batch C). Keeps the workspace-wide `unused_crate_dependencies` lint quiet
// without losing the manifest entries.
use anyhow as _;
use async_trait as _;
use crabcloud_storage as _;
use tokio as _;
use tracing as _;
