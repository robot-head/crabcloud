//! Session machinery: data model, cache-backed store, signed cookie, and the
//! Tower layer that ties them together.
//!
//! See spec §7.3.

mod cookie;
mod data;
mod layer;
mod store;

pub use cookie::{decode_cookie, encode_cookie, CookieError};
pub use data::{Session, SessionId};
pub use layer::{PendingCookie, SessionHandle, SessionLayer, COOKIE_NAME};
pub use store::{SessionStore, SESSION_IDLE_TTL};
