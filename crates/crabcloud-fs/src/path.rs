//! `UserPath` — user-facing absolute path under the user's filesystem root.
//!
//! Rules enforced at construction:
//! - **MUST start with `/`.**
//! - No `..` segments.
//! - No `.` segments.
//! - No empty segments (`/a//b`).
//! - No embedded NUL.
//! - No backslash.
//! - Forward-slash separator only.
//! - Max length 4096.
//! - Trailing slash stripped (`/foo/` → `/foo`).
//! - `UserPath::root() == "/"`; `is_root()` true.

use crate::error::{FsError, FsResult};

const MAX_PATH_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserPath(String);

impl UserPath {
    pub fn new(s: impl Into<String>) -> FsResult<Self> {
        let mut s: String = s.into();
        if s.len() > MAX_PATH_LEN {
            return Err(FsError::InvalidPath("path too long".into()));
        }
        if s.contains('\0') {
            return Err(FsError::InvalidPath("embedded NUL".into()));
        }
        if s.contains('\\') {
            return Err(FsError::InvalidPath(
                "backslash is not a path separator".into(),
            ));
        }
        if !s.starts_with('/') {
            return Err(FsError::InvalidPath("user path must start with '/'".into()));
        }
        // Trim trailing slash unless this IS the root "/" (preserve single slash).
        while s.len() > 1 && s.ends_with('/') {
            s.pop();
        }
        // Validate every segment after the leading "/".
        if s.len() > 1 {
            for seg in s[1..].split('/') {
                if seg.is_empty() {
                    return Err(FsError::InvalidPath("empty segment".into()));
                }
                if seg == "." || seg == ".." {
                    return Err(FsError::InvalidPath(format!("illegal segment: {seg}")));
                }
            }
        }
        Ok(Self(s))
    }

    /// The user's filesystem root — `"/"`.
    pub fn root() -> Self {
        Self("/".into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    pub fn parent(&self) -> Option<UserPath> {
        if self.is_root() {
            return None;
        }
        match self.0.rfind('/') {
            // Last slash at position 0 → parent is root.
            Some(0) => Some(UserPath::root()),
            Some(i) => Some(UserPath(self.0[..i].to_string())),
            None => None, // Can't happen — `new` enforces leading slash.
        }
    }

    pub fn basename(&self) -> &str {
        if self.is_root() {
            return "";
        }
        match self.0.rfind('/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    pub fn join(&self, child: &str) -> FsResult<UserPath> {
        let combined = if self.is_root() {
            format!("/{}", child)
        } else {
            format!("{}/{}", self.0, child)
        };
        UserPath::new(combined)
    }
}

impl std::fmt::Display for UserPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_slash() {
        let r = UserPath::root();
        assert_eq!(r.as_str(), "/");
        assert!(r.is_root());
        assert!(r.parent().is_none());
        assert_eq!(r.basename(), "");
    }

    #[test]
    fn simple_path_parses() {
        let p = UserPath::new("/photos/cat.jpg").unwrap();
        assert_eq!(p.as_str(), "/photos/cat.jpg");
        assert_eq!(p.basename(), "cat.jpg");
        assert_eq!(p.parent().unwrap().as_str(), "/photos");
    }

    #[test]
    fn trailing_slash_stripped() {
        let p = UserPath::new("/photos/").unwrap();
        assert_eq!(p.as_str(), "/photos");
    }

    #[test]
    fn multiple_trailing_slashes_stripped() {
        let p = UserPath::new("/photos///").unwrap();
        assert_eq!(p.as_str(), "/photos");
    }

    #[test]
    fn root_path_preserved() {
        // "/" alone should NOT have its slash stripped.
        let p = UserPath::new("/").unwrap();
        assert_eq!(p.as_str(), "/");
        assert!(p.is_root());
    }

    #[test]
    fn missing_leading_slash_rejected() {
        assert!(matches!(
            UserPath::new("photos/cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn parent_dot_dot_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos/../etc"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn current_dot_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos/./cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn empty_segment_rejected() {
        assert!(matches!(
            UserPath::new("/photos//cat.jpg"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn embedded_nul_rejected() {
        assert!(matches!(
            UserPath::new("/a\0b"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn backslash_rejected() {
        assert!(matches!(
            UserPath::new("/a\\b"),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn too_long_rejected() {
        let big = "/".to_string() + &"a".repeat(5000);
        assert!(matches!(UserPath::new(big), Err(FsError::InvalidPath(_))));
    }

    #[test]
    fn empty_string_rejected() {
        assert!(matches!(UserPath::new(""), Err(FsError::InvalidPath(_))));
    }

    #[test]
    fn parent_of_top_level_returns_root() {
        let p = UserPath::new("/file.txt").unwrap();
        assert_eq!(p.parent().unwrap().as_str(), "/");
        assert_eq!(p.basename(), "file.txt");
    }

    #[test]
    fn join_onto_root() {
        let p = UserPath::root().join("a").unwrap();
        assert_eq!(p.as_str(), "/a");
    }

    #[test]
    fn join_onto_path() {
        let p = UserPath::new("/a/b").unwrap().join("c.txt").unwrap();
        assert_eq!(p.as_str(), "/a/b/c.txt");
    }

    #[test]
    fn join_validates_child() {
        let p = UserPath::new("/a").unwrap();
        assert!(p.join("../escape").is_err());
    }
}
