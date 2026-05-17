//! Public-facing value types for the trash service.

use serde::{Deserialize, Serialize};

/// A single row in `oc_files_trash`. Returned from [`crate::Trash::list`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrashEntry {
    pub id: i64,
    pub user: String,
    /// Original basename without the suffix, e.g. "report.pdf".
    pub basename: String,
    /// On-disk suffix portion, e.g. "d1716000000" (or "d1716000000_2"
    /// on collision). Combined with `basename` gives the file's name
    /// inside the user's `files_trashbin/files/` directory.
    pub suffix: String,
    /// Original parent dir at delete time, e.g. "/projects/q1". "/" for root.
    pub location: String,
    /// Unix seconds at delete time.
    pub deleted_at: i64,
    pub r#type: TrashType,
    /// Best-effort: the `oc_filecache.fileid` of the file pre-delete.
    /// Populated when the source row was findable; `None` otherwise.
    pub fileid_legacy: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrashType {
    File,
    Dir,
}

impl TrashType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrashType::File => "file",
            TrashType::Dir => "dir",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "dir" => Some(Self::Dir),
            _ => None,
        }
    }
}

/// Returned from [`crate::Trash::restore`]. Holds the path the file was
/// actually restored to (may differ from the original `location/basename`
/// if the caller passed an explicit destination, or if the original name
/// collided and the service appended ` (restored)`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoredTo {
    pub path: String,
}
