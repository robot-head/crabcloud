//! Public types for the streaming-zip helper.

use crabcloud_storage::StoragePath;
use serde::Serialize;
use std::time::SystemTime;

/// Operator-tunable size caps. Pre-flight walk rejects anything over.
#[derive(Debug, Clone, Copy)]
pub struct ZipCaps {
    pub max_entries: u64,
    pub max_bytes: u64,
}

impl ZipCaps {
    /// Sensible defaults matching `FileConfig`'s defaults.
    pub fn defaults() -> Self {
        Self {
            max_entries: 500,
            max_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// What kind of entry a [`PlannedEntry`] represents in the zip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    File,
    Dir,
}

/// One entry the walker decided to include. `zip_name` is the path inside
/// the zip archive (always `/`-separated, no leading slash; directory
/// names are stored without a trailing `/` and `add_directory` in the
/// writer appends it).
#[derive(Debug, Clone)]
pub struct PlannedEntry {
    pub storage_path: StoragePath,
    pub zip_name: String,
    pub kind: PlanKind,
    pub size: u64,
    pub mtime: SystemTime,
    pub mime: String,
}

#[derive(Debug, Clone)]
pub struct ZipPlan {
    pub entries: Vec<PlannedEntry>,
    pub total_bytes: u64,
}

/// Returned by [`crate::stream_folder`] on success.
#[derive(Debug, Clone, Copy)]
pub struct ZipSummary {
    pub entries: u64,
    pub bytes: u64,
}

/// JSON body for HTTP 413 (folder too large) responses on either the authed
/// or public-link zip surface. Shared so both handlers serialise the same
/// shape; see `crates/crabcloud-http/src/routes/files_zip.rs` and
/// `crates/crabcloud-http/src/routes/public_link/mod.rs`.
#[derive(Debug, Clone, Serialize)]
pub struct OverCapBody {
    pub error: &'static str,
    pub entries: u64,
    pub bytes: u64,
    pub limits: OverCapLimits,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverCapLimits {
    pub max_entries: u64,
    pub max_bytes: u64,
}

impl OverCapBody {
    /// Convenience constructor. `count`/`bytes` come from a
    /// [`crate::WalkError::TooLarge`] arm; `caps` echoes the configured
    /// limits so callers can render both the overflow and the policy in
    /// the same payload.
    pub fn for_too_large(count: u64, bytes: u64, caps: ZipCaps) -> Self {
        Self {
            error: "folder too large",
            entries: count,
            bytes,
            limits: OverCapLimits {
                max_entries: caps.max_entries,
                max_bytes: caps.max_bytes,
            },
        }
    }
}
