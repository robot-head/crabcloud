//! Client-side allowlist mirroring `crabcloud_preview::provider_for_mime`.
//! Used by `FileRow` and the public-link `PublicRow` to decide whether to
//! render an inline `<img>` thumbnail (`is_previewable_mime == true`) or
//! keep the generic file icon.
//!
//! Kept in sync with `crabcloud-preview::provider::provider_for_mime`:
//! the server-side check authoritative — if the UI misclassifies a mime as
//! previewable the request will 415 and the `onerror` fallback restores
//! the generic icon. The duplication avoids pulling the entire
//! `crabcloud-preview` crate (and its `image` + `hayro` deps) into the
//! wasm32 build graph.

/// Returns `true` for mimes the preview backend can render. Mirrors the
/// server's `provider_for_mime` matchers byte-for-byte: case-insensitive
/// prefix match against `image/jpeg`, `image/png`, `image/gif`,
/// `image/webp`, `application/pdf`. Other mimes (including `image/svg+xml`
/// and `image/heic`) collapse to `false`.
pub fn is_previewable_mime(mime: &str) -> bool {
    let lc = mime.to_ascii_lowercase();
    lc.starts_with("image/jpeg")
        || lc.starts_with("image/png")
        || lc.starts_with("image/gif")
        || lc.starts_with("image/webp")
        || lc.starts_with("application/pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previewable_mimes_match_server_allowlist() {
        for mime in &[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "application/pdf",
            "IMAGE/JPEG",
            "image/jpeg; charset=binary",
        ] {
            assert!(is_previewable_mime(mime), "{mime} should be previewable");
        }
    }

    #[test]
    fn non_previewable_mimes_rejected() {
        for mime in &[
            "video/mp4",
            "application/zip",
            "application/octet-stream",
            "text/plain",
            "image/svg+xml",
            "image/heic",
            "",
        ] {
            assert!(
                !is_previewable_mime(mime),
                "{mime} should not be previewable"
            );
        }
    }
}
