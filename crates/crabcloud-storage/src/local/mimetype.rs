//! Mimetype detection: extension lookup against the build-script-generated
//! phf map, then magic-byte sniffing via the `infer` crate. Final fallback
//! is `application/octet-stream`.

use crate::meta::Mimetype;

include!(concat!(env!("OUT_DIR"), "/mimetype_map.rs"));

/// Best-effort mimetype from path extension. Returns `None` if no entry.
pub fn from_extension(path: &str) -> Option<Mimetype> {
    let idx = path.rfind('.')?;
    let ext = path[idx + 1..].to_ascii_lowercase();
    EXTENSION_MIMETYPES
        .get(ext.as_str())
        .and_then(|s| Mimetype::parse(s).ok())
}

/// Magic-byte sniff on the first 4096 bytes of a file body.
pub fn sniff_magic(head: &[u8]) -> Option<Mimetype> {
    infer::get(head).and_then(|t| Mimetype::parse(t.mime_type()).ok())
}

/// Best-effort combined detection: extension → magic → octet-stream.
pub fn detect(path: &str, head: &[u8]) -> Mimetype {
    if let Some(m) = from_extension(path) {
        return m;
    }
    if let Some(m) = sniff_magic(head) {
        return m;
    }
    Mimetype::octet_stream()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_table_is_seeded() {
        const { assert!(EXTENSION_COUNT > 50) };
    }

    #[test]
    fn extension_lookup_known_types() {
        assert_eq!(from_extension("x.txt").unwrap().as_str(), "text/plain");
        assert_eq!(from_extension("x.png").unwrap().as_str(), "image/png");
        assert_eq!(from_extension("Photo.JPG").unwrap().as_str(), "image/jpeg");
        assert_eq!(
            from_extension("doc.pdf").unwrap().as_str(),
            "application/pdf"
        );
    }

    #[test]
    fn extension_lookup_unknown_returns_none() {
        assert!(from_extension("x.unknownextension").is_none());
        assert!(from_extension("noext").is_none());
    }

    #[test]
    fn sniff_magic_detects_png() {
        // PNG signature
        let head = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(sniff_magic(head).unwrap().as_str(), "image/png");
    }

    #[test]
    fn sniff_magic_returns_none_on_unknown_bytes() {
        let head = b"random text content here";
        assert!(sniff_magic(head).is_none());
    }

    #[test]
    fn detect_prefers_extension_over_sniff() {
        // PNG bytes but with .txt extension — extension wins.
        let head = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(detect("misnamed.txt", head).as_str(), "text/plain");
    }

    #[test]
    fn detect_falls_through_to_octet_stream() {
        assert_eq!(
            detect("noext", b"random").as_str(),
            "application/octet-stream"
        );
    }
}
