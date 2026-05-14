//! Helpers for the Files page's URL routing. The browser hits
//! `/apps/files/<segments...>`; this module converts between the captured
//! `Vec<String>` and a normalized absolute path string (`"/"`, `"/photos"`,
//! `"/photos/vacation"`). Path validation against `..`/`.`/etc. is left to
//! `UserPath::new` on the server.

/// Join captured route segments into a normalized absolute path. An empty
/// `segments` slice yields `"/"`.
pub fn segments_to_path(segments: &[String]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(segments.iter().map(|s| s.len() + 1).sum());
    for seg in segments {
        out.push('/');
        out.push_str(seg);
    }
    out
}

/// Split an absolute path into its non-empty segments. `"/"` yields `[]`.
pub fn path_to_segments(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_segments_yield_root() {
        assert_eq!(segments_to_path(&[]), "/");
    }

    #[test]
    fn single_segment_prefixed() {
        assert_eq!(segments_to_path(&["photos".to_string()]), "/photos");
    }

    #[test]
    fn multiple_segments_joined() {
        let s = vec!["photos".to_string(), "vacation".to_string()];
        assert_eq!(segments_to_path(&s), "/photos/vacation");
    }

    #[test]
    fn root_yields_empty_segments() {
        assert!(path_to_segments("/").is_empty());
    }

    #[test]
    fn path_segments_roundtrip() {
        let s = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(path_to_segments(&segments_to_path(&s)), s);
    }

    #[test]
    fn path_to_segments_strips_empty() {
        assert_eq!(
            path_to_segments("//a///b/"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
