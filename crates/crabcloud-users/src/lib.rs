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

// Workspace deps consumed by later batches in this sub-project. Keeping them
// listed here lets each batch add the needed code without a Cargo.toml churn
// commit, but the lint requires we acknowledge them until first use.
use async_trait as _;
use bcrypt as _;
use sqlx as _;
use tracing as _;

#[cfg(test)]
mod test_deps {
    use crabcloud_config as _;
    use tempfile as _;
    use tokio as _;
}
