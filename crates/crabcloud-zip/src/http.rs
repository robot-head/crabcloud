//! HTTP response-shaping helpers shared by both folder-zip surfaces.
//!
//! Both the authed (`/api/files/zip/...`) and public-link
//! (`/s/{token}/zip/...`) handlers stream a `application/zip` body with the
//! exact same RFC 6266 dual-form `Content-Disposition` header. The helper
//! lives here so a change to the encoding policy lands in one place.

use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};

/// Build the `Content-Type` + `Content-Disposition` headers for a zip
/// download response, using RFC 6266 dual-form filename encoding.
/// `archive_basename` should be the bare name (no `.zip` suffix); the
/// helper appends `.zip` in both forms.
///
/// The ASCII fallback substitutes `_` for any byte outside
/// `[A-Za-z0-9._-]` so the value is always a valid HTTP-header
/// `quoted-string`; the `filename*=UTF-8''…` form carries the original
/// name percent-encoded.
pub fn zip_response_headers(archive_basename: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    let safe_ascii: String = archive_basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let percent = urlencoding::encode(archive_basename);
    let disp = format!("attachment; filename=\"{safe_ascii}.zip\"; filename*=UTF-8''{percent}.zip");
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disp).unwrap_or(HeaderValue::from_static("attachment")),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_basename_round_trips() {
        let h = zip_response_headers("Photos");
        assert_eq!(h.get(CONTENT_TYPE).unwrap(), "application/zip");
        let disp = h.get(CONTENT_DISPOSITION).unwrap().to_str().unwrap();
        assert!(
            disp.contains("filename=\"Photos.zip\""),
            "missing ASCII filename: {disp}"
        );
        assert!(
            disp.contains("filename*=UTF-8''Photos.zip"),
            "missing UTF-8 filename: {disp}"
        );
    }

    #[test]
    fn non_ascii_basename_gets_underscored_and_percent_encoded() {
        let h = zip_response_headers("résumé");
        let disp = h.get(CONTENT_DISPOSITION).unwrap().to_str().unwrap();
        assert!(
            disp.contains("filename=\"r_sum_.zip\""),
            "missing sanitised ASCII filename: {disp}"
        );
        assert!(
            disp.contains("filename*=UTF-8''r%C3%A9sum%C3%A9.zip"),
            "missing percent-encoded UTF-8 filename: {disp}"
        );
    }
}
