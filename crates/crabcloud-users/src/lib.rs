//! User store + group store + preference store + password verifier for Crabcloud.
//!
//! See `docs/superpowers/specs/2026-05-11-users-core-design.md`.

// Integration tests under `tests/` pull in axum / crabcloud-core / crabcloud-http /
// serde_json / tower as dev-deps. Those deps are not referenced from the library
// crate proper, so the `unused-crate-dependencies` lint complains when the lib's
// own test target is built. Silence it for test builds only.
#![cfg_attr(test, allow(unused_crate_dependencies))]

mod app_password;
mod auth_token;
pub mod cli;
mod email;
mod error;
mod group;
mod password;
mod service;
mod store;
mod user;

pub use app_password::AppPasswordService;
pub use auth_token::{hash_token, AuthToken, AuthTokenType, RawToken};
pub use email::Email;
pub use error::{UsersError, UsersResult};
pub use group::{Group, GroupId};
pub use password::{BcryptVerifier, PasswordVerifier, BCRYPT_COST};
pub use service::UsersService;
pub use store::auth_token::{SqlTokenStore, TokenAuthCache, TokenStore};
pub use store::bootstrap_shim::BootstrapAdminBackend;
pub use store::sql::{SqlGroupStore, SqlPreferenceStore, SqlUserStore};
pub use store::{
    GroupListFilter, GroupStore, PreferenceStore, UserListFilter, UserStore, UserWithHash,
};
pub use user::{User, UserId};
