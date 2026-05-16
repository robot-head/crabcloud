//! Public link infrastructure: tokens, passwords, unlock cookies, rate limiting.
//!
//! Spec: `docs/superpowers/specs/2026-05-15-public-links-design.md`.
//!
//! This crate is intentionally db-agnostic and storage-agnostic. The DB lookup
//! for tokens is delegated back to `crabcloud-sharing::Shares::resolve_by_token`
//! (passed in via a small trait), keeping the dependency arrows clean.

mod cookie;
mod error;
mod passwords;
mod ratelimit;
mod tokens;

pub use cookie::UnlockCookie;
pub use error::PublicLinkError;
pub use passwords::{HashedPassword, Passwords};
pub use ratelimit::{RateLimitDecision, RateLimiter};
pub use tokens::{Token, Tokens};
