//! Compression-method dispatch keyed off mime type.

use zip::CompressionMethod;

const COMPRESSIBLE_PREFIXES: &[&str] = &[
    "text/",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/x-yaml",
    "application/wasm",
    "image/svg+xml",
];

/// Pick a compression method for a single zip entry based on its mime.
///
/// Already-compressed binary types (jpeg, png, mp4, zip, octet-stream) are
/// stored verbatim to avoid burning CPU for negligible size wins. The
/// matching is case-insensitive prefix; an unknown or empty mime falls
/// through to [`CompressionMethod::Stored`].
pub fn compression_for_mime(mime: &str) -> CompressionMethod {
    let lc = mime.to_ascii_lowercase();
    if COMPRESSIBLE_PREFIXES.iter().any(|p| lc.starts_with(p)) {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_mimes_get_deflate() {
        for mime in &[
            "text/plain",
            "text/html",
            "text/css",
            "application/json",
            "application/javascript",
            "application/xml",
            "application/x-yaml",
            "application/wasm",
            "image/svg+xml",
        ] {
            assert_eq!(
                compression_for_mime(mime),
                CompressionMethod::Deflated,
                "{mime} should DEFLATE",
            );
        }
    }

    #[test]
    fn binary_mimes_get_stored() {
        for mime in &[
            "image/jpeg",
            "image/png",
            "video/mp4",
            "application/zip",
            "application/octet-stream",
            "application/pdf",
            "",
        ] {
            assert_eq!(
                compression_for_mime(mime),
                CompressionMethod::Stored,
                "{mime} should STORE",
            );
        }
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            compression_for_mime("TEXT/Plain"),
            CompressionMethod::Deflated,
        );
        assert_eq!(
            compression_for_mime("Application/JSON"),
            CompressionMethod::Deflated,
        );
    }
}
