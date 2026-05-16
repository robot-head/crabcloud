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
//!
//! The parity test only compiles for the host (not wasm32) target because
//! `crabcloud-preview` pulls in `image` + `hayro` which we deliberately
//! keep out of the WASM graph. The `#[cfg(not(target_arch = "wasm32"))]`
//! gate ensures this test is only built/run on the host.

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

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod parity_tests {
    use super::is_previewable_mime;

    /// Shared set of mimes that both client and server allowlists are
    /// asked about. New entries here force both sides to agree.
    const SHARED_MIME_PROBES: &[(&str, bool)] = &[
        // Expected previewable.
        ("image/jpeg", true),
        ("image/png", true),
        ("image/gif", true),
        ("image/webp", true),
        ("image/jpeg; charset=binary", true),
        ("IMAGE/JPEG", true),
        ("application/pdf", true),
        ("Application/PDF", true),
        // Expected NOT previewable.
        ("image/svg+xml", false),
        ("image/heic", false),
        ("image/avif", false),
        ("text/plain", false),
        ("text/html", false),
        ("video/mp4", false),
        ("application/zip", false),
        ("application/json", false),
        ("application/octet-stream", false),
        ("application/x-empty", false),
        ("", false),
    ];

    #[test]
    fn client_allowlist_matches_server_for_all_probes() {
        for (mime, expected) in SHARED_MIME_PROBES {
            // Client side.
            let client_says = is_previewable_mime(mime);
            // Server side — must agree with both the probe expectation
            // and with client_says.
            let server_says = crabcloud_preview::provider_for_mime(mime).is_some();
            assert_eq!(
                client_says, *expected,
                "client allowlist disagrees with probe expectation for {mime:?}",
            );
            assert_eq!(
                server_says, *expected,
                "server allowlist disagrees with probe expectation for {mime:?}",
            );
            assert_eq!(
                client_says, server_says,
                "client ({client_says}) and server ({server_says}) disagree for {mime:?}",
            );
        }
    }
}
