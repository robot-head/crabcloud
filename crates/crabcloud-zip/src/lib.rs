//! Streaming folder-zip helper for `crabcloud-http`.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-folder-zip-design.md`.
//!
//! Public entry point is [`stream_folder`]. The helper walks a folder tree
//! via [`crabcloud_fs::View`], enforces operator-configurable [`ZipCaps`],
//! then streams a zip archive into an [`tokio::io::AsyncWrite`] sink.
//! Compression is picked per-entry from the file's mime type
//! ([`compression_for_mime`]). Filename encoding uses UTF-8
//! (general-purpose bit 11) plus the Info-ZIP Unicode Path extra field
//! on every entry.

mod compression;
mod error;
mod http;
mod mpsc_writer;
mod stream;
mod types;
mod walk;

pub use compression::compression_for_mime;
pub use error::{WalkError, ZipError};
pub use http::zip_response_headers;
pub use mpsc_writer::MpscBytesWriter;
pub use stream::stream_folder;
pub use types::{OverCapBody, OverCapLimits, PlanKind, PlannedEntry, ZipCaps, ZipPlan, ZipSummary};
pub use walk::{root_basename, walk_for_caps};

// Anchors for dev-deps only referenced from `#[cfg(test)]` modules across
// other files — keeps `unused_crate_dependencies` quiet for the lib test
// binary. Matches the pattern used by `crabcloud-publiclinks/src/lib.rs`.
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
