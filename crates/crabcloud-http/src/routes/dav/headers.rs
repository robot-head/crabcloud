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

/// Parse the `If:` header. SP5 supports only the `(<urn:uuid:...>)` form
/// (Nextcloud's clients use this). Returns the list of submitted tokens.
///
/// The full RFC 4918 §10.4 grammar (tagged-list, etag conditions, `Not`
/// operator) is out of scope for SP5 — the lock-aware mutation path only
/// needs to compare submitted tokens against the stored lock token.
pub fn parse_if_tokens(headers: &HeaderMap) -> Vec<String> {
    let raw = match headers.get("if").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    // Walk every `<...>` bracketed token. Tagged lists like
    // `<https://example.com/foo> (<urn:uuid:abc>)` still resolve correctly
    // because the only tokens we collect that look like lock tokens are
    // `urn:uuid:*`; the caller compares by exact-equality so spurious
    // bracketed URLs simply never match a stored lock token.
    let mut tokens = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut t = String::new();
            for cc in chars.by_ref() {
                if cc == '>' {
                    break;
                }
                t.push(cc);
            }
            if !t.is_empty() {
                tokens.push(t);
            }
        }
    }
    tokens
}

/// Parse the `Lock-Token:` request header (single value). Strips the
/// surrounding `<` and `>` (per RFC 4918 §10.5 the header is a Coded-URL).
pub fn parse_lock_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("lock-token").and_then(|v| v.to_str().ok())?;
    let trimmed = raw.trim();
    let inner = trimmed.trim_start_matches('<').trim_end_matches('>');
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// Parse `Timeout: Second-<N>` or `Timeout: Infinite`. Returns the
/// clamped TTL in seconds (cap at 1800; default 1800). Accepts multiple
/// comma-separated values per RFC 4918 §10.7 and uses the first valid one.
pub fn parse_timeout(headers: &HeaderMap) -> i64 {
    const DEFAULT_TTL: i64 = 1800;
    const MAX_TTL: i64 = 1800;
    let raw = match headers.get("timeout").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return DEFAULT_TTL,
    };
    for part in raw.split(',') {
        let p = part.trim();
        if p.eq_ignore_ascii_case("infinite") {
            return MAX_TTL;
        }
        if let Some(n) = p
            .strip_prefix("Second-")
            .or_else(|| p.strip_prefix("second-"))
        {
            if let Ok(v) = n.parse::<i64>() {
                return v.clamp(0, MAX_TTL);
            }
        }
    }
    DEFAULT_TTL
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

    #[test]
    fn if_tokens_simple() {
        let h = hm(&[("if", "(<urn:uuid:abc-123>)")]);
        let tokens = parse_if_tokens(&h);
        assert_eq!(tokens, vec!["urn:uuid:abc-123".to_string()]);
    }

    #[test]
    fn if_tokens_multiple() {
        let h = hm(&[("if", "(<urn:uuid:one>) (<urn:uuid:two>)")]);
        let tokens = parse_if_tokens(&h);
        assert_eq!(
            tokens,
            vec!["urn:uuid:one".to_string(), "urn:uuid:two".to_string()]
        );
    }

    #[test]
    fn if_tokens_absent() {
        assert!(parse_if_tokens(&hm(&[])).is_empty());
    }

    #[test]
    fn lock_token_parses_brackets() {
        let h = hm(&[("lock-token", "<urn:uuid:xyz>")]);
        assert_eq!(parse_lock_token(&h), Some("urn:uuid:xyz".to_string()));
    }

    #[test]
    fn lock_token_absent() {
        assert!(parse_lock_token(&hm(&[])).is_none());
    }

    #[test]
    fn timeout_second_form() {
        assert_eq!(parse_timeout(&hm(&[("timeout", "Second-60")])), 60);
    }

    #[test]
    fn timeout_caps_at_1800() {
        assert_eq!(parse_timeout(&hm(&[("timeout", "Second-9999")])), 1800);
    }

    #[test]
    fn timeout_infinite_is_capped() {
        assert_eq!(parse_timeout(&hm(&[("timeout", "Infinite")])), 1800);
    }

    #[test]
    fn timeout_default_when_absent() {
        assert_eq!(parse_timeout(&hm(&[])), 1800);
    }

    #[test]
    fn timeout_multi_value_picks_first_valid() {
        // First entry unparseable; second is valid.
        assert_eq!(parse_timeout(&hm(&[("timeout", "garbage, Second-30")])), 30);
    }
}
