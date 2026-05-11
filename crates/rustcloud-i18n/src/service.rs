//! Top-level i18n service.

use crate::catalog::Catalog;
use crate::locale::Locale;
use std::collections::HashMap;
use std::sync::Arc;

/// The runtime i18n service. Clone-cheap (`Arc` inside).
#[derive(Debug, Clone)]
pub struct I18n {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    catalogs: HashMap<(String, Locale), Catalog>,
    available: Vec<Locale>,
    fallback: Locale,
}

impl I18n {
    pub fn new(catalogs: HashMap<(String, Locale), Catalog>, fallback: Locale) -> Self {
        let mut available: Vec<Locale> = catalogs.keys().map(|(_, l)| l.clone()).collect();
        available.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        available.dedup();
        Self {
            inner: Arc::new(Inner {
                catalogs,
                available,
                fallback,
            }),
        }
    }

    pub fn available_locales(&self) -> &[Locale] {
        &self.inner.available
    }

    pub fn fallback(&self) -> &Locale {
        &self.inner.fallback
    }

    /// Translate a singular message. If no translation is available for
    /// `(app, locale, msgid)`, returns the source `msgid` unchanged.
    /// `args` substitutes `%s` placeholders in order.
    pub fn t(&self, app: &str, locale: &Locale, msgid: &str, args: &[&str]) -> String {
        let translated = self
            .inner
            .catalogs
            .get(&(app.to_string(), locale.clone()))
            .and_then(|c| c.lookup(msgid))
            .unwrap_or(msgid);
        substitute(translated, args)
    }

    /// Translate a pluralized message. `n` selects the form (simple English
    /// rule: 0 == many, 1 == singular, anything else == many).
    /// Falls back to the appropriate English source string.
    pub fn tn(
        &self,
        app: &str,
        locale: &Locale,
        singular: &str,
        plural: &str,
        n: i64,
        args: &[&str],
    ) -> String {
        let translated = self
            .inner
            .catalogs
            .get(&(app.to_string(), locale.clone()))
            .and_then(|c| c.lookup_plural(singular, n));
        let chosen = translated.unwrap_or(if n == 1 { singular } else { plural });
        substitute(chosen, args)
    }
}

/// Simple `%s` and `%d` substitution; consumes args in order.
fn substitute(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    let mut iter = args.iter();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                Some('s') | Some('d') => {
                    chars.next();
                    out.push_str(iter.next().copied().unwrap_or(""));
                }
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_all;
    use std::fs;
    use tempfile::tempdir;

    fn seed_catalogs() -> HashMap<(String, Locale), Catalog> {
        let dir = tempdir().unwrap();
        let app = dir.path().join("core");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("de.po"),
            r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Hello %s"
msgstr "Hallo %s"
"#,
        )
        .unwrap();
        load_all(dir.path()).unwrap()
    }

    #[test]
    fn translates_with_args() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("de"), "Hello %s", &["Alice"]);
        assert_eq!(s, "Hallo Alice");
    }

    #[test]
    fn falls_back_to_source_when_locale_missing() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("ja"), "Hello %s", &["Alice"]);
        assert_eq!(s, "Hello Alice");
    }

    #[test]
    fn falls_back_to_source_when_msgid_untranslated() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("de"), "Bye %s", &["Alice"]);
        assert_eq!(s, "Bye Alice");
    }

    #[test]
    fn substitute_handles_percent_d_and_percent_percent() {
        assert_eq!(
            substitute("%d apples are 100%%", &["5"]),
            "5 apples are 100%"
        );
        assert_eq!(substitute("plain text", &[]), "plain text");
        assert_eq!(substitute("%s %s", &["a"]), "a "); // missing arg → empty
    }

    #[test]
    fn tn_uses_singular_for_one_else_plural() {
        let cats = HashMap::new();
        let i18n = I18n::new(cats, Locale::new("en"));
        let l = Locale::new("en");
        assert_eq!(
            i18n.tn("files", &l, "%d file", "%d files", 1, &["1"]),
            "1 file"
        );
        assert_eq!(
            i18n.tn("files", &l, "%d file", "%d files", 5, &["5"]),
            "5 files"
        );
        assert_eq!(
            i18n.tn("files", &l, "%d file", "%d files", 0, &["0"]),
            "0 files"
        );
    }

    #[test]
    fn available_locales_is_sorted_and_unique() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let avail = i18n.available_locales();
        assert_eq!(avail, &[Locale::new("de")]);
    }
}
