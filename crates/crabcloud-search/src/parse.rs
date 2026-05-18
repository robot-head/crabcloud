//! Query parser.
//!
//! Splits a free-text query into:
//!  - `text`: bare tokens (joined by space) for FTS match
//!  - `phrase`: quoted phrase, if any (at most one in MVP)
//!  - filter operators: `mime:<glob>`, `modified:>EPOCH|YYYY-MM-DD`,
//!    `modified:<...`, `modified:YYYY-MM-DD..YYYY-MM-DD`,
//!    `size:>N{B,KB,MB,GB,TB}`, `size:<N...`
//!  - Unknown `key:value` → bare text term (graceful degradation)

use crate::types::SearchQuery;

/// Parse the user-supplied query into a structured [`SearchQuery`].
/// The grammar is forgiving: malformed filters degrade to text terms.
pub fn parse_query(input: &str) -> SearchQuery {
    let mut q = SearchQuery::default();
    let mut text_parts: Vec<String> = Vec::new();

    // Phrase extraction: a single "..."-quoted run.
    let (phrase, rest) = extract_phrase(input);
    q.phrase = phrase;

    for tok in tokenize(&rest) {
        if let Some((key, value)) = tok.split_once(':') {
            if !apply_filter(&mut q, key, value) {
                tracing::debug!(
                    unknown_filter = %tok,
                    "search parser: unknown key:value, treating as text term"
                );
                text_parts.push(tok.to_string());
            }
        } else {
            text_parts.push(tok.to_string());
        }
    }
    q.text = text_parts.join(" ");
    q
}

fn extract_phrase(input: &str) -> (Option<String>, String) {
    let bytes = input.as_bytes();
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            start = Some(i);
            break;
        }
    }
    let Some(s) = start else {
        return (None, input.to_string());
    };
    let after = &input[s + 1..];
    if let Some(end_rel) = after.find('"') {
        let phrase = after[..end_rel].to_string();
        let mut rest = String::with_capacity(input.len().saturating_sub(phrase.len() + 2));
        rest.push_str(&input[..s]);
        rest.push_str(&after[end_rel + 1..]);
        (Some(phrase), rest)
    } else {
        // Unterminated quote — treat the rest as text, no phrase.
        (None, input.to_string())
    }
}

fn tokenize(s: &str) -> impl Iterator<Item = &str> {
    s.split_whitespace().filter(|t| !t.is_empty())
}

fn apply_filter(q: &mut SearchQuery, key: &str, value: &str) -> bool {
    match key {
        "mime" => {
            q.mime = Some(value.to_string());
            true
        }
        "modified" => parse_modified_filter(q, value),
        "size" => parse_size_filter(q, value),
        _ => false,
    }
}

fn parse_modified_filter(q: &mut SearchQuery, value: &str) -> bool {
    if let Some(rest) = value.strip_prefix('>') {
        if let Some(ts) = parse_epoch_or_iso(rest) {
            q.modified_after = Some(ts);
            return true;
        }
        return false;
    }
    if let Some(rest) = value.strip_prefix('<') {
        if let Some(ts) = parse_epoch_or_iso(rest) {
            q.modified_before = Some(ts);
            return true;
        }
        return false;
    }
    if let Some((a, b)) = value.split_once("..") {
        let (Some(a_ts), Some(b_ts)) = (parse_epoch_or_iso(a), parse_epoch_or_iso(b)) else {
            return false;
        };
        q.modified_after = Some(a_ts);
        q.modified_before = Some(b_ts);
        return true;
    }
    false
}

fn parse_epoch_or_iso(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let dt = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    dt.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc().timestamp())
}

fn parse_size_filter(q: &mut SearchQuery, value: &str) -> bool {
    if let Some(rest) = value.strip_prefix('>') {
        if let Some(n) = parse_size(rest) {
            q.size_min = Some(n);
            return true;
        }
        return false;
    }
    if let Some(rest) = value.strip_prefix('<') {
        if let Some(n) = parse_size(rest) {
            q.size_max = Some(n);
            return true;
        }
        return false;
    }
    false
}

fn parse_size(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    let mut split = bytes.len();
    while split > 0 && !bytes[split - 1].is_ascii_digit() {
        split -= 1;
    }
    if split == 0 {
        return None;
    }
    let (num_str, unit) = s.split_at(split);
    let n: i64 = num_str.parse().ok()?;
    let mult: i64 = match unit.to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" => 1024,
        "MB" => 1024 * 1024,
        "GB" => 1024 * 1024 * 1024,
        "TB" => 1024_i64 * 1024 * 1024 * 1024,
        _ => return None,
    };
    Some(n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_tokens_become_text() {
        let q = parse_query("q3 report");
        assert_eq!(q.text, "q3 report");
        assert!(q.phrase.is_none());
    }

    #[test]
    fn mime_filter_lifts_out() {
        let q = parse_query("q3 mime:image/*");
        assert_eq!(q.text, "q3");
        assert_eq!(q.mime.as_deref(), Some("image/*"));
    }

    #[test]
    fn modified_gt_iso_lifts_out() {
        let q = parse_query("modified:>2024-01-01 design");
        assert_eq!(q.text, "design");
        let want = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(q.modified_after, Some(want));
    }

    #[test]
    fn modified_gt_epoch_lifts_out() {
        let q = parse_query("modified:>1700000000");
        assert_eq!(q.text, "");
        assert_eq!(q.modified_after, Some(1700000000));
    }

    #[test]
    fn modified_range_lifts_both() {
        let q = parse_query("modified:2024-01-01..2024-12-31");
        let lo = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let hi = chrono::NaiveDate::from_ymd_opt(2024, 12, 31)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(q.modified_after, Some(lo));
        assert_eq!(q.modified_before, Some(hi));
    }

    #[test]
    fn size_gt_mb_lifts_out() {
        let q = parse_query("size:>1MB photo");
        assert_eq!(q.text, "photo");
        assert_eq!(q.size_min, Some(1024 * 1024));
    }

    #[test]
    fn size_lt_kb_lifts_out() {
        let q = parse_query("size:<10KB");
        assert_eq!(q.size_max, Some(10 * 1024));
    }

    #[test]
    fn phrase_extracts() {
        let q = parse_query("alice \"q3 report\" mime:application/pdf");
        assert_eq!(q.phrase.as_deref(), Some("q3 report"));
        assert_eq!(q.text, "alice");
        assert_eq!(q.mime.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn unterminated_phrase_falls_through() {
        let q = parse_query("\"unterminated text");
        assert!(q.phrase.is_none());
        assert_eq!(q.text, "\"unterminated text");
    }

    #[test]
    fn unknown_key_falls_back_to_text() {
        let q = parse_query("foo:bar baz");
        assert_eq!(q.text, "foo:bar baz");
        assert!(q.mime.is_none());
    }

    #[test]
    fn empty_query_is_empty() {
        let q = parse_query("");
        assert!(q.is_empty());
        assert!(!q.has_text_match());
    }

    #[test]
    fn filters_only_is_not_empty_but_no_text_match() {
        let q = parse_query("mime:image/*");
        assert!(!q.is_empty());
        assert!(!q.has_text_match());
    }
}
