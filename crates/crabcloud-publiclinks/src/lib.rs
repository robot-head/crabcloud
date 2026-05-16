//! Public link infrastructure: tokens, passwords, unlock cookies, rate limiting.
//!
//! Spec: `docs/superpowers/specs/2026-05-15-public-links-design.md`.
//!
//! This crate is intentionally db-agnostic and storage-agnostic. The DB lookup
//! for tokens is delegated back to `crabcloud-sharing::Shares::resolve_by_token`
//! (passed in via a small trait), keeping the dependency arrows clean.

mod error;
mod tokens;

pub use error::PublicLinkError;
pub use tokens::{Token, Tokens};
