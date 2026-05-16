//! Password hashing for public-link passwords. Uses bcrypt (cost 12) to mirror
//! `crabcloud-users::password::BcryptVerifier` so operational cost is consistent
//! across the codebase.
//!
//! Bcrypt has a 72-byte plaintext cap; longer inputs are rejected at hash time
//! rather than silently truncated.

use crate::error::PublicLinkError;
pub use crabcloud_users::BCRYPT_COST;

/// Bcrypt has a hard plaintext cap of 72 bytes; we reject longer inputs at
/// hash time rather than truncate silently.
pub const MAX_PASSWORD_BYTES: usize = 72;

/// A stored bcrypt hash. The `String` is the `$2b$...` / `$2y$...` hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedPassword(String);

impl HashedPassword {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wrap an already-persisted hash without revalidating it. Verification
    /// will reject malformed strings as non-matching rather than erroring,
    /// so a corrupt row in the DB doesn't leak via the error response shape.
    pub fn from_stored(s: String) -> Self {
        Self(s)
    }
}

pub struct Passwords;

impl Passwords {
    pub fn new() -> Self {
        Self
    }

    pub fn hash(&self, plaintext: &str) -> Result<HashedPassword, PublicLinkError> {
        if plaintext.len() > MAX_PASSWORD_BYTES {
            return Err(PublicLinkError::PasswordTooWeak(
                "max 72 bytes (bcrypt limit)",
            ));
        }
        let hashed = bcrypt::hash(plaintext, BCRYPT_COST)?;
        Ok(HashedPassword(hashed))
    }

    /// Constant-time-ish verification (bcrypt's compare is constant-time over
    /// the hash bytes). Returns `false` for an invalid stored-hash format
    /// rather than erroring, so a malformed row doesn't leak via the response
    /// shape.
    pub fn verify(&self, plaintext: &str, hashed: &HashedPassword) -> bool {
        bcrypt::verify(plaintext, hashed.as_str()).unwrap_or(false)
    }
}

impl Default for Passwords {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_matches() {
        let p = Passwords::new();
        let h = p.hash("hunter2").unwrap();
        assert!(p.verify("hunter2", &h));
    }

    #[test]
    fn wrong_password_rejected() {
        let p = Passwords::new();
        let h = p.hash("hunter2").unwrap();
        assert!(!p.verify("hunter3", &h));
    }

    #[test]
    fn empty_password_round_trips() {
        // bcrypt accepts empty plaintext; we keep the same surface for symmetry
        // with the documented public-link password optionality (the caller
        // chooses whether to allow empty before reaching this layer).
        let p = Passwords::new();
        let h = p.hash("").unwrap();
        assert!(p.verify("", &h));
        assert!(!p.verify("anything", &h));
    }

    #[test]
    fn malformed_hash_yields_false() {
        let p = Passwords::new();
        let bad = HashedPassword::from_stored("not-a-real-hash".into());
        assert!(!p.verify("anything", &bad));
    }

    #[test]
    fn distinct_hashes_for_same_password() {
        // Salts differ, so the stored strings should not collide.
        let p = Passwords::new();
        let h1 = p.hash("same").unwrap();
        let h2 = p.hash("same").unwrap();
        assert_ne!(h1, h2);
        assert!(p.verify("same", &h1));
        assert!(p.verify("same", &h2));
    }

    #[test]
    fn stored_hash_uses_bcrypt_prefix() {
        let p = Passwords::new();
        let h = p.hash("hunter2").unwrap();
        let s = h.as_str();
        assert!(
            s.starts_with("$2b$") || s.starts_with("$2y$") || s.starts_with("$2a$"),
            "expected bcrypt prefix, got {s}"
        );
    }

    #[test]
    fn over_72_bytes_rejected() {
        let p = Passwords::new();
        let big = "a".repeat(73);
        let err = p.hash(&big).unwrap_err();
        assert!(matches!(err, PublicLinkError::PasswordTooWeak(_)));
    }
}
