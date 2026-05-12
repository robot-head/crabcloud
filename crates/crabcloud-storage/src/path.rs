//! `StoragePath` — UTF-8, normalized, relative-to-storage-root.
//!
//! Rules enforced at construction:
//! - No leading `/`.
//! - No `..` segments.
//! - No `.` segments (current-dir indirections are an error, not silently stripped).
//! - No empty segments (`a//b`).
//! - No embedded NUL.
//! - Forward-slash separator only.
//! - Max length 4096.
//! - Trailing slash stripped.

use crate::error::{StorageError, StorageResult};

const MAX_PATH_LEN: usize = 4096;

/// Normalized, relative-to-storage-root path.
///
/// Construct via [`StoragePath::new`] (validates) or [`StoragePath::root`]
/// (always-empty path representing the storage root).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoragePath(String);

impl StoragePath {
    pub fn new(s: impl Into<String>) -> StorageResult<Self> {
        let mut s: String = s.into();
        if s.len() > MAX_PATH_LEN {
            return Err(StorageError::InvalidPath("path too long".into()));
        }
        if s.contains('\0') {
            return Err(StorageError::InvalidPath("embedded NUL".into()));
        }
        if s.contains('\\') {
            return Err(StorageError::InvalidPath(
                "backslash is not a path separator".into(),
            ));
        }
        if s.starts_with('/') {
            return Err(StorageError::InvalidPath("leading slash".into()));
        }
        // Trim trailing slash (idempotent).
        while s.ends_with('/') {
            s.pop();
        }
        // Empty string is the root; skip segment validation.
        if !s.is_empty() {
            for seg in s.split('/') {
                if seg.is_empty() {
                    return Err(StorageError::InvalidPath("empty segment".into()));
                }
                if seg == "." || seg == ".." {
                    return Err(StorageError::InvalidPath(format!("illegal segment: {seg}")));
                }
            }
        }
        Ok(Self(s))
    }

    /// The storage root — empty path. Used as the "list everything" target
    /// and as the base for `join`.
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parent(&self) -> Option<StoragePath> {
        if self.0.is_empty() {
            return None;
        }
        match self.0.rfind('/') {
            Some(i) => Some(StoragePath(self.0[..i].to_string())),
            None => Some(StoragePath::root()),
        }
    }

    pub fn basename(&self) -> &str {
        match self.0.rfind('/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    pub fn join(&self, child: &str) -> StorageResult<StoragePath> {
        if self.0.is_empty() {
            StoragePath::new(child)
        } else {
            StoragePath::new(format!("{}/{}", self.0, child))
        }
    }
}

impl std::fmt::Display for StoragePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_empty() {
        let r = StoragePath::root();
        assert_eq!(r.as_str(), "");
        assert!(r.is_root());
        assert!(r.parent().is_none());
    }

    #[test]
    fn simple_path_parses() {
        let p = StoragePath::new("a/b/c.txt").unwrap();
        assert_eq!(p.as_str(), "a/b/c.txt");
        assert_eq!(p.basename(), "c.txt");
        assert_eq!(p.parent().unwrap().as_str(), "a/b");
    }

    #[test]
    fn trailing_slash_stripped() {
        let p = StoragePath::new("a/b/").unwrap();
        assert_eq!(p.as_str(), "a/b");
    }

    #[test]
    fn multiple_trailing_slashes_stripped() {
        let p = StoragePath::new("a/b///").unwrap();
        assert_eq!(p.as_str(), "a/b");
    }

    #[test]
    fn leading_slash_rejected() {
        assert!(matches!(
            StoragePath::new("/abs"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn parent_dot_dot_segment_rejected() {
        assert!(matches!(
            StoragePath::new("a/../b"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn current_dot_segment_rejected() {
        assert!(matches!(
            StoragePath::new("a/./b"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn empty_segment_rejected() {
        assert!(matches!(
            StoragePath::new("a//b"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn embedded_nul_rejected() {
        assert!(matches!(
            StoragePath::new("a\0b"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn backslash_rejected() {
        assert!(matches!(
            StoragePath::new("a\\b"),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn empty_string_is_root_equivalent() {
        let p = StoragePath::new("").unwrap();
        assert!(p.is_root());
    }

    #[test]
    fn too_long_rejected() {
        let big = "a".repeat(5000);
        assert!(matches!(
            StoragePath::new(big),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn basename_of_root_is_empty() {
        assert_eq!(StoragePath::root().basename(), "");
    }

    #[test]
    fn basename_of_single_segment() {
        let p = StoragePath::new("file.txt").unwrap();
        assert_eq!(p.basename(), "file.txt");
        assert_eq!(p.parent().unwrap().as_str(), "");
    }

    #[test]
    fn join_onto_root() {
        let p = StoragePath::root().join("a").unwrap();
        assert_eq!(p.as_str(), "a");
    }

    #[test]
    fn join_onto_path() {
        let p = StoragePath::new("a/b").unwrap().join("c.txt").unwrap();
        assert_eq!(p.as_str(), "a/b/c.txt");
    }

    #[test]
    fn join_validates_child() {
        let p = StoragePath::new("a").unwrap();
        assert!(p.join("../escape").is_err());
    }
}
