//! Public-link tokens: 15-char `[A-Za-z0-9]` strings (~89 bits entropy).
//!
//! Matches Nextcloud's token format byte-for-byte so existing desktop/mobile
//! clients accept the URLs without modification.

use rand::Rng;
use std::fmt;

const ALPHABET: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
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
