//! On-demand thumbnail generation for image and PDF source files.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-previews-design.md`.
//!
//! Public entry point is [`PreviewCache::get_or_render`]. Providers
//! ([`ImageProvider`], [`PdfProvider`]) dispatch by source mime through
//! [`provider_for_mime`]. Output is always JPEG (quality 80).

mod cache;
mod error;
mod ladder;
mod provider;
mod providers;

pub use cache::PreviewCache;
pub use error::PreviewError;
pub use ladder::{round_up_to_ladder, LADDER};
pub use provider::{provider_for_mime, PreviewProvider, ProviderResult};
pub use providers::{ImageProvider, PdfProvider};

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

// Workspace deps that the foundation crate doesn't itself call yet but
// will once Batch B wires up `View::read` + axum streaming. Anchored here
// so the `unused_crate_dependencies` lint stays quiet until then.
use crabcloud_fs as _;
use crabcloud_storage as _;
use tokio_util as _;
use tracing as _;
