//! Response format selection. Mirrors Nextcloud's content negotiation.

/// Which serialization the response should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xml,
    Json,
}

impl Format {
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Xml => "application/xml; charset=utf-8",
            Format::Json => "application/json; charset=utf-8",
        }
    }
}

/// Pick the format from a request's `?format=` query value and `Accept` header.
/// Precedence: `?format=` query > `Accept: application/json` > XML default.
pub fn negotiate(format_query: Option<&str>, accept_header: Option<&str>) -> Format {
    if let Some(q) = format_query {
        let q = q.to_ascii_lowercase();
        if q == "json" {
            return Format::Json;
        }
        if q == "xml" {
            return Format::Xml;
        }
    }
    if let Some(accept) = accept_header {
        // Naive: substring search for "application/json" or "json".
        let a = accept.to_ascii_lowercase();
        if a.contains("application/json") || a.contains("text/json") {
            return Format::Json;
        }
    }
    Format::Xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_wins() {
        assert_eq!(
            negotiate(Some("json"), Some("application/xml")),
            Format::Json
        );
        assert_eq!(
            negotiate(Some("xml"), Some("application/json")),
            Format::Xml
        );
    }

    #[test]
    fn accept_header_falls_through_when_no_query() {
        assert_eq!(negotiate(None, Some("application/json")), Format::Json);
        assert_eq!(negotiate(None, Some("text/json")), Format::Json);
        assert_eq!(negotiate(None, Some("application/xml")), Format::Xml);
    }

    #[test]
    fn default_is_xml() {
        assert_eq!(negotiate(None, None), Format::Xml);
        assert_eq!(negotiate(Some("garbage"), None), Format::Xml);
    }

    #[test]
    fn content_type_matches_format() {
        assert!(Format::Json.content_type().starts_with("application/json"));
        assert!(Format::Xml.content_type().starts_with("application/xml"));
    }
}
