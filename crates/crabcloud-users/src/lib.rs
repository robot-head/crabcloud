//! User store + group store + preference store + password verifier for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-11-users-core-design.md`.

mod email;
mod error;
mod group;
mod user;

pub use email::Email;
pub use error::{UsersError, UsersResult};
pub use group::{Group, GroupId};
pub use user::{User, UserId};
