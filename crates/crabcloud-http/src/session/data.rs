//! Session payload types. Stored in cache as JSON.

use serde::{Deserialize, Serialize};

/// Opaque session ID. 32 random bytes, hex-encoded for storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Generate a fresh random session id (32 bytes, hex-encoded).
    pub fn new_random() -> Self {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        SessionId(hex::encode(buf))
    }

    /// Hex string form of the session id, used for cookies + cache keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Server-side session data. Persisted in cache keyed by `SessionId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Authenticated user ID, if any.
    pub user_id: Option<String>,
    /// CSRF request token. Rotated at login/logout.
    pub csrf_token: String,
    /// Last access timestamp (seconds since epoch). Used for sliding TTL.
    pub last_activity: u64,
    /// True once the user has cleared 2FA for this session. `#[serde(default)]`
    /// keeps backwards compatibility with sessions cached before this field
    /// existed.
    #[serde(default)]
    pub two_factor_passed: bool,
}

impl Default for Session {
    /// Delegate to [`Session::new`] so a defaulted session has a fresh random
    /// CSRF token — never the empty string. A `derive(Default)` would leave
    /// `csrf_token` empty, which collapses CSRF protection (an empty header
    /// would compare equal to an empty expected value).
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Create a fresh session with a random CSRF token and `last_activity` set
    /// to the current time.
    pub fn new() -> Self {
        Self {
            user_id: None,
            csrf_token: random_token(),
            last_activity: now_secs(),
            two_factor_passed: false,
        }
    }

    /// Replace the CSRF token with a fresh random value. Called at login/logout.
    pub fn rotate_csrf(&mut self) {
        self.csrf_token = random_token();
    }
}

fn random_token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_64_hex_chars() {
        let id = SessionId::new_random();
        assert_eq!(id.0.len(), 64);
        assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ids_differ_on_each_call() {
        let a = SessionId::new_random();
        let b = SessionId::new_random();
        assert_ne!(a, b);
    }

    #[test]
    fn new_session_has_token_and_no_user() {
        let s = Session::new();
        assert!(s.user_id.is_none());
        assert_eq!(s.csrf_token.len(), 64);
    }

    #[test]
    fn rotate_csrf_changes_token() {
        let mut s = Session::new();
        let before = s.csrf_token.clone();
        s.rotate_csrf();
        assert_ne!(s.csrf_token, before);
    }

    #[test]
    fn default_session_has_random_csrf_token() {
        let s = Session::default();
        assert!(!s.csrf_token.is_empty());
        assert_eq!(s.csrf_token.len(), 64);
    }
}
