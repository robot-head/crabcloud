//! Gettext `.po` catalog loader.

use crate::locale::Locale;
use polib::po_file;
use std::collections::HashMap;
use std::path::Path;

/// Errors that can occur while scanning or parsing `.po` catalog files.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// I/O error while reading the catalog directory or a file within it.
    #[error("scan I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The `.po` file failed to parse; `path` identifies the file and
    /// `message` is the underlying parser error.
    #[error("parse error in {path}: {message}")]
    Parse {
        /// Filesystem path of the `.po` file that failed to parse.
        path: String,
        /// Human-readable parser error message.
        message: String,
    },
}

/// One catalog: a flat msgid → msgstr lookup for a specific `(app, locale)`.
///
/// Plural forms are stored separately; `lookup_plural` returns by index n.
#[derive(Debug, Default)]
pub struct Catalog {
    singular: HashMap<String, String>,
    /// For pluralized entries: msgid_singular → Vec of plural forms.
    /// The index for `n` is computed by the caller (we use the simple
    /// "n != 1" English rule; full plural-form expressions can land later).
    plural: HashMap<String, Vec<String>>,
}

impl Catalog {
    /// Look up a singular message. Returns `None` if not translated; callers
    /// should fall back to the source string.
    pub fn lookup(&self, msgid: &str) -> Option<&str> {
        self.singular.get(msgid).map(String::as_str)
    }

    /// Look up a plural message. `n` is the count; we use the simple
    /// English rule (index 0 for n==1, index 1 otherwise). Returns the source
    /// string fallback expectation: `None` means the caller should fall back.
    pub fn lookup_plural(&self, msgid: &str, n: i64) -> Option<&str> {
        let forms = self.plural.get(msgid)?;
        let idx = if n == 1 { 0 } else { 1 };
        forms.get(idx).map(String::as_str)
    }
}

/// Load all `l10n/<app>/<locale>.po` catalogs under `root`. Each subdirectory
/// of `root` is treated as an `<app>` name; each `*.po` file inside is treated
/// as a locale.
///
/// Returns a map keyed by `(app, locale)` for fast lookup at request time.
/// Missing `root` directories are not an error — they just produce an empty map.
pub fn load_all(root: &Path) -> Result<HashMap<(String, Locale), Catalog>, CatalogError> {
    let mut out = HashMap::new();
    if !root.exists() {
        return Ok(out);
    }
    for app_entry in std::fs::read_dir(root)? {
        let app_entry = app_entry?;
        if !app_entry.file_type()?.is_dir() {
            continue;
        }
        let app = app_entry.file_name().to_string_lossy().to_string();
        for po_entry in std::fs::read_dir(app_entry.path())? {
            let po_entry = po_entry?;
            let path = po_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("po") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let locale = Locale::new(stem);
            let catalog = parse_po(&path)?;
            out.insert((app.clone(), locale), catalog);
        }
    }
    Ok(out)
}

fn parse_po(path: &Path) -> Result<Catalog, CatalogError> {
    let file = po_file::parse(path).map_err(|e| CatalogError::Parse {
        path: path.display().to_string(),
        message: format!("{e:?}"),
    })?;
    let mut cat = Catalog::default();
    for msg in file.messages() {
        if !msg.is_translated() {
            continue;
        }
        if msg.is_plural() {
            if let Ok(forms) = msg.msgstr_plural() {
                if !forms.is_empty() {
                    cat.plural.insert(msg.msgid().to_string(), forms.clone());
                }
            }
        } else if let Ok(msgstr) = msg.msgstr() {
            if !msgstr.is_empty() {
                cat.singular
                    .insert(msg.msgid().to_string(), msgstr.to_string());
            }
        }
    }
    Ok(cat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_po(dir: &Path, app: &str, locale: &str, body: &str) {
        let app_dir = dir.join(app);
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(app_dir.join(format!("{locale}.po")), body).unwrap();
    }

    const MIN_PO: &str = r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Hello"
msgstr "Hallo"

msgid "Bye"
msgstr "Tschüss"
"#;

    #[test]
    fn missing_root_returns_empty() {
        let dir = tempdir().unwrap();
        let map = load_all(&dir.path().join("does-not-exist")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn loads_single_app_locale() {
        let dir = tempdir().unwrap();
        write_po(dir.path(), "core", "de", MIN_PO);
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 1);
        let key = ("core".to_string(), Locale::new("de"));
        let cat = map.get(&key).unwrap();
        assert_eq!(cat.lookup("Hello"), Some("Hallo"));
        assert_eq!(cat.lookup("Bye"), Some("Tschüss"));
        assert_eq!(cat.lookup("Untranslated"), None);
    }

    #[test]
    fn loads_multiple_apps_and_locales() {
        let dir = tempdir().unwrap();
        write_po(dir.path(), "core", "de", MIN_PO);
        write_po(dir.path(), "core", "fr", MIN_PO);
        write_po(dir.path(), "files", "de", MIN_PO);
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key(&("core".to_string(), Locale::new("de"))));
        assert!(map.contains_key(&("core".to_string(), Locale::new("fr"))));
        assert!(map.contains_key(&("files".to_string(), Locale::new("de"))));
    }

    #[test]
    fn ignores_non_po_files() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("core")).unwrap();
        fs::write(dir.path().join("core").join("readme.md"), "not a po file").unwrap();
        fs::write(dir.path().join("core").join("de.po"), MIN_PO).unwrap();
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 1);
    }
}
