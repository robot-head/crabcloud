//! Password hashing + verification.

use crate::error::{UsersError, UsersResult};
use std::sync::OnceLock;

/// bcrypt cost. 12 is the project default; revisit when 13 becomes affordable.
pub const BCRYPT_COST: u32 = 12;

pub trait PasswordVerifier: Send + Sync {
    /// Constant-time-ish verification. If `hash` is None, runs against a
    /// sentinel hash so the call still takes ~equivalent time, defeating
    /// user-enumeration timing oracles.
    fn verify(&self, password: &str, hash: Option<&str>) -> bool;

    fn hash(&self, password: &str) -> UsersResult<String>;
}

pub struct BcryptVerifier;

impl BcryptVerifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BcryptVerifier {
    fn default() -> Self {
        Self::new()
    }
}

fn sentinel() -> &'static str {
    static SENTINEL: OnceLock<String> = OnceLock::new();
    SENTINEL.get_or_init(|| {
        bcrypt::hash(
            "invalid sentinel — never matches a real password",
            BCRYPT_COST,
        )
        .expect("bcrypt::hash on a literal never fails")
    })
}

impl PasswordVerifier for BcryptVerifier {
    fn verify(&self, password: &str, hash: Option<&str>) -> bool {
        let target = hash.unwrap_or_else(|| sentinel());
        bcrypt::verify(password, target).unwrap_or(false)
    }

    fn hash(&self, password: &str) -> UsersResult<String> {
        if password.is_empty() {
            return Err(UsersError::PasswordTooWeak("must not be empty"));
        }
        if password.len() > 72 {
            return Err(UsersError::PasswordTooWeak("max 72 bytes (bcrypt limit)"));
        }
        bcrypt::hash(password, BCRYPT_COST)
            .map_err(|e| UsersError::Internal(anyhow::anyhow!("bcrypt hash failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = BcryptVerifier::new();
        let h = v.hash("hunter2").unwrap();
        assert!(v.verify("hunter2", Some(&h)));
        assert!(!v.verify("WRONG", Some(&h)));
    }

    #[test]
    fn no_hash_always_fails_but_runs() {
        let v = BcryptVerifier::new();
        assert!(!v.verify("anything", None));
    }

    #[test]
    fn empty_password_rejected_on_hash() {
        let v = BcryptVerifier::new();
        let err = v.hash("").unwrap_err();
        assert!(matches!(err, UsersError::PasswordTooWeak(_)));
    }

    #[test]
    fn over_72_bytes_rejected() {
        let v = BcryptVerifier::new();
        let big = "a".repeat(73);
        let err = v.hash(&big).unwrap_err();
        assert!(matches!(err, UsersError::PasswordTooWeak(_)));
    }
}
