//! Internationalization for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.2.

mod catalog;
mod locale;
mod service;

pub use catalog::{load_all, Catalog, CatalogError};
pub use locale::{resolve, Locale};
pub use service::I18n;
