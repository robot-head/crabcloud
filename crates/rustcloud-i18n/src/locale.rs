//! `Locale` type and Accept-Language resolution.

/// A short language tag, normalized to lowercase with underscores
/// (Nextcloud convention: `en`, `de`, `fr_FR`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locale(String);

impl Locale {
    /// Build a `Locale` from any string-like value. Normalizes to lowercase and
    /// converts `-` separators to `_` so `EN-US` and `en_us` collapse to one form.
    pub fn new(s: impl Into<String>) -> Self {
        let raw = s.into();
        // Normalize: lowercase, hyphen → underscore (Accept-Language uses hyphens;
        // Nextcloud filenames use underscores).
        let normalized = raw.to_lowercase().replace('-', "_");
        Locale(normalized)
    }

    /// Returns the normalized tag (e.g. `en`, `fr_fr`).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// "en" or "fr" or similar — the base language without region.
    pub fn base(&self) -> &str {
        self.0.split_once('_').map(|(b, _)| b).unwrap_or(&self.0)
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Pick the best locale for a request, given:
/// - `accept_language`: the raw header value (may be empty).
/// - `available`: locales we actually have catalogs for.
/// - `fallback`: the `config.default_language` (lowercased).
///
/// Resolution order: header preferences (highest q-weight first) that match available
/// → header base-language match (e.g. header says `de-DE`, only `de` available) →
/// fallback → `en`.
pub fn resolve(accept_language: &str, available: &[Locale], fallback: &Locale) -> Locale {
    let prefs = accept_language::parse(accept_language); // Vec<String>, ordered by q desc

    for pref in &prefs {
        let want = Locale::new(pref.clone());
        if available.iter().any(|l| l == &want) {
            return want;
        }
    }
    for pref in &prefs {
        let want = Locale::new(pref.clone());
        if available.iter().any(|l| l.base() == want.base()) {
            return available
                .iter()
                .find(|l| l.base() == want.base())
                .unwrap()
                .clone();
        }
    }
    if available.iter().any(|l| l == fallback) {
        return fallback.clone();
    }
    Locale::new("en")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locales(tags: &[&str]) -> Vec<Locale> {
        tags.iter().map(|s| Locale::new(*s)).collect()
    }

    #[test]
    fn locale_normalizes_to_lowercase_underscore() {
        assert_eq!(Locale::new("EN-US").as_str(), "en_us");
        assert_eq!(Locale::new("de").as_str(), "de");
    }

    #[test]
    fn base_strips_region() {
        assert_eq!(Locale::new("fr_FR").base(), "fr");
        assert_eq!(Locale::new("de").base(), "de");
    }

    #[test]
    fn exact_match_wins() {
        let avail = locales(&["en", "de", "fr_fr"]);
        let fb = Locale::new("en");
        assert_eq!(resolve("de", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn higher_q_weight_wins_when_multiple_offered() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("en");
        assert_eq!(
            resolve("de;q=0.9, en;q=0.8", &avail, &fb),
            Locale::new("de")
        );
        assert_eq!(
            resolve("de;q=0.1, en;q=0.9", &avail, &fb),
            Locale::new("en")
        );
    }

    #[test]
    fn base_language_falls_back_when_region_unavailable() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("en");
        // Asked for de-DE; we only have plain "de" — should match by base.
        assert_eq!(resolve("de-DE", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn fallback_used_when_no_match() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("de");
        assert_eq!(resolve("ja, ko", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn empty_header_uses_fallback() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("de");
        assert_eq!(resolve("", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn final_en_when_fallback_also_unavailable() {
        let avail = locales(&["en"]);
        let fb = Locale::new("de"); // not available
        assert_eq!(resolve("", &avail, &fb), Locale::new("en"));
    }
}
