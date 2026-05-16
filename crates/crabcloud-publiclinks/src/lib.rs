//! Public link infrastructure: tokens, passwords, unlock cookies, rate limiting,
//! and the axum auth middleware that ties them together.
//!
//! Spec: `docs/superpowers/specs/2026-05-15-public-links-design.md`.
//!
//! This crate is intentionally db-agnostic. The DB lookup for tokens is
//! delegated back to `crabcloud-sharing::Shares::resolve_by_token` via the
//! `TokenLookup` trait (adapter lives in `crabcloud-core`), keeping the
//! dependency arrows clean and the auth-layer tests stub-driven.

mod auth_layer;
mod context;
mod cookie;
mod error;
mod passwords;
mod ratelimit;
mod tokens;

pub use auth_layer::{
    public_link_auth, AuthSurface, PasswordGateRequired, PublicLinkAuthState,
};
pub use context::PublicLinkAuthContext;
pub use cookie::UnlockCookie;
pub use error::PublicLinkError;
pub use passwords::{HashedPassword, Passwords};
pub use ratelimit::{RateLimitDecision, RateLimiter};
pub use tokens::{LinkRow, Token, TokenLookup, Tokens};
