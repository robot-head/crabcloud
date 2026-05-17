//! Public-facing value types for the versions service.

use serde::{Deserialize, Serialize};

/// A single row in `oc_files_versions`. Returned from
/// [`crate::Versions::list_for`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionEntry {
    pub id: i64,
    pub storage_id: i64,
    pub fileid: i64,
    pub user: String,
    pub path: String,
    /// Unix seconds at snapshot time. Matches the on-disk suffix
    /// `<path>.v<version_mtime>`.
    pub version_mtime: i64,
    pub size: i64,
}
