//! Public types for the streaming-zip helper.

use crabcloud_storage::StoragePath;
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
