//! `UserId` newtype + `User` struct.

use crate::email::Email;
use crate::error::UsersError;
use serde::{Deserialize, Serialize};

/// Validated user identifier. 1-64 chars, `[A-Za-z0-9._@-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Result<Self, UsersError> {
        let s = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(UsersError::InvalidUid(format!("length {}", s.len())));
        }
        for ch in s.chars() {
            if !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '@' | '-') {
                return Err(UsersError::InvalidUid(format!("char {:?}", ch)));
            }
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Public user record. Note: the password hash is NOT a field here.
/// `UserStore::lookup_for_auth` returns hash + user together; everything else
/// returns this struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub uid: UserId,
    pub display_name: String,
    pub email: Option<Email>,
    pub enabled: bool,
    pub last_seen: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_uids_accepted() {
        for ok in &["alice", "bob.smith", "user_123", "a-b", "x@y", "A"] {
            assert!(UserId::new(*ok).is_ok(), "{ok:?} should be valid");
        }
    }

    #[test]
    fn invalid_uids_rejected() {
        for bad in &["", " alice", "alice ", "a/b", "a\\b", "a\nb", "a:b"] {
            assert!(UserId::new(*bad).is_err(), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn uid_max_length_64() {
        let ok = "a".repeat(64);
        assert!(UserId::new(&ok).is_ok());
        let bad = "a".repeat(65);
        assert!(UserId::new(&bad).is_err());
    }
}
