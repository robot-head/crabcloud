//! Signed-cookie encode/decode. The cookie value format is:
//!
//! ```text
//! <base64url(session_id_bytes)>.<base64url(hmac)>
//! ```
//!
//! HMAC is HMAC-SHA256 keyed by `config.secret`, computed over the raw
//! session-ID bytes. Verification is constant-time via `subtle`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors produced while decoding a signed session cookie.
#[derive(Debug, Error)]
pub enum CookieError {
    /// The cookie value did not match the expected `<id>.<sig>` shape.
    #[error("cookie value is not a valid signed-session token")]
    Malformed,
    /// The HMAC over the session id did not verify against the configured secret.
    #[error("cookie signature mismatch")]
    BadSignature,
}

/// Encode a hex-form session id into the signed cookie value
/// `<base64url(id)>.<base64url(hmac)>`.
pub fn encode_cookie(session_id_hex: &str, secret: &[u8]) -> String {
    let id_bytes = hex::decode(session_id_hex).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&id_bytes);
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", B64.encode(&id_bytes), B64.encode(sig))
}

/// Verify and decode a signed cookie produced by [`encode_cookie`]. Returns
/// the hex-form session id on success.
pub fn decode_cookie(raw: &str, secret: &[u8]) -> Result<String, CookieError> {
    let (id_b64, sig_b64) = raw.split_once('.').ok_or(CookieError::Malformed)?;
    let id_bytes = B64.decode(id_b64).map_err(|_| CookieError::Malformed)?;
    let sig = B64.decode(sig_b64).map_err(|_| CookieError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&id_bytes);
    let expected = mac.finalize().into_bytes();
    if expected.ct_eq(&sig).into() {
        Ok(hex::encode(&id_bytes))
    } else {
        Err(CookieError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_id() {
        let id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let secret = b"shhh-its-a-secret";
        let token = encode_cookie(id, secret);
        let decoded = decode_cookie(&token, secret).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn wrong_secret_fails_verification() {
        let id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let token = encode_cookie(id, b"key-a");
        let err = decode_cookie(&token, b"key-b").unwrap_err();
        assert!(matches!(err, CookieError::BadSignature));
    }

    #[test]
    fn malformed_value_is_rejected() {
        let err = decode_cookie("no-dot", b"key").unwrap_err();
        assert!(matches!(err, CookieError::Malformed));
        let err2 = decode_cookie("xx.yy", b"key").unwrap_err();
        // Either malformed base64 or bad signature once decoded — both are
        // safe rejections; our implementation returns Malformed when b64 fails.
        assert!(matches!(
            err2,
            CookieError::Malformed | CookieError::BadSignature
        ));
    }
}
