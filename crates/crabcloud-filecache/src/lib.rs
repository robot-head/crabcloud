//! `crabcloud-filecache` — DB-backed cache for storage state.
//!
//! Mirrors 4a's storage events in `oc_filecache`/`oc_storages`/`oc_mimetypes`
//! so consumers (sub-project 5's WebDAV, future indexes) can serve `stat`/
//! `list` in O(1). Cache-miss populate happens through real-backend stats
//! under a per-path lock. Ancestor `size` + `etag` propagation runs in one
//! DB transaction per event — matches upstream Nextcloud behavior so desktop
//! sync clients see byte-identical ETags at every level.

pub mod error;
pub mod mimetypes;
pub mod schema;
pub mod storages;

pub use error::{FileCacheError, FileCacheResult};
pub use schema::{path_hash, type_half, FilecacheRow, FilecacheRowRaw, DIRECTORY_MIMETYPE};

// Batches B–E will add:
//   pub mod populate;
//   pub mod propagate;
//   pub mod scanner;
//   pub struct FileCache { ... }
//   pub struct Scanner { ... }
//
// The following workspace deps are declared on Batch A's Cargo.toml so the
// crate's manifest is stable across batches — they pick up real call sites
// in Batches B–D. Anchor them here to keep `unused_crate_dependencies`
// (workspace-wide `-D warnings`) quiet.
use async_trait as _;
use crabcloud_cache as _;
use crabcloud_config as _;
use tokio as _;
use tracing as _;
