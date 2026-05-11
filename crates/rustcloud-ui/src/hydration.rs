//! Hydration payload — emits a `<script>` tag with the JSON-encoded
//! `RequestContext` for the WASM client to read on mount.
//!
//! See spec §8.3.

use crate::context::RequestContext;
use serde_json::Value;

const SCRIPT_OPEN: &str = "<script id=\"__dx_ctx\" type=\"application/json\">";
const SCRIPT_CLOSE: &str = "</script>";

/// Render the hydration script tag for a given context. The JSON body is
/// escaped so `<`, `>`, and `&` cannot terminate the surrounding script
/// element nor execute via HTML interpretation.
pub fn render_hydration_script(ctx: &RequestContext) -> String {
    let body = encode_safe_json(ctx);
    let mut out = String::with_capacity(SCRIPT_OPEN.len() + body.len() + SCRIPT_CLOSE.len());
    out.push_str(SCRIPT_OPEN);
    out.push_str(&body);
    out.push_str(SCRIPT_CLOSE);
    out
}

fn encode_safe_json<T: serde::Serialize>(value: &T) -> String {
    // `serde_json::to_value` is infallible for our `RequestContext` (no
    // non-string map keys, no NaN floats, no unrepresentable types).
    let v: Value = serde_json::to_value(value).unwrap_or(Value::Null);
    let raw = serde_json::to_string(&v).unwrap_or_else(|_| "null".to_string());
    escape_for_script_tag(&raw)
}

fn escape_for_script_tag(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_payload_in_script_tag() {
        let ctx = RequestContext::anonymous("en", "tok");
        let s = render_hydration_script(&ctx);
        assert!(s.starts_with("<script id=\"__dx_ctx\" type=\"application/json\">"));
        assert!(s.ends_with("</script>"));
    }

    #[test]
    fn escapes_lt_gt_amp_in_payload() {
        // Locale shouldn't ever contain these but request_token could in theory.
        let ctx = RequestContext::anonymous("en", "ab<c>&d");
        let s = render_hydration_script(&ctx);
        // Tag boundaries remain the only literal `<...>` in the output.
        // Body should not contain another literal `<` or `>`.
        let body = &s[SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        assert!(!body.contains('<'));
        assert!(!body.contains('>'));
        assert!(!body.contains('&'));
        assert!(body.contains("\\u003c"));
        assert!(body.contains("\\u003e"));
        assert!(body.contains("\\u0026"));
    }

    #[test]
    fn payload_parses_back_when_unescaped() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-123");
        let s = render_hydration_script(&ctx);
        let body = &s[SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        // The escapes are valid JSON `\u00XX` sequences which `serde_json` will
        // happily decode back to `<`/`>`/`&`. For this test the payload contains
        // none of those, so the parse is a straight round-trip.
        let parsed: RequestContext = serde_json::from_str(body).unwrap();
        assert_eq!(parsed, ctx);
    }

    #[test]
    fn escapes_line_separator_chars() {
        // U+2028 / U+2029 are valid JSON but break browsers' script parsing.
        let ctx = RequestContext::anonymous("en", "a\u{2028}b\u{2029}c");
        let s = render_hydration_script(&ctx);
        let body = &s[SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        assert!(!body.contains('\u{2028}'));
        assert!(!body.contains('\u{2029}'));
        assert!(body.contains("\\u2028"));
        assert!(body.contains("\\u2029"));
    }
}
