//! Session payload types. Stored in cache as JSON.

use serde::{Deserialize, Serialize};

/// Opaque session ID. 32 random bytes, hex-encoded for storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new_random() -> Self {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        SessionId(hex::encode(buf))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Server-side session data. Persisted in cache keyed by `SessionId`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// Authenticated user ID, if any.
    pub user_id: Option<String>,
    /// CSRF request token. Rotated at login/logout.
    pub csrf_token: String,
    /// Last access timestamp (seconds since epoch). Used for sliding TTL.
    pub last_activity: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            user_id: None,
            csrf_token: random_token(),
            last_activity: now_secs(),
        }
    }

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
}
