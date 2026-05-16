//! Unlock cookie format: base64url(`exp_unix_le_8 || hmac_sha256(secret, token || exp_unix_le_8)`).
//!
//! Cookie name is `pl_<token>`. Scope: `Path=/`, `HttpOnly`, `Secure` (prod),
//! `SameSite=Lax`, `Max-Age=3600`. The MAC binds the cookie to *both* the
//! token (so cookies don't cross-link) and the expiry (so a captured cookie
//! can't be replayed forever).

use crate::error::PublicLinkError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct UnlockCookie;

impl UnlockCookie {
    pub const COOKIE_NAME_PREFIX: &'static str = "pl_";

    pub fn cookie_name_for(token: &str) -> String {
        format!("{}{}", Self::COOKIE_NAME_PREFIX, token)
    }

    /// Build the cookie value for `token` valid until `exp_unix`.
    pub fn sign(secret: &[u8], token: &str, exp_unix: i64) -> String {
        let mut payload = Vec::with_capacity(8 + 32);
        payload.extend_from_slice(&exp_unix.to_le_bytes());
        let tag = {
            let mut mac =
                HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
            mac.update(token.as_bytes());
            mac.update(&exp_unix.to_le_bytes());
            mac.finalize().into_bytes()
        };
        payload.extend_from_slice(&tag);
        URL_SAFE_NO_PAD.encode(payload)
    }

    /// Returns `Ok(exp_unix)` on a valid, unexpired-at-now cookie.
    pub fn verify(
        secret: &[u8],
        token: &str,
        cookie_value: &str,
        now_unix: i64,
    ) -> Result<i64, PublicLinkError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(cookie_value)
            .map_err(|_| PublicLinkError::InvalidCookie)?;
        if bytes.len() != 8 + 32 {
            return Err(PublicLinkError::InvalidCookie);
        }
        let mut exp_bytes = [0u8; 8];
        exp_bytes.copy_from_slice(&bytes[..8]);
        let exp_unix = i64::from_le_bytes(exp_bytes);
        if exp_unix < now_unix {
            return Err(PublicLinkError::InvalidCookie);
        }
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(token.as_bytes());
        mac.update(&exp_bytes);
        mac.verify_slice(&bytes[8..])
            .map_err(|_| PublicLinkError::InvalidCookie)?;
        Ok(exp_unix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"32-byte-secret-for-tests--------";

    #[test]
    fn round_trip_verifies() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        assert_eq!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_000).unwrap(),
            2_000_000_000
        );
    }

    #[test]
    fn expired_cookie_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 1_000_000_000);
        let err = UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_001);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn tampered_mac_rejected() {
        let mut v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        // Flip a bit in the MAC region (last base64url char).
        let last = v.pop().unwrap();
        let flipped = if last == 'A' { 'B' } else { 'A' };
        v.push(flipped);
        let err = UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_000);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn wrong_token_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        let err = UnlockCookie::verify(SECRET, "ZZZZZZZZZZZZZZZ", &v, 1_000_000_000);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn wrong_secret_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        let err = UnlockCookie::verify(
            b"different-secret-for-tests------",
            "ABCDEFGHIJKLMNO",
            &v,
            1_000_000_000,
        );
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn garbage_input_rejected() {
        assert!(matches!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", "not-base64@@", 1_000_000_000),
            Err(PublicLinkError::InvalidCookie)
        ));
        assert!(matches!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", "AAAA", 1_000_000_000),
            Err(PublicLinkError::InvalidCookie)
        ));
    }

    #[test]
    fn cookie_name_includes_token() {
        assert_eq!(UnlockCookie::cookie_name_for("ABC"), "pl_ABC");
    }
}
