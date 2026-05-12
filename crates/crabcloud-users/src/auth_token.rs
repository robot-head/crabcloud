//! `oc_authtoken` row type + raw-token generator + hashing helper.

use crate::error::UsersError;
use crate::user::UserId;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

/// Discriminator for [`AuthToken::kind`]. Mapped to the upstream `type`
/// integer column.
#[repr(i32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthTokenType {
    /// Cookie-backed browser session.
    Session = 0,
    /// Long-lived token used via Bearer / Basic auth (DAV / desktop / mobile).
    AppPassword = 1,
}

impl AuthTokenType {
    pub fn from_i32(v: i32) -> Result<Self, UsersError> {
        match v {
            0 => Ok(Self::Session),
            1 => Ok(Self::AppPassword),
            other => Err(UsersError::Internal(anyhow::anyhow!(
                "unknown AuthTokenType discriminator: {other}"
            ))),
        }
    }

    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Persisted token row. Mirrors the full upstream `oc_authtoken` schema; many
/// columns are nullable / always-default in sub-project 2b (E2E key pair,
/// scope, etc.) and are populated by later sub-projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthToken {
    pub id: i64,
    pub uid: UserId,
    pub login_name: String,
    pub password: Option<String>,
    pub name: String,
    /// Hashed token value (128-hex SHA-512 of `raw_token || secret`).
    pub token: String,
    pub kind: AuthTokenType,
    pub remember: bool,
    pub last_activity: u64,
    pub last_check: u64,
    pub public_key: Option<String>,
    pub private_key: Option<String>,
    pub version: i32,
    pub scope: Option<String>,
    pub expires: Option<u64>,
    pub password_invalid: bool,
    pub remote_wipe: bool,
}

impl AuthToken {
    /// True when the row is in a state the auth path must reject.
    pub fn is_unusable(&self, now: u64) -> bool {
        if self.password_invalid || self.remote_wipe {
            return true;
        }
        matches!(self.expires, Some(exp) if exp <= now)
    }
}

/// Raw, plaintext token. Produced once at mint time, displayed to the user
/// once, then discarded. Wrapped in `SecretString` so it never lands in
/// `Debug` / log output.
#[derive(Debug, Clone)]
pub struct RawToken(SecretString);

impl RawToken {
    /// Generate a fresh 72-byte token from `OsRng`, base64-URL-encoded
    /// without padding (~96 ASCII chars). The alphabet is `[A-Za-z0-9_-]`
    /// — URL-safe and safe to embed in HTTP Basic auth.
    pub fn generate() -> Self {
        let mut buf = [0u8; 72];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        Self(SecretString::new(B64.encode(buf).into()))
    }

    /// Construct from an existing string (e.g. read from a Bearer header or
    /// Basic-auth password portion).
    pub fn from_string(s: String) -> Self {
        Self(SecretString::new(s.into()))
    }

    /// Borrow the raw value. Caller MUST NOT log or `Debug`-print the result.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

/// Compute the storage hash for a raw token: lowercase hex of
/// `SHA-512(raw_token_bytes || secret_bytes)`.
pub fn hash_token(raw: &str, secret: &str) -> String {
    use sha2::{Digest, Sha512};
    let mut hasher = Sha512::new();
    hasher.update(raw.as_bytes());
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_token_is_long_and_url_safe() {
        let t = RawToken::generate();
        let s = t.expose();
        assert_eq!(s.len(), 96);
        for c in s.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-URL-safe char {c:?} in token"
            );
        }
    }

    #[test]
    fn raw_token_debug_does_not_leak_value() {
        let t = RawToken::generate();
        let dbg = format!("{t:?}");
        assert!(!dbg.contains(t.expose()), "Debug printed the secret");
    }

    #[test]
    fn raw_tokens_differ_each_call() {
        let a = RawToken::generate();
        let b = RawToken::generate();
        assert_ne!(a.expose(), b.expose());
    }

    #[test]
    fn hash_token_is_deterministic_for_same_inputs() {
        assert_eq!(hash_token("abc", "k"), hash_token("abc", "k"));
    }

    #[test]
    fn hash_token_changes_with_secret() {
        assert_ne!(hash_token("abc", "k1"), hash_token("abc", "k2"));
    }

    #[test]
    fn hash_token_changes_with_input() {
        assert_ne!(hash_token("abc", "k"), hash_token("xyz", "k"));
    }

    #[test]
    fn hash_token_is_128_hex_chars() {
        let h = hash_token("anything", "secret");
        assert_eq!(h.len(), 128);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn auth_token_type_roundtrip() {
        for kind in [AuthTokenType::Session, AuthTokenType::AppPassword] {
            assert_eq!(AuthTokenType::from_i32(kind.as_i32()).unwrap(), kind);
        }
        assert!(AuthTokenType::from_i32(7).is_err());
    }

    #[test]
    fn unusable_detects_expiry_and_flags() {
        let mut row = AuthToken {
            id: 1,
            uid: UserId::new("alice").unwrap(),
            login_name: "alice".into(),
            password: None,
            name: "x".into(),
            token: "h".into(),
            kind: AuthTokenType::Session,
            remember: false,
            last_activity: 0,
            last_check: 0,
            public_key: None,
            private_key: None,
            version: 2,
            scope: None,
            expires: None,
            password_invalid: false,
            remote_wipe: false,
        };
        assert!(!row.is_unusable(1000));
        row.expires = Some(500);
        assert!(row.is_unusable(1000));
        row.expires = Some(2000);
        assert!(!row.is_unusable(1000));
        row.password_invalid = true;
        assert!(row.is_unusable(1000));
        row.password_invalid = false;
        row.remote_wipe = true;
        assert!(row.is_unusable(1000));
    }
}
