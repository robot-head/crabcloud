//! Header parsers for DAV: Destination, Depth, If, Lock-Token, Timeout, Overwrite.

use axum::http::HeaderMap;
use std::ops::Range;

use crate::routes::dav::error::{DavError, DavResult};

/// Parsed value of the `Depth:` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Zero,
    One,
    Infinity,
}

/// Parse the `Depth:` header. Returns one of `0`, `1`, or `infinity`. Default
/// is supplied by the caller. The caller decides whether `infinity` is allowed
/// (e.g. PROPFIND limits it via `propfind-finite-depth`).
pub fn parse_depth(headers: &HeaderMap, default: Depth) -> DavResult<Depth> {
    match headers.get("depth").and_then(|v| v.to_str().ok()) {
        None => Ok(default),
        Some("0") => Ok(Depth::Zero),
        Some("1") => Ok(Depth::One),
        Some("infinity") => Ok(Depth::Infinity),
        Some(other) => Err(DavError::BadRequest(format!("invalid Depth: {other}"))),
    }
}

/// Parse the `Overwrite:` header. `T` (default) or `F`.
pub fn parse_overwrite(headers: &HeaderMap) -> DavResult<bool> {
    match headers.get("overwrite").and_then(|v| v.to_str().ok()) {
        None | Some("T") => Ok(true),
        Some("F") => Ok(false),
        Some(other) => Err(DavError::BadRequest(format!("invalid Overwrite: {other}"))),
    }
}

/// Parse the `Destination:` header. Accepts both absolute URL (strips
/// `<scheme>://<host>` prefix up to and including the first `/dav` or
/// `/remote.php/dav`) and path-only forms. Returns the captured `(user, path)`
/// pair (path is the URL-encoded segment after `/dav/files/{user}/`; the
/// handler decodes and validates as a `UserPath`).
pub fn parse_destination_files(headers: &HeaderMap) -> DavResult<(String, String)> {
    let raw = headers
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| DavError::BadRequest("missing Destination header".into()))?;
    // Strip scheme+host if absolute.
    let path = if let Some(idx) = raw.find("://") {
        let after_scheme = &raw[idx + 3..];
        match after_scheme.find('/') {
            Some(slash) => &after_scheme[slash..],
            None => return Err(DavError::BadRequest("Destination missing path".into())),
        }
    } else {
        raw
    };
    // Find the `/files/` segment after either prefix.
    let after_files = path
        .strip_prefix("/remote.php/dav/files/")
        .or_else(|| path.strip_prefix("/dav/files/"))
        .ok_or_else(|| DavError::BadRequest(format!("Destination not under /dav/files/: {raw}")))?;
    // Split into user + path.
    match after_files.find('/') {
        Some(slash) => Ok((after_files[..slash].into(), after_files[slash + 1..].into())),
        None => Ok((after_files.into(), String::new())),
    }
}

/// Parse a Range header value `bytes=N-M`. Returns the half-open byte range.
/// Errors on multi-range (`bytes=0-499,1000-1499`) and out-of-bounds requests.
pub fn parse_range(headers: &HeaderMap, file_size: u64) -> DavResult<Option<Range<u64>>> {
    let raw = match headers.get("range").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return Ok(None),
    };
    let rest = raw
        .strip_prefix("bytes=")
        .ok_or(DavError::RangeNotSatisfiable { file_size })?;
    if rest.contains(',') {
        return Err(DavError::RangeNotSatisfiable { file_size });
    }
    let (start_s, end_s) = rest
        .split_once('-')
        .ok_or(DavError::RangeNotSatisfiable { file_size })?;
    let range = match (start_s.is_empty(), end_s.is_empty()) {
        (false, false) => {
            let start: u64 = start_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            let end: u64 = end_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if end < start || end >= file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            start..(end + 1)
        }
        (false, true) => {
            let start: u64 = start_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if start >= file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            start..file_size
        }
        (true, false) => {
            // `bytes=-N` means the last N bytes.
            let suffix: u64 = end_s
                .parse()
                .map_err(|_| DavError::RangeNotSatisfiable { file_size })?;
            if suffix == 0 || suffix > file_size {
                return Err(DavError::RangeNotSatisfiable { file_size });
            }
            (file_size - suffix)..file_size
        }
        (true, true) => return Err(DavError::RangeNotSatisfiable { file_size }),
    };
    Ok(Some(range))
}

#[derive(Debug, Clone)]
pub enum IfMatch {
    Absent,
    Wildcard,
    Etag(String),
}

pub fn parse_if_match(headers: &HeaderMap) -> IfMatch {
    match headers.get("if-match").and_then(|v| v.to_str().ok()) {
        None => IfMatch::Absent,
        Some("*") => IfMatch::Wildcard,
        Some(raw) => {
            // Strip surrounding quotes if present.
            let s = raw.trim();
            let unquoted = s.trim_matches('"');
            IfMatch::Etag(unquoted.to_string())
        }
    }
}

pub fn parse_if_none_match_wildcard(headers: &HeaderMap) -> bool {
    headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim() == "*")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            let name = axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
            h.insert(name, axum::http::HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn depth_default() {
        let h = hm(&[]);
        assert_eq!(parse_depth(&h, Depth::One).unwrap(), Depth::One);
    }

    #[test]
    fn depth_zero_one_infinity() {
        assert_eq!(
            parse_depth(&hm(&[("depth", "0")]), Depth::One).unwrap(),
            Depth::Zero
        );
        assert_eq!(
            parse_depth(&hm(&[("depth", "1")]), Depth::One).unwrap(),
            Depth::One
        );
        assert_eq!(
            parse_depth(&hm(&[("depth", "infinity")]), Depth::One).unwrap(),
            Depth::Infinity
        );
    }

    #[test]
    fn depth_invalid_rejects() {
        assert!(matches!(
            parse_depth(&hm(&[("depth", "2")]), Depth::One),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn overwrite_default_true() {
        assert!(parse_overwrite(&hm(&[])).unwrap());
        assert!(parse_overwrite(&hm(&[("overwrite", "T")])).unwrap());
        assert!(!parse_overwrite(&hm(&[("overwrite", "F")])).unwrap());
    }

    #[test]
    fn destination_absolute_url() {
        let h = hm(&[(
            "destination",
            "https://example.com/dav/files/alice/photos/cat.jpg",
        )]);
        let (u, p) = parse_destination_files(&h).unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "photos/cat.jpg");
    }

    #[test]
    fn destination_path_only_legacy_prefix() {
        let h = hm(&[("destination", "/remote.php/dav/files/alice/x.txt")]);
        let (u, p) = parse_destination_files(&h).unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "x.txt");
    }

    #[test]
    fn destination_missing_header_errors() {
        assert!(matches!(
            parse_destination_files(&hm(&[])),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn range_simple() {
        let r = parse_range(&hm(&[("range", "bytes=0-9")]), 100)
            .unwrap()
            .unwrap();
        assert_eq!(r, 0..10);
    }

    #[test]
    fn range_open_end() {
        let r = parse_range(&hm(&[("range", "bytes=50-")]), 100)
            .unwrap()
            .unwrap();
        assert_eq!(r, 50..100);
    }

    #[test]
    fn range_suffix() {
        let r = parse_range(&hm(&[("range", "bytes=-10")]), 100)
            .unwrap()
            .unwrap();
        assert_eq!(r, 90..100);
    }

    #[test]
    fn range_invalid_rejects() {
        assert!(matches!(
            parse_range(&hm(&[("range", "bytes=500-999")]), 100),
            Err(DavError::RangeNotSatisfiable { .. })
        ));
        assert!(matches!(
            parse_range(&hm(&[("range", "bytes=0-99,100-199")]), 200),
            Err(DavError::RangeNotSatisfiable { .. })
        ));
    }

    #[test]
    fn if_match_parsing() {
        assert!(matches!(parse_if_match(&hm(&[])), IfMatch::Absent));
        assert!(matches!(
            parse_if_match(&hm(&[("if-match", "*")])),
            IfMatch::Wildcard
        ));
        match parse_if_match(&hm(&[("if-match", r#""abc""#)])) {
            IfMatch::Etag(s) => assert_eq!(s, "abc"),
            _ => panic!(),
        }
    }

    #[test]
    fn if_none_match_star() {
        assert!(parse_if_none_match_wildcard(&hm(&[("if-none-match", "*")])));
        assert!(!parse_if_none_match_wildcard(&hm(&[])));
    }
}
