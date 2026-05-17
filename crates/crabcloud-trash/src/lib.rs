//! Trash bin service for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-trash-bin-design.md`.
//!
//! Public entry points are [`Trash`] (CRUD operations) and the value
//! types in [`types`]. SQL dispatch is multidialect via
//! `match self.pool.as_ref()` mirroring `crabcloud-sharing`.

mod error;
mod service;
mod sql;
mod types;

pub use error::TrashError;
pub use service::Trash;
pub use types::{RestoredTo, TrashEntry, TrashType};
