//! User store + group store + preference store + password verifier for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-11-users-core-design.md`.

mod email;
mod error;
mod group;
mod password;
mod service;
mod store;
mod user;

pub use email::Email;
pub use error::{UsersError, UsersResult};
pub use group::{Group, GroupId};
pub use password::{BcryptVerifier, PasswordVerifier, BCRYPT_COST};
pub use service::UsersService;
pub use store::bootstrap_shim::BootstrapAdminBackend;
pub use store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
pub use store::{GroupStore, PreferenceStore, UserStore, UserWithHash};
pub use user::{User, UserId};
