//! File versioning service for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-file-versioning-design.md`.
//!
//! Public entry points are [`Versions`] and the value types in [`types`].
//! SQL dispatch is multidialect via `match self.pool.as_ref()` mirroring
//! `crabcloud-trash` / `crabcloud-sharing`.

mod error;
mod service;
mod sql;
mod types;

pub use error::VersionsError;
pub use service::Versions;
pub use types::VersionEntry;
