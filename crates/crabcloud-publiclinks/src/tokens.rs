//! Public-link tokens: 15-char `[A-Za-z0-9]` strings (~89 bits entropy).
//!
//! Matches Nextcloud's token format byte-for-byte so existing desktop/mobile
//! clients accept the URLs without modification.

use async_trait::async_trait;
use rand::Rng;
use std::fmt;

const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const TOKEN_LEN: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Self {
        let mut buf = [0u8; TOKEN_LEN];
        let mut entropy = [0u8; TOKEN_LEN];
        rand::rng().fill_bytes(&mut entropy);
        for (i, b) in entropy.iter().enumerate() {
            buf[i] = ALPHABET[(*b as usize) % ALPHABET.len()];
        }
        // SAFETY: every byte chosen from `ALPHABET`, which is ASCII.
        Token(String::from_utf8(buf.to_vec()).expect("alphabet is ASCII"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate the *shape* of an incoming token string. Returns `None` if the
    /// string isn't a plausible token; this short-circuits DB lookups for
    /// random garbage path segments.
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != TOKEN_LEN {
            return None;
        }
        if !s.bytes().all(|b| ALPHABET.contains(&b)) {
            return None;
        }
        Some(Token(s.to_string()))
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Facade used by callers. `Shares` owns the actual DB lookup; this trait
/// keeps `crabcloud-publiclinks` independent of sharing internals.
pub struct Tokens;

impl Tokens {
    pub fn new() -> Self {
        Self
    }
    pub fn generate(&self) -> Token {
        Token::generate()
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal projection of a public-link share row, decoupled from the sharing
/// crate's `ShareRow`. The auth layer needs only these fields; keeping the
/// trait small lets us implement it from a stub in tests and from
/// `crabcloud-core::SharesTokenLookup` in production.
#[derive(Debug, Clone)]
pub struct LinkRow {
    /// `oc_share.id` for the link.
    pub share_id: i64,
    /// Owner of the linked subtree (`oc_share.uid_owner`).
    pub owner_uid: String,
    /// Full path inside the owner's home (`oc_share.file_target` for link
    /// rows, per Batch B). Includes a leading `/`; the auth layer strips it
    /// before constructing a `StoragePath`.
    pub owner_path: String,
    /// Stored permission bits. Caller-side wrappers normalise via
    /// `SharePermissions::from_wire` so the re-share bit is dropped.
    pub permissions: u32,
    /// Bcrypt hash from `oc_share.password`, or `None` for unprotected links.
    pub password_hash: Option<String>,
    /// Hard expiration; the auth layer treats past-expiration as 404
    /// (indistinguishable from a missing token).
    pub expiration: Option<chrono::DateTime<chrono::Utc>>,
}

/// What the auth layer needs from the sharing service. Implemented in
/// production by `crabcloud-core::SharesTokenLookup` (a thin adapter over
/// `crabcloud-sharing::Shares::resolve_by_token`) and in tests by a stub.
/// Keeping this trait in `crabcloud-publiclinks` is what lets the crate
/// stay free of any direct dep on the sharing service in its public API.
#[async_trait]
pub trait TokenLookup: Send + Sync {
    /// Resolve a public-link token to its row, or `Ok(None)` if the token is
    /// unknown. The trait returns `std::io::Error` so adapters can flatten
    /// any backend-specific error type into a single shape without dragging
    /// their error enum into this crate.
    async fn lookup(&self, token: &str) -> Result<Option<LinkRow>, std::io::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_token_has_correct_length_and_charset() {
        let t = Token::generate();
        assert_eq!(t.as_str().len(), TOKEN_LEN);
        for b in t.as_str().bytes() {
            assert!(ALPHABET.contains(&b), "byte {b} not in alphabet");
        }
    }

    #[test]
    fn ten_thousand_tokens_are_unique() {
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            assert!(seen.insert(Token::generate().0));
        }
    }

    #[test]
    fn parse_accepts_well_formed() {
        let t = Token::generate();
        assert_eq!(Token::parse(t.as_str()).unwrap(), t);
    }

    #[test]
    fn parse_rejects_wrong_length() {
        assert!(Token::parse("short").is_none());
        assert!(Token::parse("waytoolongforthistokentokentokentoken").is_none());
    }

    #[test]
    fn parse_rejects_invalid_chars() {
        // 15 chars but contains `_`
        assert!(Token::parse("ABC_DEFGHIJKLMN").is_none());
        // 15 chars but contains `+`
        assert!(Token::parse("ABC+DEFGHIJKLMN").is_none());
    }
}
