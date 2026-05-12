//! Signed-cookie encode/decode. The cookie value format is:
//!
//! ```text
//! <base64url(payload_bytes)>.<base64url(hmac)>
//! ```
//!
//! HMAC is HMAC-SHA256 keyed by `config.secret`, computed over the raw
//! payload bytes. Verification is constant-time via `subtle`. Payload bytes
//! are the raw token (URL-safe base64 from `crabcloud_users::RawToken`), but
//! the codec doesn't know or care.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors produced while decoding a signed session cookie.
#[derive(Debug, Error)]
pub enum CookieError {
    /// The cookie value did not match the expected `<payload>.<sig>` shape.
    #[error("cookie value is not a valid signed-session token")]
    Malformed,
    /// The HMAC over the payload did not verify against the configured secret.
    #[error("cookie signature mismatch")]
    BadSignature,
}

/// Encode an opaque payload into the signed cookie value
/// `<base64url(payload)>.<base64url(hmac)>`.
pub fn encode_cookie(payload: &str, secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
}

/// Verify and decode a signed cookie produced by [`encode_cookie`]. Returns
/// the opaque payload as a UTF-8 string on success.
pub fn decode_cookie(raw: &str, secret: &[u8]) -> Result<String, CookieError> {
    let (payload_b64, sig_b64) = raw.split_once('.').ok_or(CookieError::Malformed)?;
    let payload = B64
        .decode(payload_b64)
        .map_err(|_| CookieError::Malformed)?;
    let sig = B64.decode(sig_b64).map_err(|_| CookieError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let expected = mac.finalize().into_bytes();
    if expected.ct_eq(&sig).into() {
        String::from_utf8(payload).map_err(|_| CookieError::Malformed)
    } else {
        Err(CookieError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_generic_payload() {
        let payload = "some-arbitrary-token-not-just-hex.with-symbols_and-stuff";
        let token = encode_cookie(payload, b"shhh-its-a-secret");
        let decoded = decode_cookie(&token, b"shhh-its-a-secret").unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn wrong_secret_fails_verification() {
        let payload = "abc.def";
        let token = encode_cookie(payload, b"key-a");
        let err = decode_cookie(&token, b"key-b").unwrap_err();
        assert!(matches!(err, CookieError::BadSignature));
    }

    #[test]
    fn malformed_value_is_rejected() {
        let err = decode_cookie("no-dot", b"key").unwrap_err();
        assert!(matches!(err, CookieError::Malformed));
        let err2 = decode_cookie("xx.yy", b"key").unwrap_err();
        assert!(matches!(
            err2,
            CookieError::Malformed | CookieError::BadSignature
        ));
    }
}
