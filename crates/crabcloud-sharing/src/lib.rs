//! User and group sharing for Crabcloud.
//!
//! Schema lives in `migrations/core/0006_shares`. Design spec:
//! `docs/superpowers/specs/2026-05-13-sharing-user-group-and-virtual-mount-design.md`.

// Test target pulls in dev-only deps (e.g. `crabcloud-config` for
// `test_support`, `crabcloud-core` for `AppState` fixtures, `tempfile`) that
// aren't referenced from the library crate proper. The first integration
// tests land in Batch B; until then, silence the workspace
// `unused_crate_dependencies` lint for the test build.
#![cfg_attr(test, allow(unused_crate_dependencies))]

mod error;
mod mail;
mod permissions;
mod service;
mod sql;
mod types;

pub use error::ShareError;
pub use mail::{MailEnqueueError, MailEnqueuer, NullEnqueuer};
pub use permissions::SharePermissions;
pub use service::Shares;
pub use types::{
    CreateShareRequest, ExpiringLink, ItemType, ShareRow, ShareType, UpdateShareFields,
};

// Anchors for crates whose first real call site lands in later tasks/batches
// (e.g. `async-trait` traits in Batch B; `crabcloud-storage` integrations in
// Batch C). Keeps the workspace-wide `unused_crate_dependencies` lint quiet
// without losing the manifest entries.
use anyhow as _;
use crabcloud_storage as _;
use tokio as _;
