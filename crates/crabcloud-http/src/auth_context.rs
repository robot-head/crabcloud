//! `AuthContext` — the request extension installed by [`crate::middleware::auth::AuthLayer`].
//! Extractors read it instead of `SessionHandle`. Three auth methods
//! (cookie, Bearer, Basic) collapse into one record so handlers don't
//! repeat per-scheme logic.

use crabcloud_users::UserId;

/// How a request was authenticated.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthMethod {
    /// Cookie-backed browser session.
    Session,
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// `Authorization: Basic <b64(uid:token)>`.
    Basic,
}

/// Per-request authentication context. Inserted as a request extension by
/// [`crate::middleware::auth::AuthLayer`] when any of the three auth arms
/// succeed.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// Authenticated user id.
    pub user_id: UserId,
    /// Which arm matched.
    pub method: AuthMethod,
    /// PK of the backing `oc_authtoken` row.
    pub token_id: i64,
    /// What the user typed at login (Session) or the row's `login_name`
    /// (Bearer / Basic).
    pub login_name: String,
    /// `remember` checkbox state at login. Only meaningful for Session tokens.
    pub remember: bool,
}
