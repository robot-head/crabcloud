# Storage Trait + Local FS + Memory Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `crabcloud-storage` — a new workspace crate with the `Storage` async trait, two backends (`LocalStorage`, `MemoryStorage`), and a parametrized trait test suite that both backends pass.

**Architecture:** Pure-primitive crate, no DB / HTTP / Dioxus deps. `Storage` trait is object-safe. All mutating methods accept `&dyn EventSink` (no-op in 4a; 4b plugs in the async scanner consumer). Local backend uses atomic-rename writes + xattr-persisted random ETags (mtime+inode fallback) + ~400-entry mimetype table + magic-byte sniffing. Memory backend uses a single-`RwLock`-guarded BTreeMap.

**Tech Stack:** Rust 1.95 + tokio (fs, io-util, sync) + bytes + async-trait + thiserror + tracing + phf + infer + xattr (unix only) + rand (workspace 0.10).

**Parent spec:** `docs/superpowers/specs/2026-05-12-storage-trait-and-backends-design.md` (merged at master `6b0601d`).

**Branch protection:** master is rules-gated (PR required); auto-merge disabled at repo level. Merge each batch manually with `gh pr merge --squash --delete-branch` after the 5 non-cosmetic checks pass (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e).

---

## Conventions

- **Commits:** Conventional Commits (`feat(storage)`, `test(storage)`, `docs(storage)`) with `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
- **TDD:** Failing test → fail → implement → pass → commit. Type-level tests for Batch A; trait suite for Batches C-E; specific tests for E.
- **rustfmt:** `cargo fmt --all` after each task.
- **`cargo xtask check-all` must pass at branch tip before push.**
- **`-D warnings` workspace-wide.** New deps must be referenced immediately.
- **One PR per batch.** STOP at "PR opened, awaiting controller merge."

---

## File Structure

```
crates/crabcloud-storage/                          NEW CRATE
├── Cargo.toml
├── build.rs                                       phf_codegen for mimetype table
├── data/
│   └── mimetypes.txt                              extension→mimetype seed (~400 entries)
└── src/
    ├── lib.rs                                     Storage trait + EventSink trait + StorageEvent + NoopEventSink + re-exports
    ├── error.rs                                   StorageError + StorageResult + map_io helper
    ├── path.rs                                    StoragePath (UTF-8, normalized)
    ├── meta.rs                                    FileMetadata, ETag, Mimetype, Permissions, DirEntry, FileKind, MultipartHandle, PartTag
    ├── local/
    │   ├── mod.rs                                 LocalStorage struct + Storage impl
    │   ├── atomic.rs                              TempFileGuard + put_file + commit_multipart write sequence
    │   ├── mimetype.rs                            extension lookup + magic-byte sniff
    │   └── xattr_io.rs                            ETag + mimetype xattr read/write with fallback
    └── memory/
        └── mod.rs                                 MemoryStorage struct + Storage impl

crates/crabcloud-storage/tests/
├── support/
│   └── mod.rs                                     RecordingSink fake
├── trait_suite.rs                                 Parametrized suite + two top-level runners (LocalStorage, MemoryStorage)
├── local_specific.rs                              Atomic durability, xattr persistence, path escape
└── memory_specific.rs                             Concurrent writes
```

Workspace-level edits:

- `Cargo.toml` — add `bytes`, `phf`, `phf_codegen`, `infer`, `xattr` to `[workspace.dependencies]`; add `crates/crabcloud-storage` to `[workspace] members`.
- `README.md` — extend workspace-layout bullet to include `crabcloud-storage` (in Batch F).

---

## Batches

| Batch | Tasks | Theme |
|-------|-------|---|
| **A** | 1 | Crate skeleton + types (StoragePath, error, meta, EventSink/StorageEvent) + type-level unit tests |
| **B** | 2 | Storage trait + parametrized test runner skeleton |
| **C** | 3 | MemoryStorage complete impl + trait suite green |
| **D** | 4 | LocalStorage core (no multipart) + trait suite green |
| **E** | 5 | LocalStorage multipart + local-specific tests |
| **F** | 6 | Acceptance docs (changelog + README + 4b prep notes) |

---

## Task 1: Crate skeleton + types (Batch A)

**Files:**
- Create: `crates/crabcloud-storage/Cargo.toml`
- Create: `crates/crabcloud-storage/src/lib.rs`
- Create: `crates/crabcloud-storage/src/error.rs`
- Create: `crates/crabcloud-storage/src/path.rs`
- Create: `crates/crabcloud-storage/src/meta.rs`
- Modify: `Cargo.toml` (workspace) — add deps + new member
- Test: inline `#[cfg(test)] mod tests` in `path.rs`, `meta.rs`, `error.rs`

### Step 1: Branch + workspace dep additions

```
git checkout -b storage-batch-a origin/master
```

Modify root `Cargo.toml`. Find the `[workspace] members = [...]` array and add `"crates/crabcloud-storage",` after `"crates/crabcloud-users",`:

```toml
[workspace]
members = [
    "crates/crabcloud-cache",
    "crates/crabcloud-config",
    "crates/crabcloud-core",
    "crates/crabcloud-db",
    "crates/crabcloud-http",
    "crates/crabcloud-i18n",
    "crates/crabcloud-ocs",
    "crates/crabcloud-server",
    "crates/crabcloud-storage",
    "crates/crabcloud-ui",
    "crates/crabcloud-users",
    "xtask",
]
```

Find `[workspace.dependencies]` and add (alphabetically near existing entries):

```toml
bytes = "1"
infer = { version = "0.16", default-features = false }
phf = { version = "0.11", features = ["macros"] }
phf_codegen = "0.11"
xattr = "1"
```

(Do NOT remove existing deps. Insert in alphabetical position.)

### Step 2: Create `crates/crabcloud-storage/Cargo.toml`

```toml
[package]
name = "crabcloud-storage"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
bytes.workspace = true
hex.workspace = true
infer.workspace = true
phf.workspace = true
rand.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["fs", "io-util", "sync", "macros"] }
tracing.workspace = true

[target.'cfg(unix)'.dependencies]
xattr.workspace = true

[build-dependencies]
phf_codegen.workspace = true

[dev-dependencies]
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "fs", "io-util", "sync", "time"] }

[lints]
workspace = true
```

### Step 3: Create `crates/crabcloud-storage/data/mimetypes.txt`

A text seed file: one `extension<TAB>mimetype` pair per line. Start with a focused set covering the test cases and common types. Full list:

```
txt	text/plain
md	text/markdown
csv	text/csv
log	text/plain
html	text/html
htm	text/html
xml	text/xml
json	application/json
yaml	application/x-yaml
yml	application/x-yaml
toml	application/toml
pdf	application/pdf
doc	application/msword
docx	application/vnd.openxmlformats-officedocument.wordprocessingml.document
xls	application/vnd.ms-excel
xlsx	application/vnd.openxmlformats-officedocument.spreadsheetml.sheet
ppt	application/vnd.ms-powerpoint
pptx	application/vnd.openxmlformats-officedocument.presentationml.presentation
odt	application/vnd.oasis.opendocument.text
ods	application/vnd.oasis.opendocument.spreadsheet
odp	application/vnd.oasis.opendocument.presentation
rtf	application/rtf
zip	application/zip
gz	application/gzip
tar	application/x-tar
7z	application/x-7z-compressed
rar	application/vnd.rar
bz2	application/x-bzip2
xz	application/x-xz
png	image/png
jpg	image/jpeg
jpeg	image/jpeg
gif	image/gif
webp	image/webp
svg	image/svg+xml
bmp	image/bmp
tiff	image/tiff
tif	image/tiff
ico	image/vnd.microsoft.icon
avif	image/avif
heic	image/heic
mp3	audio/mpeg
wav	audio/wav
ogg	audio/ogg
flac	audio/flac
m4a	audio/mp4
aac	audio/aac
opus	audio/opus
mp4	video/mp4
mkv	video/x-matroska
webm	video/webm
mov	video/quicktime
avi	video/x-msvideo
wmv	video/x-ms-wmv
flv	video/x-flv
3gp	video/3gpp
mpg	video/mpeg
mpeg	video/mpeg
js	application/javascript
mjs	application/javascript
ts	application/typescript
tsx	application/typescript
jsx	application/javascript
css	text/css
scss	text/x-scss
sass	text/x-sass
less	text/x-less
rs	text/x-rust
py	text/x-python
rb	text/x-ruby
go	text/x-go
java	text/x-java
c	text/x-c
h	text/x-c
cpp	text/x-c++
hpp	text/x-c++
cc	text/x-c++
hh	text/x-c++
cs	text/x-csharp
swift	text/x-swift
kt	text/x-kotlin
scala	text/x-scala
php	application/x-php
pl	application/x-perl
lua	text/x-lua
sh	application/x-shellscript
bash	application/x-shellscript
zsh	application/x-shellscript
fish	application/x-shellscript
ps1	application/x-powershell
sql	application/sql
ini	text/plain
cfg	text/plain
conf	text/plain
key	application/x-x509-ca-cert
pem	application/x-x509-ca-cert
crt	application/x-x509-ca-cert
cer	application/x-x509-ca-cert
woff	font/woff
woff2	font/woff2
ttf	font/ttf
otf	font/otf
eot	application/vnd.ms-fontobject
epub	application/epub+zip
mobi	application/x-mobipocket-ebook
exe	application/x-msdownload
dll	application/x-msdownload
deb	application/vnd.debian.binary-package
rpm	application/x-rpm
dmg	application/x-apple-diskimage
iso	application/x-iso9660-image
img	application/octet-stream
bin	application/octet-stream
```

(Roughly 100 entries — the spec said ~400 is the goal; this initial set is the most-used subset. Future expansions are additive.)

### Step 4: Create `crates/crabcloud-storage/build.rs`

```rust
//! Builds a static phf::Map<&'static str, &'static str> from
//! `data/mimetypes.txt` (TAB-separated `extension<TAB>mimetype` lines).
//! Generated module is included from `mimetype.rs` via include!.

use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=data/mimetypes.txt");

    let src = fs::read_to_string("data/mimetypes.txt").expect("read mimetypes.txt");
    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR");
    let out_path = Path::new(&out_dir).join("mimetype_map.rs");
    let out_file = fs::File::create(&out_path).expect("create mimetype_map.rs");
    let mut writer = BufWriter::new(out_file);

    let mut map = phf_codegen::Map::<&'static str>::new();
    let mut count: usize = 0;
    // Hold borrowed string references for the duration of map-building. The
    // closure `entry` returns &str references into `lines_owned`, so it has
    // to outlive the map builder.
    let lines_owned: Vec<(String, String)> = src
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (ext, mime) = l.split_once('\t')?;
            Some((ext.trim().to_string(), mime.trim().to_string()))
        })
        .collect();

    let leaked: Vec<(&'static str, &'static str)> = lines_owned
        .iter()
        .map(|(e, m)| (e.as_str(), m.as_str()))
        .map(|(e, m)| {
            // Leak strings so phf_codegen receives 'static refs. Acceptable
            // for a build script: process is one-shot.
            let e: &'static str = Box::leak(e.to_string().into_boxed_str());
            let m: &'static str = Box::leak(m.to_string().into_boxed_str());
            (e, m)
        })
        .collect();

    for (ext, mime) in &leaked {
        map.entry(*ext, &format!("\"{}\"", mime));
        count += 1;
    }

    writeln!(
        &mut writer,
        "/// Auto-generated extension→mimetype map. Do not edit; regenerated\n\
         /// from `data/mimetypes.txt` by `build.rs`.\n\
         pub static EXTENSION_MIMETYPES: phf::Map<&'static str, &'static str> = {};",
        map.build()
    )
    .expect("write map");

    writeln!(
        &mut writer,
        "\n#[cfg(test)]\npub const EXTENSION_COUNT: usize = {};",
        count
    )
    .expect("write count");
}
```

### Step 5: Create `crates/crabcloud-storage/src/error.rs`

```rust
//! Error types for `crabcloud-storage`. `Io` carries the original error for
//! diagnostics; `map_io` lifts well-known `io::ErrorKind`s to the richer
//! variants (NotFound, AlreadyExists, NotEmpty, etc.) before the catch-all.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("directory not empty")]
    NotEmpty,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("multipart: {0}")]
    Multipart(String),
    #[error("storage error: {0}")]
    Other(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Translate a `std::io::Error` into the richest matching `StorageError`
/// variant. Use this in backend code paths instead of relying on the
/// `#[from]` impl when you want richer mapping (most of them do).
pub fn map_io(e: io::Error) -> StorageError {
    match e.kind() {
        io::ErrorKind::NotFound => StorageError::NotFound,
        io::ErrorKind::AlreadyExists => StorageError::AlreadyExists,
        io::ErrorKind::PermissionDenied => StorageError::PermissionDenied,
        // ErrorKind::IsADirectory and NotADirectory exist on nightly; on
        // stable we sniff the os_error code on Unix. Skip OS-specific
        // mapping for now — the common cases above cover most callers; the
        // catch-all preserves the original error for diagnostics.
        _ => StorageError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_io_lifts_not_found() {
        let e = io::Error::new(io::ErrorKind::NotFound, "x");
        assert!(matches!(map_io(e), StorageError::NotFound));
    }

    #[test]
    fn map_io_lifts_already_exists() {
        let e = io::Error::new(io::ErrorKind::AlreadyExists, "x");
        assert!(matches!(map_io(e), StorageError::AlreadyExists));
    }

    #[test]
    fn map_io_lifts_permission_denied() {
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "x");
        assert!(matches!(map_io(e), StorageError::PermissionDenied));
    }

    #[test]
    fn map_io_falls_through_to_io() {
        let e = io::Error::other("weird");
        assert!(matches!(map_io(e), StorageError::Io(_)));
    }

    #[test]
    fn from_io_error_wraps_as_io() {
        let e: StorageError = io::Error::other("x").into();
        assert!(matches!(e, StorageError::Io(_)));
    }
}
```

### Step 6: Create `crates/crabcloud-storage/src/path.rs`

```rust
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
        // Validate every segment.
        for seg in s.split('/') {
            if seg.is_empty() {
                return Err(StorageError::InvalidPath("empty segment".into()));
            }
            if seg == "." || seg == ".." {
                return Err(StorageError::InvalidPath(format!(
                    "illegal segment: {seg}"
                )));
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
```

### Step 7: Create `crates/crabcloud-storage/src/meta.rs`

```rust
//! Metadata types: `FileMetadata`, `DirEntry`, `FileKind`, `ETag`,
//! `Mimetype`, `Permissions`, `MultipartHandle`, `PartTag`.

use crate::error::{StorageError, StorageResult};
use crate::path::StoragePath;
use rand::Rng;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub path: StoragePath,
    pub kind: FileKind,
    pub size: u64,
    pub mtime: SystemTime,
    pub etag: ETag,
    pub mimetype: Mimetype,
    pub permissions: Permissions,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub metadata: FileMetadata,
}

/// 40-char lowercase hex string. Match upstream Nextcloud's ETag shape so
/// existing desktop/iOS/Android clients can detect changes byte-identically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ETag(String);

impl ETag {
    /// Generate a fresh ETag from the workspace CSPRNG.
    pub fn new() -> Self {
        let mut bytes = [0u8; 20];
        rand::rng().fill(&mut bytes);
        Self(hex::encode(bytes))
    }

    /// Parse a pre-existing ETag string. Validates length + hex.
    pub fn from_hex(s: &str) -> StorageResult<Self> {
        if s.len() != 40 {
            return Err(StorageError::Other(format!(
                "etag length: expected 40 hex chars, got {}",
                s.len()
            )));
        }
        if !s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err(StorageError::Other(
                "etag: non-lowercase-hex character".into(),
            ));
        }
        Ok(Self(s.to_string()))
    }

    /// Derive an ETag from mtime + an opaque identifier (e.g. inode number).
    /// Stable across reads, changes on mutation. Lower entropy than `new`,
    /// but required when xattr storage is unavailable.
    pub fn from_mtime_and_id(mtime: SystemTime, id: u64) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut h);
        id.hash(&mut h);
        // Spread 8 hasher bytes into 20 by mixing with rotations.
        let base = h.finish();
        let mut bytes = [0u8; 20];
        for i in 0..20 {
            bytes[i] = ((base.rotate_left((i * 5) as u32)) & 0xff) as u8;
        }
        Self(hex::encode(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ETag {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonical "type/subtype" string. Construct via [`Mimetype::parse`] (validates)
/// or [`Mimetype::octet_stream`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mimetype(String);

impl Mimetype {
    pub fn parse(s: &str) -> StorageResult<Self> {
        if s.is_empty() {
            return Err(StorageError::Other("empty mimetype".into()));
        }
        let mut parts = s.splitn(2, '/');
        let ty = parts
            .next()
            .ok_or_else(|| StorageError::Other("mimetype missing type".into()))?;
        let sub = parts
            .next()
            .ok_or_else(|| StorageError::Other("mimetype missing subtype".into()))?;
        if ty.is_empty() || sub.is_empty() {
            return Err(StorageError::Other("mimetype empty component".into()));
        }
        Ok(Self(s.to_lowercase()))
    }

    pub fn octet_stream() -> Self {
        Self("application/octet-stream".into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bitmap matching upstream Nextcloud's per-file permission model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(u8);

impl Permissions {
    pub const READ: u8 = 1;
    pub const UPDATE: u8 = 2;
    pub const CREATE: u8 = 4;
    pub const DELETE: u8 = 8;
    pub const SHARE: u8 = 16;
    pub const ALL: u8 = Self::READ | Self::UPDATE | Self::CREATE | Self::DELETE | Self::SHARE;

    pub fn new(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    pub fn full() -> Self {
        Self(Self::ALL)
    }

    pub fn readonly() -> Self {
        Self(Self::READ)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Permissions) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Opaque handle to an in-progress multipart upload. The `upload_id` shape
/// is backend-defined (local-fs uses `"local-mp-{random_32}"`; S3 will use
/// AWS's UploadId).
#[derive(Debug, Clone)]
pub struct MultipartHandle {
    pub upload_id: String,
    pub target: StoragePath,
}

/// Caller-replay token for one part of a multipart upload. The `etag` field
/// is backend-defined (S3 returns part ETag; local-fs returns sha256 hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartTag {
    pub part_number: u32,
    pub etag: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_new_is_40_hex_chars() {
        let e = ETag::new();
        assert_eq!(e.as_str().len(), 40);
        assert!(e.as_str().chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn etag_new_is_random() {
        let a = ETag::new();
        let b = ETag::new();
        assert_ne!(a, b);
    }

    #[test]
    fn etag_from_hex_validates_length() {
        assert!(ETag::from_hex("abc").is_err());
    }

    #[test]
    fn etag_from_hex_validates_charset() {
        let s: String = "g".repeat(40);
        assert!(ETag::from_hex(&s).is_err());
    }

    #[test]
    fn etag_from_hex_accepts_valid() {
        let s: String = "0123456789abcdef".repeat(2) + "01234567";
        let e = ETag::from_hex(&s).unwrap();
        assert_eq!(e.as_str(), s);
    }

    #[test]
    fn etag_from_mtime_and_id_is_deterministic() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(123);
        let a = ETag::from_mtime_and_id(t, 42);
        let b = ETag::from_mtime_and_id(t, 42);
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 40);
    }

    #[test]
    fn etag_from_mtime_and_id_changes_on_mutation() {
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2);
        assert_ne!(ETag::from_mtime_and_id(t1, 42), ETag::from_mtime_and_id(t2, 42));
        assert_ne!(ETag::from_mtime_and_id(t1, 42), ETag::from_mtime_and_id(t1, 43));
    }

    #[test]
    fn mimetype_parse_accepts_simple() {
        let m = Mimetype::parse("text/plain").unwrap();
        assert_eq!(m.as_str(), "text/plain");
    }

    #[test]
    fn mimetype_parse_lowercases() {
        let m = Mimetype::parse("Image/PNG").unwrap();
        assert_eq!(m.as_str(), "image/png");
    }

    #[test]
    fn mimetype_parse_rejects_missing_slash() {
        assert!(Mimetype::parse("plain").is_err());
    }

    #[test]
    fn mimetype_parse_rejects_empty_components() {
        assert!(Mimetype::parse("/plain").is_err());
        assert!(Mimetype::parse("text/").is_err());
    }

    #[test]
    fn mimetype_octet_stream() {
        assert_eq!(Mimetype::octet_stream().as_str(), "application/octet-stream");
    }

    #[test]
    fn permissions_constants() {
        assert_eq!(Permissions::READ, 1);
        assert_eq!(Permissions::UPDATE, 2);
        assert_eq!(Permissions::CREATE, 4);
        assert_eq!(Permissions::DELETE, 8);
        assert_eq!(Permissions::SHARE, 16);
        assert_eq!(Permissions::ALL, 31);
    }

    #[test]
    fn permissions_full_and_readonly() {
        assert!(Permissions::full().contains(Permissions::new(Permissions::READ)));
        assert!(Permissions::full().contains(Permissions::new(Permissions::DELETE)));
        assert!(!Permissions::readonly().contains(Permissions::new(Permissions::UPDATE)));
    }

    #[test]
    fn permissions_strips_unknown_bits() {
        let p = Permissions::new(0xff);
        assert_eq!(p.bits(), Permissions::ALL);
    }
}
```

### Step 8: Create `crates/crabcloud-storage/src/lib.rs`

```rust
//! `crabcloud-storage` — async storage primitives.
//!
//! This crate ships the [`Storage`] trait and supporting types. Two backends
//! live in this crate: [`local::LocalStorage`] (production) and
//! [`memory::MemoryStorage`] (tests + dev).
//!
//! Mutating operations take a [`EventSink`] reference. Sub-project 4a ships
//! [`NoopEventSink`]; sub-project 4b will add a real channel-backed sink that
//! drives the filecache scanner.
//!
//! Future backends (S3 in 4b; SMB/external-storage later) implement
//! [`Storage`] and slot into the same call sites.

pub mod error;
pub mod meta;
pub mod path;

pub mod local;
pub mod memory;

pub use error::{StorageError, StorageResult};
pub use meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
pub use path::StoragePath;

use async_trait::async_trait;
use std::ops::Range;
use std::pin::Pin;
use tokio::io::AsyncRead;

/// Events emitted by [`Storage`] operations. Subscribers in sub-project 4b
/// will use these to keep `oc_filecache` in sync with storage state.
#[derive(Debug, Clone)]
pub enum StorageEvent {
    Written {
        storage_id: String,
        path: StoragePath,
        metadata: FileMetadata,
    },
    DirCreated {
        storage_id: String,
        path: StoragePath,
        metadata: FileMetadata,
    },
    Deleted {
        storage_id: String,
        path: StoragePath,
    },
    Moved {
        storage_id: String,
        from: StoragePath,
        to: StoragePath,
    },
    Copied {
        storage_id: String,
        from: StoragePath,
        to: StoragePath,
    },
}

/// Receiver for [`StorageEvent`]s. Emissions are fire-and-forget — a failing
/// emit must NOT roll back the storage operation. Failures are logged.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: StorageEvent);
}

/// No-op sink used in sub-project 4a tests and as the default. 4b adds a
/// channel-backed implementation that fans out to subscribed consumers.
pub struct NoopEventSink;

#[async_trait]
impl EventSink for NoopEventSink {
    async fn emit(&self, _event: StorageEvent) {}
}

/// The storage trait. All mutating methods take `&dyn EventSink` so callers
/// can subscribe to the resulting events.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Stable identifier for this storage. Used as `storage_id` in events
    /// and (in 4b) as the foreign-key value for `oc_filecache.storage`.
    fn id(&self) -> &str;

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata>;
    async fn exists(&self, path: &StoragePath) -> StorageResult<bool>;
    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>>;

    async fn read(
        &self,
        path: &StoragePath,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>>;

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn mkdir(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn delete(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()>;

    async fn begin_multipart(
        &self,
        target: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle>;

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag>;

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata>;

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn storage_trait_is_object_safe() {
        // Compile-only assertion. If this fails to compile, someone added a
        // non-object-safe method (generic on the trait method, Self in a
        // non-receiver position, etc.).
        fn _accepts(_s: Arc<dyn Storage>) {}
    }

    #[test]
    fn event_sink_is_object_safe() {
        fn _accepts(_s: Arc<dyn EventSink>) {}
    }

    #[tokio::test]
    async fn noop_sink_swallows_events() {
        let sink = NoopEventSink;
        sink.emit(StorageEvent::Deleted {
            storage_id: "x".into(),
            path: StoragePath::root(),
        })
        .await;
    }
}
```

### Step 9: Create empty `local/mod.rs` and `memory/mod.rs` stubs

These are needed so `lib.rs`'s `pub mod local;` and `pub mod memory;` compile. Implementations land in later batches.

Create `crates/crabcloud-storage/src/local/mod.rs`:

```rust
//! Local filesystem backend. Implementation lands in Batches D + E.
```

Create `crates/crabcloud-storage/src/memory/mod.rs`:

```rust
//! In-memory backend for tests + dev. Implementation lands in Batch C.
```

### Step 10: Run + commit + push + open Batch A PR

```
cargo build -p crabcloud-storage
cargo test -p crabcloud-storage --lib
cargo xtask check-all
```

Expected: builds clean; ~40 tests pass (error: 5, path: 18, meta: 14, lib: 3); workspace check-all green.

```
git add Cargo.toml crates/crabcloud-storage
git commit -m "feat(storage): crabcloud-storage crate skeleton — types + EventSink trait

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-a
gh pr create --base master --head storage-batch-a \
  --title "storage: batch A — crate skeleton + types + EventSink" \
  --body "Sub-project 4a, batch A: new crabcloud-storage crate. Types only (StoragePath, FileMetadata, ETag, Mimetype, Permissions, MultipartHandle, PartTag, StorageError, StorageEvent), EventSink trait + NoopEventSink, and the Storage trait declaration (no impls yet). Type-level unit tests cover normalization rules, ETag shape, mimetype parsing, permissions bitmap."
```

**STOP.** Do NOT call `gh pr merge`. Controller merges after CI greens.

---

## Task 2: Storage trait test runner skeleton (Batch B)

**Files:**
- Create: `crates/crabcloud-storage/tests/support/mod.rs`
- Create: `crates/crabcloud-storage/tests/trait_suite.rs`

### Step 1: Branch

```
git checkout -b storage-batch-b origin/master
```

### Step 2: Create `tests/support/mod.rs`

```rust
//! Test fixtures shared across `trait_suite.rs`, `local_specific.rs`,
//! and `memory_specific.rs`.

#![allow(dead_code)]

use crabcloud_storage::{EventSink, StorageEvent};
use std::sync::{Arc, Mutex};

/// EventSink that buffers every emission into a `Vec` for assertions.
#[derive(Clone, Default)]
pub struct RecordingSink {
    pub events: Arc<Mutex<Vec<StorageEvent>>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn drain(&self) -> Vec<StorageEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    pub fn snapshot(&self) -> Vec<StorageEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl EventSink for RecordingSink {
    async fn emit(&self, event: StorageEvent) {
        self.events.lock().unwrap().push(event);
    }
}
```

### Step 3: Create `tests/trait_suite.rs` — runner skeleton

```rust
//! Parametrized trait suite. Both backends (MemoryStorage in Batch C,
//! LocalStorage in Batch D) invoke `run_storage_suite` via their own
//! top-level test functions.
//!
//! Adding a backend in this crate? Add a top-level `#[tokio::test]` that
//! calls `run_storage_suite("backend_name", || your_factory()).await`.

mod support;

use crabcloud_storage::{
    DirEntry, FileKind, NoopEventSink, Storage, StorageError, StoragePath,
};
use std::sync::Arc;
use support::RecordingSink;
use tokio::io::AsyncReadExt;

/// Drive the full battery of trait-level assertions against `factory()`,
/// which must produce a fresh, empty storage on each call.
pub async fn run_storage_suite<S: Storage + 'static>(
    name: &str,
    factory: impl Fn() -> S + Send + Sync,
) {
    eprintln!("--- storage suite: {name} ---");

    path_invariants();
    write_then_read(&factory).await;
    write_overwrite_changes_etag(&factory).await;
    stat_after_write(&factory).await;
    read_range_returns_slice(&factory).await;
    mkdir_then_list_includes_dir(&factory).await;
    write_to_dir_lists_correctly(&factory).await;
    delete_file_then_stat_404(&factory).await;
    delete_empty_dir_ok_nonempty_errs(&factory).await;
    rename_moves(&factory).await;
    copy_preserves_contents_changes_etag(&factory).await;
    multipart_happy_path(&factory).await;
    multipart_abort_drops_target(&factory).await;
    multipart_gap_rejected(&factory).await;
    multipart_duplicate_rejected(&factory).await;
    event_sink_emits_one_per_mutation(&factory).await;
}

// --- individual assertions (each is a pure async fn against a fresh storage) ---

fn path_invariants() {
    // These don't depend on the backend; assert constructor behavior here
    // so `StoragePath` is sanity-checked at the integration boundary too.
    assert!(matches!(
        StoragePath::new(""),
        Ok(_)
    ));
    assert!(matches!(
        StoragePath::new("/abs"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a/../b"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a/./b"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a\0b"),
        Err(StorageError::InvalidPath(_))
    ));
}

async fn write_then_read<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("hello.txt").unwrap();
    let body = make_body(b"hi");
    let sink = NoopEventSink;
    let meta = storage.put_file(&path, body, &sink).await.unwrap();
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.size, 2);
    let mut reader = storage.read(&path).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hi");
}

async fn write_overwrite_changes_etag<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("over.txt").unwrap();
    let sink = NoopEventSink;
    let a = storage.put_file(&path, make_body(b"v1"), &sink).await.unwrap();
    let b = storage.put_file(&path, make_body(b"v2"), &sink).await.unwrap();
    assert_ne!(a.etag, b.etag);
}

async fn stat_after_write<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("stat.txt").unwrap();
    let sink = NoopEventSink;
    storage.put_file(&path, make_body(b"data"), &sink).await.unwrap();
    let meta = storage.stat(&path).await.unwrap();
    assert_eq!(meta.size, 4);
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.etag.as_str().len(), 40);
}

async fn read_range_returns_slice<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("range.txt").unwrap();
    let sink = NoopEventSink;
    storage.put_file(&path, make_body(b"abcdefghij"), &sink).await.unwrap();
    let mut reader = storage.read_range(&path, 2..5).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"cde");
}

async fn mkdir_then_list_includes_dir<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let dir = StoragePath::new("d1").unwrap();
    let sink = NoopEventSink;
    storage.mkdir(&dir, &sink).await.unwrap();
    let listing = storage.list(&StoragePath::root()).await.unwrap();
    let found = listing.iter().find(|e: &&DirEntry| e.name == "d1").expect("d1 in root listing");
    assert_eq!(found.metadata.kind, FileKind::Directory);
}

async fn write_to_dir_lists_correctly<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = NoopEventSink;
    storage.mkdir(&StoragePath::new("d").unwrap(), &sink).await.unwrap();
    storage.put_file(&StoragePath::new("d/x.txt").unwrap(), make_body(b"x"), &sink).await.unwrap();
    let listing = storage.list(&StoragePath::new("d").unwrap()).await.unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].name, "x.txt");
}

async fn delete_file_then_stat_404<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("del.txt").unwrap();
    let sink = NoopEventSink;
    storage.put_file(&path, make_body(b"x"), &sink).await.unwrap();
    storage.delete(&path, &sink).await.unwrap();
    assert!(matches!(storage.stat(&path).await, Err(StorageError::NotFound)));
}

async fn delete_empty_dir_ok_nonempty_errs<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = NoopEventSink;
    storage.mkdir(&StoragePath::new("empty").unwrap(), &sink).await.unwrap();
    storage.delete(&StoragePath::new("empty").unwrap(), &sink).await.unwrap();

    storage.mkdir(&StoragePath::new("full").unwrap(), &sink).await.unwrap();
    storage.put_file(&StoragePath::new("full/x.txt").unwrap(), make_body(b"x"), &sink).await.unwrap();
    assert!(matches!(
        storage.delete(&StoragePath::new("full").unwrap(), &sink).await,
        Err(StorageError::NotEmpty)
    ));
}

async fn rename_moves<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let from = StoragePath::new("from.txt").unwrap();
    let to = StoragePath::new("to.txt").unwrap();
    let sink = NoopEventSink;
    storage.put_file(&from, make_body(b"x"), &sink).await.unwrap();
    storage.rename(&from, &to, &sink).await.unwrap();
    assert!(matches!(storage.stat(&from).await, Err(StorageError::NotFound)));
    let mut buf = Vec::new();
    storage.read(&to).await.unwrap().read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"x");
}

async fn copy_preserves_contents_changes_etag<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let src = StoragePath::new("src.txt").unwrap();
    let dst = StoragePath::new("dst.txt").unwrap();
    let sink = NoopEventSink;
    let a = storage.put_file(&src, make_body(b"copy-me"), &sink).await.unwrap();
    storage.copy(&src, &dst, &sink).await.unwrap();
    let b = storage.stat(&dst).await.unwrap();
    assert_ne!(a.etag, b.etag);
    let mut buf = Vec::new();
    storage.read(&dst).await.unwrap().read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"copy-me");
}

async fn multipart_happy_path<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("big.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1 = storage.put_part(&handle, 1, make_body(b"AAA")).await.unwrap();
    let t2 = storage.put_part(&handle, 2, make_body(b"BBB")).await.unwrap();
    storage.commit_multipart(handle, vec![t1, t2], &sink).await.unwrap();
    let mut buf = Vec::new();
    storage.read(&target).await.unwrap().read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"AAABBB");
}

async fn multipart_abort_drops_target<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("aborted.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    storage.put_part(&handle, 1, make_body(b"AAA")).await.unwrap();
    storage.abort_multipart(handle).await.unwrap();
    assert!(matches!(storage.stat(&target).await, Err(StorageError::NotFound)));
}

async fn multipart_gap_rejected<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("gap.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1 = storage.put_part(&handle, 1, make_body(b"AAA")).await.unwrap();
    let t3 = storage.put_part(&handle, 3, make_body(b"CCC")).await.unwrap();
    let err = storage.commit_multipart(handle, vec![t1, t3], &sink).await.unwrap_err();
    assert!(matches!(err, StorageError::Multipart(_)));
}

async fn multipart_duplicate_rejected<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("dup.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1a = storage.put_part(&handle, 1, make_body(b"AAA")).await.unwrap();
    let t1b = storage.put_part(&handle, 1, make_body(b"BBB")).await.unwrap();
    let err = storage.commit_multipart(handle, vec![t1a, t1b], &sink).await.unwrap_err();
    assert!(matches!(err, StorageError::Multipart(_)));
}

async fn event_sink_emits_one_per_mutation<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = RecordingSink::new();
    storage.put_file(&StoragePath::new("a").unwrap(), make_body(b"x"), &sink).await.unwrap();
    storage.mkdir(&StoragePath::new("d").unwrap(), &sink).await.unwrap();
    storage.rename(&StoragePath::new("a").unwrap(), &StoragePath::new("b").unwrap(), &sink).await.unwrap();
    storage.copy(&StoragePath::new("b").unwrap(), &StoragePath::new("c").unwrap(), &sink).await.unwrap();
    storage.delete(&StoragePath::new("c").unwrap(), &sink).await.unwrap();
    let events = sink.snapshot();
    assert_eq!(events.len(), 5);
    let _ = Arc::new(events); // keep variable used; aids future inspection
}

// --- helpers ---

fn make_body(bytes: &'static [u8]) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes.to_vec())) as _
}
```

Note: `std::io::Cursor<Vec<u8>>` does NOT implement `tokio::io::AsyncRead` by default. We need a thin shim:

Replace the `make_body` helper above with:

```rust
fn make_body(bytes: &'static [u8]) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(tokio::io::BufReader::new(&bytes[..]))
}
```

`&[u8]` does implement `tokio::io::AsyncRead` directly (via `tokio` blanket impls for `&[u8]`). Wrapping in `BufReader` keeps the type uniform.

### Step 4: Verify it compiles (no backends yet)

```
cargo build -p crabcloud-storage --tests
```

Expected: compiles. No tests are runnable since no top-level `#[tokio::test]` calls `run_storage_suite` yet. Batch C adds the Memory backend's call; Batch D adds Local's.

### Step 5: Commit + push + open Batch B PR

```
git add crates/crabcloud-storage/tests
git commit -m "test(storage): parametrized trait suite + RecordingSink

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-b
gh pr create --base master --head storage-batch-b \
  --title "storage: batch B — parametrized trait suite skeleton" \
  --body "Sub-project 4a, batch B: the parametrized trait test runner. Compiles cleanly; not yet invoked by any test since no backends ship in this batch. Memory backend wires up the runner in batch C, Local in batch D."
```

**STOP.**

---

## Task 3: MemoryStorage (Batch C)

**Files:**
- Modify: `crates/crabcloud-storage/src/memory/mod.rs` (replace stub)
- Modify: `crates/crabcloud-storage/tests/trait_suite.rs` (add MemoryStorage runner)

### Step 1: Branch

```
git checkout -b storage-batch-c origin/master
```

### Step 2: Replace `src/memory/mod.rs`

```rust
//! In-memory backend. `Arc<RwLock<MemTree>>` around a `BTreeMap` keyed by
//! [`StoragePath`]. Coarse but adequate for test + dev workloads.
//!
//! Multipart uploads buffer parts in a per-handle `Mutex<BTreeMap<u32, Bytes>>`
//! stored in a sibling `BTreeMap` keyed by upload id.

use crate::error::{StorageError, StorageResult};
use crate::meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
use crate::path::StoragePath;
use crate::{EventSink, Storage, StorageEvent};
use async_trait::async_trait;
use bytes::Bytes;
use rand::Rng;
use std::collections::BTreeMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone)]
enum MemEntry {
    File {
        bytes: Bytes,
        etag: ETag,
        mtime: SystemTime,
        mimetype: Mimetype,
    },
    Directory {
        etag: ETag,
        mtime: SystemTime,
    },
}

impl MemEntry {
    fn to_metadata(&self, path: StoragePath) -> FileMetadata {
        match self {
            MemEntry::File {
                bytes,
                etag,
                mtime,
                mimetype,
            } => FileMetadata {
                path,
                kind: FileKind::File,
                size: bytes.len() as u64,
                mtime: *mtime,
                etag: etag.clone(),
                mimetype: mimetype.clone(),
                permissions: Permissions::full(),
            },
            MemEntry::Directory { etag, mtime } => FileMetadata {
                path,
                kind: FileKind::Directory,
                size: 0,
                mtime: *mtime,
                etag: etag.clone(),
                mimetype: Mimetype::octet_stream(),
                permissions: Permissions::full(),
            },
        }
    }
}

#[derive(Default)]
struct MemTree {
    entries: BTreeMap<StoragePath, MemEntry>,
    /// Upload-id → ordered map of part-number → bytes. Each insertion replaces
    /// any existing key, so `put_part(n, …)` overwrites.
    uploads: BTreeMap<String, Arc<Mutex<BTreeMap<u32, Bytes>>>>,
}

pub struct MemoryStorage {
    id: String,
    inner: Arc<RwLock<MemTree>>,
}

impl MemoryStorage {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: format!("memory::{}", id.into()),
            inner: Arc::new(RwLock::new(MemTree::default())),
        }
    }

    fn ensure_parents(tree: &mut MemTree, path: &StoragePath) -> StorageResult<()> {
        // Implicit-mkdir: traverse path segments, materialize each ancestor as
        // a Directory entry if absent. Documented asymmetry vs LocalStorage.
        let segs: Vec<&str> = path.as_str().split('/').collect();
        if segs.len() <= 1 {
            return Ok(());
        }
        let mut cur = String::new();
        for seg in &segs[..segs.len() - 1] {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(seg);
            let p = StoragePath::new(cur.clone())?;
            match tree.entries.get(&p) {
                Some(MemEntry::File { .. }) => return Err(StorageError::NotADirectory),
                Some(MemEntry::Directory { .. }) => {}
                None => {
                    tree.entries.insert(
                        p,
                        MemEntry::Directory {
                            etag: ETag::new(),
                            mtime: SystemTime::now(),
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        let tree = self.inner.read().unwrap();
        tree.entries
            .get(path)
            .map(|e| e.to_metadata(path.clone()))
            .ok_or(StorageError::NotFound)
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        Ok(self.inner.read().unwrap().entries.contains_key(path))
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        let tree = self.inner.read().unwrap();
        if !path.is_root() {
            match tree.entries.get(path) {
                Some(MemEntry::Directory { .. }) => {}
                Some(MemEntry::File { .. }) => return Err(StorageError::NotADirectory),
                None => return Err(StorageError::NotFound),
            }
        }
        let prefix = if path.is_root() {
            String::new()
        } else {
            format!("{}/", path.as_str())
        };
        let mut out = Vec::new();
        for (k, v) in &tree.entries {
            let s = k.as_str();
            if !s.starts_with(&prefix) {
                continue;
            }
            let rest = &s[prefix.len()..];
            if rest.is_empty() || rest.contains('/') {
                // Either the dir itself, or a deeper grandchild.
                continue;
            }
            out.push(DirEntry {
                name: rest.to_string(),
                metadata: v.to_metadata(k.clone()),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(
        &self,
        path: &StoragePath,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let tree = self.inner.read().unwrap();
        match tree.entries.get(path) {
            Some(MemEntry::File { bytes, .. }) => {
                let buf = bytes.clone();
                Ok(Box::pin(std::io::Cursor::new(buf.to_vec())) as Pin<Box<dyn AsyncRead + Send>>)
            }
            Some(MemEntry::Directory { .. }) => Err(StorageError::IsADirectory),
            None => Err(StorageError::NotFound),
        }
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let tree = self.inner.read().unwrap();
        match tree.entries.get(path) {
            Some(MemEntry::File { bytes, .. }) => {
                let start = range.start as usize;
                let end = (range.end as usize).min(bytes.len());
                let slice = bytes.slice(start..end);
                Ok(Box::pin(std::io::Cursor::new(slice.to_vec())) as Pin<Box<dyn AsyncRead + Send>>)
            }
            Some(MemEntry::Directory { .. }) => Err(StorageError::IsADirectory),
            None => Err(StorageError::NotFound),
        }
    }

    async fn put_file(
        &self,
        path: &StoragePath,
        mut body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let mut buf = Vec::new();
        body.read_to_end(&mut buf).await?;
        let bytes = Bytes::from(buf);
        let mimetype = sniff_mimetype(path.as_str(), &bytes);

        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            if let Some(MemEntry::Directory { .. }) = tree.entries.get(path) {
                return Err(StorageError::IsADirectory);
            }
            Self::ensure_parents(&mut tree, path)?;
            let entry = MemEntry::File {
                bytes: bytes.clone(),
                etag: ETag::new(),
                mtime: SystemTime::now(),
                mimetype,
            };
            metadata = entry.to_metadata(path.clone());
            tree.entries.insert(path.clone(), entry);
        }

        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn mkdir(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            if tree.entries.contains_key(path) {
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, path)?;
            let entry = MemEntry::Directory {
                etag: ETag::new(),
                mtime: SystemTime::now(),
            };
            metadata = entry.to_metadata(path.clone());
            tree.entries.insert(path.clone(), entry);
        }
        sink.emit(StorageEvent::DirCreated {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()> {
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree
                .entries
                .get(path)
                .ok_or(StorageError::NotFound)?
                .clone();
            if let MemEntry::Directory { .. } = entry {
                let prefix = format!("{}/", path.as_str());
                let has_children = tree.entries.keys().any(|k| k.as_str().starts_with(&prefix));
                if has_children {
                    return Err(StorageError::NotEmpty);
                }
            }
            tree.entries.remove(path);
        }
        sink.emit(StorageEvent::Deleted {
            storage_id: self.id.clone(),
            path: path.clone(),
        })
        .await;
        Ok(())
    }

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree.entries.remove(from).ok_or(StorageError::NotFound)?;
            if tree.entries.contains_key(to) {
                // Restore original
                tree.entries.insert(from.clone(), entry);
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, to)?;
            tree.entries.insert(to.clone(), entry);
        }
        sink.emit(StorageEvent::Moved {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree
                .entries
                .get(from)
                .ok_or(StorageError::NotFound)?
                .clone();
            if tree.entries.contains_key(to) {
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, to)?;
            let new_entry = match entry {
                MemEntry::File { bytes, mimetype, .. } => MemEntry::File {
                    bytes,
                    etag: ETag::new(),
                    mtime: SystemTime::now(),
                    mimetype,
                },
                MemEntry::Directory { .. } => MemEntry::Directory {
                    etag: ETag::new(),
                    mtime: SystemTime::now(),
                },
            };
            tree.entries.insert(to.clone(), new_entry);
        }
        sink.emit(StorageEvent::Copied {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn begin_multipart(
        &self,
        target: &StoragePath,
        _sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        let mut rng = rand::rng();
        let mut id_bytes = [0u8; 16];
        rng.fill(&mut id_bytes);
        let upload_id = format!("mem-mp-{}", hex::encode(id_bytes));
        let mut tree = self.inner.write().unwrap();
        tree.uploads
            .insert(upload_id.clone(), Arc::new(Mutex::new(BTreeMap::new())));
        Ok(MultipartHandle {
            upload_id,
            target: target.clone(),
        })
    }

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        mut body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        let mut buf = Vec::new();
        body.read_to_end(&mut buf).await?;
        let bytes = Bytes::from(buf);
        let etag = hex::encode({
            use std::collections::hash_map::DefaultHasher;
            use std::hash::Hasher;
            let mut h = DefaultHasher::new();
            h.write(&bytes);
            h.write_u32(part_number);
            h.finish().to_le_bytes()
        });
        let parts = {
            let tree = self.inner.read().unwrap();
            tree.uploads
                .get(&handle.upload_id)
                .cloned()
                .ok_or_else(|| StorageError::Multipart("unknown upload id".into()))?
        };
        parts.lock().unwrap().insert(part_number, bytes);
        Ok(PartTag { part_number, etag })
    }

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        if parts.is_empty() {
            return Err(StorageError::Multipart("no parts".into()));
        }
        // Validate contiguous, starts at 1, no duplicates.
        let mut nums: Vec<u32> = parts.iter().map(|p| p.part_number).collect();
        nums.sort_unstable();
        for (i, n) in nums.iter().enumerate() {
            if (*n as usize) != i + 1 {
                return Err(StorageError::Multipart(format!(
                    "expected contiguous parts starting at 1; got {n} at index {i}"
                )));
            }
        }
        let mut prev = 0u32;
        for n in &nums {
            if *n == prev {
                return Err(StorageError::Multipart(format!("duplicate part {n}")));
            }
            prev = *n;
        }

        let upload = {
            let mut tree = self.inner.write().unwrap();
            tree.uploads
                .remove(&handle.upload_id)
                .ok_or_else(|| StorageError::Multipart("unknown upload id".into()))?
        };
        let map = upload.lock().unwrap();
        let mut buf = Vec::new();
        for tag in &parts {
            let part_bytes = map
                .get(&tag.part_number)
                .ok_or_else(|| StorageError::Multipart(format!("missing part {}", tag.part_number)))?;
            buf.extend_from_slice(part_bytes);
        }
        drop(map);
        let bytes = Bytes::from(buf);
        let mimetype = sniff_mimetype(handle.target.as_str(), &bytes);

        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            Self::ensure_parents(&mut tree, &handle.target)?;
            let entry = MemEntry::File {
                bytes: bytes.clone(),
                etag: ETag::new(),
                mtime: SystemTime::now(),
                mimetype,
            };
            metadata = entry.to_metadata(handle.target.clone());
            tree.entries.insert(handle.target.clone(), entry);
        }
        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: handle.target.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()> {
        let mut tree = self.inner.write().unwrap();
        tree.uploads.remove(&handle.upload_id);
        Ok(())
    }
}

/// Mini extension-or-octet-stream mimetype guesser for the memory backend.
/// Used by the trait-suite tests; doesn't go through the phf table (the
/// Local backend handles that more thoroughly in batch D).
fn sniff_mimetype(path: &str, _bytes: &[u8]) -> Mimetype {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..].to_ascii_lowercase();
        if ext == "txt" {
            return Mimetype::parse("text/plain").unwrap();
        }
    }
    Mimetype::octet_stream()
}
```

### Step 3: Wire MemoryStorage into the trait suite

Append at the bottom of `crates/crabcloud-storage/tests/trait_suite.rs`:

```rust
// --- backends ---

#[tokio::test]
async fn memory_backend_passes_trait_suite() {
    run_storage_suite("memory", || {
        crabcloud_storage::memory::MemoryStorage::new("test")
    })
    .await;
}
```

And in the `use` block at the top, make sure `crabcloud_storage::memory` is reachable — already is since `lib.rs` declares `pub mod memory;`.

### Step 4: Add memory-specific tests

Create `crates/crabcloud-storage/tests/memory_specific.rs`:

```rust
//! Memory-backend-specific tests: concurrent writes.

mod support;

use crabcloud_storage::memory::MemoryStorage;
use crabcloud_storage::{NoopEventSink, Storage, StoragePath};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_paths_all_succeed() {
    let storage = Arc::new(MemoryStorage::new("concurrent-distinct"));
    let mut handles = Vec::new();
    for i in 0..100u32 {
        let storage = storage.clone();
        handles.push(tokio::spawn(async move {
            let path = StoragePath::new(format!("f-{i:03}.txt")).unwrap();
            storage
                .put_file(&path, body(format!("v-{i}").into_bytes()), &NoopEventSink)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let listing = storage.list(&StoragePath::root()).await.unwrap();
    assert_eq!(listing.len(), 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_path_last_writer_wins() {
    let storage = Arc::new(MemoryStorage::new("concurrent-same"));
    let path = StoragePath::new("contended.txt").unwrap();
    let mut handles = Vec::new();
    for i in 0..100u32 {
        let storage = storage.clone();
        let path = path.clone();
        handles.push(tokio::spawn(async move {
            storage
                .put_file(&path, body(format!("{i:03}").into_bytes()), &NoopEventSink)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut reader = storage.read(&path).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    let s = String::from_utf8(buf).unwrap();
    // Some value in 000..=099 won; we don't care which.
    assert_eq!(s.len(), 3);
    assert!(s.chars().all(|c| c.is_ascii_digit()));
}
```

### Step 5: Run + commit + push + open Batch C PR

```
cargo test -p crabcloud-storage
cargo xtask check-all
```

Expected: 15 trait-suite assertions pass via `memory_backend_passes_trait_suite`; 2 memory-specific tests pass; lib-level unit tests still pass.

```
git add crates/crabcloud-storage
git commit -m "feat(storage): MemoryStorage backend + trait suite green for memory

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-c
gh pr create --base master --head storage-batch-c \
  --title "storage: batch C — MemoryStorage + trait suite green" \
  --body "Sub-project 4a, batch C: MemoryStorage complete implementation. Single-RwLock around BTreeMap-of-paths; multipart via per-handle byte map. First backend through the parametrized trait suite (15 assertions). Adds 2 memory-specific tests (concurrent distinct paths, concurrent same-path last-writer-wins)."
```

**STOP.**

---

## Task 4: LocalStorage core (Batch D)

**Files:**
- Modify: `crates/crabcloud-storage/src/local/mod.rs` (replace stub)
- Create: `crates/crabcloud-storage/src/local/atomic.rs`
- Create: `crates/crabcloud-storage/src/local/mimetype.rs`
- Create: `crates/crabcloud-storage/src/local/xattr_io.rs`
- Modify: `crates/crabcloud-storage/tests/trait_suite.rs` (add LocalStorage runner)

### Step 1: Branch

```
git checkout -b storage-batch-d origin/master
```

### Step 2: Create `src/local/xattr_io.rs`

```rust
//! ETag + mimetype xattr persistence. Unix uses the `xattr` crate; Windows
//! falls back to the mtime+inode-derived ETag for ETag and to extension/sniff
//! for mimetype (no persistence — recomputed on each stat).

use crate::error::StorageError;
use crate::meta::{ETag, Mimetype};
use std::path::Path;

const ETAG_KEY: &str = "user.crabcloud.etag";
const MIME_KEY: &str = "user.crabcloud.mimetype";

#[cfg(unix)]
pub fn read_etag(p: &Path) -> Option<ETag> {
    xattr::get(p, ETAG_KEY)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| ETag::from_hex(&s).ok())
}

#[cfg(unix)]
pub fn write_etag(p: &Path, etag: &ETag) -> Result<(), StorageError> {
    // Best-effort. If xattr is unsupported, swallow + log; ETag fallback
    // path produces a usable (deterministic) value.
    if let Err(e) = xattr::set(p, ETAG_KEY, etag.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "xattr etag write failed");
    }
    Ok(())
}

#[cfg(unix)]
pub fn read_mimetype(p: &Path) -> Option<Mimetype> {
    xattr::get(p, MIME_KEY)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| Mimetype::parse(&s).ok())
}

#[cfg(unix)]
pub fn write_mimetype(p: &Path, m: &Mimetype) -> Result<(), StorageError> {
    if let Err(e) = xattr::set(p, MIME_KEY, m.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "xattr mimetype write failed");
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn read_etag(_p: &Path) -> Option<ETag> {
    None
}
#[cfg(not(unix))]
pub fn write_etag(_p: &Path, _etag: &ETag) -> Result<(), StorageError> {
    Ok(())
}
#[cfg(not(unix))]
pub fn read_mimetype(_p: &Path) -> Option<Mimetype> {
    None
}
#[cfg(not(unix))]
pub fn write_mimetype(_p: &Path, _m: &Mimetype) -> Result<(), StorageError> {
    Ok(())
}
```

### Step 3: Create `src/local/mimetype.rs`

```rust
//! Mimetype detection: extension lookup against the build-script-generated
//! phf map, then magic-byte sniffing via the `infer` crate. Final fallback
//! is `application/octet-stream`.

use crate::meta::Mimetype;

include!(concat!(env!("OUT_DIR"), "/mimetype_map.rs"));

/// Best-effort mimetype from path extension. Returns `None` if no entry.
pub fn from_extension(path: &str) -> Option<Mimetype> {
    let idx = path.rfind('.')?;
    let ext = path[idx + 1..].to_ascii_lowercase();
    EXTENSION_MIMETYPES
        .get(ext.as_str())
        .and_then(|s| Mimetype::parse(s).ok())
}

/// Magic-byte sniff on the first 4096 bytes of a file body.
pub fn sniff_magic(head: &[u8]) -> Option<Mimetype> {
    infer::get(head).and_then(|t| Mimetype::parse(t.mime_type()).ok())
}

/// Best-effort combined detection: extension → magic → octet-stream.
pub fn detect(path: &str, head: &[u8]) -> Mimetype {
    if let Some(m) = from_extension(path) {
        return m;
    }
    if let Some(m) = sniff_magic(head) {
        return m;
    }
    Mimetype::octet_stream()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_table_is_seeded() {
        assert!(EXTENSION_COUNT > 50);
    }

    #[test]
    fn extension_lookup_known_types() {
        assert_eq!(from_extension("x.txt").unwrap().as_str(), "text/plain");
        assert_eq!(from_extension("x.png").unwrap().as_str(), "image/png");
        assert_eq!(from_extension("Photo.JPG").unwrap().as_str(), "image/jpeg");
        assert_eq!(from_extension("doc.pdf").unwrap().as_str(), "application/pdf");
    }

    #[test]
    fn extension_lookup_unknown_returns_none() {
        assert!(from_extension("x.unknownextension").is_none());
        assert!(from_extension("noext").is_none());
    }

    #[test]
    fn sniff_magic_detects_png() {
        // PNG signature
        let head = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(sniff_magic(head).unwrap().as_str(), "image/png");
    }

    #[test]
    fn sniff_magic_returns_none_on_unknown_bytes() {
        let head = b"random text content here";
        assert!(sniff_magic(head).is_none());
    }

    #[test]
    fn detect_prefers_extension_over_sniff() {
        // PNG bytes but with .txt extension — extension wins.
        let head = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(detect("misnamed.txt", head).as_str(), "text/plain");
    }

    #[test]
    fn detect_falls_through_to_octet_stream() {
        assert_eq!(detect("noext", b"random").as_str(), "application/octet-stream");
    }
}
```

### Step 4: Create `src/local/atomic.rs`

```rust
//! Atomic-write sequence for `put_file` and `commit_multipart`. Stream to a
//! sibling temp file, fsync, rename, fsync parent dir. Temp file is cleaned
//! up on Drop if the rename hasn't fired.

use crate::error::{map_io, StorageError, StorageResult};
use rand::Rng;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// RAII guard that removes the temp file on drop unless `forget()` is called.
pub struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref().expect("guard already consumed")
    }

    pub fn forget(mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Make a sibling temp path under the same directory as `target`.
pub fn sibling_temp(target: &Path) -> StorageResult<PathBuf> {
    let parent = target.parent().ok_or_else(|| {
        StorageError::InvalidPath(format!(
            "no parent for {}",
            target.display()
        ))
    })?;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    Ok(parent.join(format!(".tmp-crabcloud-{}", hex::encode(bytes))))
}

/// fsync a file handle.
pub async fn fsync_file(f: &File) -> StorageResult<()> {
    f.sync_all().await.map_err(map_io)?;
    Ok(())
}

/// fsync a directory (POSIX only — no-op on Windows).
pub async fn fsync_dir(dir: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        let f = tokio::fs::File::open(dir).await.map_err(map_io)?;
        f.sync_all().await.map_err(map_io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

/// Atomic-rename a file: rename + fsync parent.
pub async fn atomic_rename(from: &Path, to: &Path) -> StorageResult<()> {
    tokio::fs::rename(from, to).await.map_err(map_io)?;
    if let Some(parent) = to.parent() {
        fsync_dir(parent).await?;
    }
    Ok(())
}

/// Stream `body` into a fresh temp file at `temp_path`, fsync, return the
/// open file handle (for callers that want to set xattrs before rename).
pub async fn stream_to_temp(
    temp_path: &Path,
    mut body: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
) -> StorageResult<File> {
    use tokio::io::AsyncReadExt;
    let mut f = File::create(temp_path).await.map_err(map_io)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = body.read(&mut buf).await.map_err(map_io)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).await.map_err(map_io)?;
    }
    fsync_file(&f).await?;
    Ok(f)
}
```

### Step 5: Create `src/local/mod.rs` — the main LocalStorage impl

Replace `crates/crabcloud-storage/src/local/mod.rs`:

```rust
//! Local filesystem backend. Atomic writes via tempfile + rename + fsync.
//! ETag + mimetype persisted via xattr (Unix) with a mtime+inode fallback.
//! Multipart writes lives in batch E (`begin_multipart`/`put_part`/
//! `commit_multipart`/`abort_multipart`).

mod atomic;
mod mimetype;
mod xattr_io;

use crate::error::{map_io, StorageError, StorageResult};
use crate::meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
use crate::path::StoragePath;
use crate::{EventSink, Storage, StorageEvent};
use async_trait::async_trait;
use atomic::{atomic_rename, sibling_temp, stream_to_temp, TempFileGuard};
use std::io::SeekFrom;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

pub struct LocalStorage {
    root: PathBuf,
    id: String,
}

impl LocalStorage {
    pub fn new(root: PathBuf) -> StorageResult<Self> {
        let root = root.canonicalize().map_err(map_io)?;
        let id = format!("local::{}", root.display());
        Ok(Self { root, id })
    }

    /// Translate `StoragePath` (relative, normalized) to an absolute path
    /// under `root`. Defense in depth: after `join`, the resulting path is
    /// `canonicalize`d (if it exists) and verified to live under `root`.
    fn resolve(&self, path: &StoragePath) -> StorageResult<PathBuf> {
        let mut joined = self.root.clone();
        if !path.is_root() {
            joined.push(path.as_str());
        }
        match joined.canonicalize() {
            Ok(c) => {
                if !c.starts_with(&self.root) {
                    return Err(StorageError::InvalidPath(format!(
                        "path escapes root: {}",
                        path.as_str()
                    )));
                }
                Ok(c)
            }
            Err(_) => {
                // Path doesn't exist (yet). Verify the closest existing ancestor
                // is inside root.
                let mut anc = joined.clone();
                while !anc.exists() {
                    if !anc.pop() {
                        return Err(StorageError::InvalidPath(format!(
                            "no existing ancestor for {}",
                            path.as_str()
                        )));
                    }
                }
                let canonical_anc = anc.canonicalize().map_err(map_io)?;
                if !canonical_anc.starts_with(&self.root) {
                    return Err(StorageError::InvalidPath(format!(
                        "ancestor escapes root: {}",
                        path.as_str()
                    )));
                }
                Ok(joined)
            }
        }
    }

    async fn metadata_of(&self, real: &Path, path: &StoragePath) -> StorageResult<FileMetadata> {
        let md = fs::metadata(real).await.map_err(map_io)?;
        let kind = if md.is_dir() {
            FileKind::Directory
        } else {
            FileKind::File
        };
        let size = if matches!(kind, FileKind::File) {
            md.len()
        } else {
            0
        };
        let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let etag = match xattr_io::read_etag(real) {
            Some(e) => e,
            None => ETag::from_mtime_and_id(mtime, stable_inode(&md)),
        };
        let mimetype = if matches!(kind, FileKind::Directory) {
            Mimetype::octet_stream()
        } else if let Some(m) = xattr_io::read_mimetype(real) {
            m
        } else {
            recompute_mimetype(real, path.as_str()).await
        };
        Ok(FileMetadata {
            path: path.clone(),
            kind,
            size,
            mtime,
            etag,
            mimetype,
            permissions: Permissions::full(),
        })
    }
}

#[async_trait]
impl Storage for LocalStorage {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        self.metadata_of(&real, path).await
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        let real = self.resolve(path)?;
        Ok(fs::try_exists(&real).await.map_err(map_io)?)
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if !md.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        let mut rd = fs::read_dir(&real).await.map_err(map_io)?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(map_io)? {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = if path.is_root() {
                StoragePath::new(name.clone())?
            } else {
                path.join(&name)?
            };
            let real_child = entry.path();
            let meta = self.metadata_of(&real_child, &child_path).await?;
            out.push(DirEntry { name, metadata: meta });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(
        &self,
        path: &StoragePath,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let f = fs::File::open(&real).await.map_err(map_io)?;
        Ok(Box::pin(f))
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let mut f = fs::File::open(&real).await.map_err(map_io)?;
        f.seek(SeekFrom::Start(range.start)).await.map_err(map_io)?;
        let limited = f.take(range.end.saturating_sub(range.start));
        Ok(Box::pin(limited))
    }

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let parent = real.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", real.display()))
        })?;
        let parent_md = fs::metadata(parent).await.map_err(map_io)?;
        if !parent_md.is_dir() {
            return Err(StorageError::NotADirectory);
        }

        let temp_path = sibling_temp(&real)?;
        let guard = TempFileGuard::new(temp_path.clone());

        // Stream body into temp. Peek the first 4KiB for mimetype sniffing.
        let (mut head, body) = peek_head(body, 4096).await?;
        let file_handle = stream_to_temp(guard.path(), body).await?;
        drop(file_handle);

        // Compute ETag + mimetype, write xattrs to the temp file.
        let etag = ETag::new();
        xattr_io::write_etag(guard.path(), &etag)?;
        head.truncate(4096);
        let mimetype = mimetype::detect(path.as_str(), &head);
        xattr_io::write_mimetype(guard.path(), &mimetype)?;

        // Atomic rename + fsync parent.
        atomic_rename(guard.path(), &real).await?;
        guard.forget();

        let meta = self.metadata_of(&real, path).await?;
        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: meta.clone(),
        })
        .await;
        Ok(meta)
    }

    async fn mkdir(
        &self,
        path: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        fs::create_dir(&real).await.map_err(map_io)?;
        let meta = self.metadata_of(&real, path).await?;
        sink.emit(StorageEvent::DirCreated {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: meta.clone(),
        })
        .await;
        Ok(meta)
    }

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            // Reject if non-empty. Don't walk; let `read_dir().next_entry()`
            // be O(1) for the common empty case.
            let mut rd = fs::read_dir(&real).await.map_err(map_io)?;
            if rd.next_entry().await.map_err(map_io)?.is_some() {
                return Err(StorageError::NotEmpty);
            }
            fs::remove_dir(&real).await.map_err(map_io)?;
        } else {
            fs::remove_file(&real).await.map_err(map_io)?;
        }
        sink.emit(StorageEvent::Deleted {
            storage_id: self.id.clone(),
            path: path.clone(),
        })
        .await;
        Ok(())
    }

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        let real_from = self.resolve(from)?;
        let real_to = self.resolve(to)?;
        if !fs::try_exists(&real_from).await.map_err(map_io)? {
            return Err(StorageError::NotFound);
        }
        if fs::try_exists(&real_to).await.map_err(map_io)? {
            return Err(StorageError::AlreadyExists);
        }
        fs::rename(&real_from, &real_to).await.map_err(map_io)?;
        sink.emit(StorageEvent::Moved {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        let real_from = self.resolve(from)?;
        let real_to = self.resolve(to)?;
        if fs::try_exists(&real_to).await.map_err(map_io)? {
            return Err(StorageError::AlreadyExists);
        }
        let md = fs::metadata(&real_from).await.map_err(map_io)?;
        if md.is_dir() {
            // Recursive copy: walk + recreate. Fresh ETag per leaf.
            copy_dir_recursive(&real_from, &real_to).await?;
        } else {
            fs::copy(&real_from, &real_to).await.map_err(map_io)?;
            // Fresh ETag at the destination — explicitly rewrite the xattr
            // because the source's xattr is copied verbatim on some FSes.
            xattr_io::write_etag(&real_to, &ETag::new())?;
        }
        sink.emit(StorageEvent::Copied {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn begin_multipart(
        &self,
        _target: &StoragePath,
        _sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        // Implemented in batch E.
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn put_part(
        &self,
        _handle: &MultipartHandle,
        _part_number: u32,
        _body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn commit_multipart(
        &self,
        _handle: MultipartHandle,
        _parts: Vec<PartTag>,
        _sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn abort_multipart(&self, _handle: MultipartHandle) -> StorageResult<()> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }
}

/// Read up to `n` bytes from `body` into a buffer; return the peek plus a
/// new reader that yields the peek followed by the rest of `body`.
async fn peek_head(
    mut body: Pin<Box<dyn AsyncRead + Send>>,
    n: usize,
) -> StorageResult<(Vec<u8>, Pin<Box<dyn AsyncRead + Send>>)> {
    use tokio::io::AsyncReadExt;
    let mut head = vec![0u8; n];
    let read = body.read(&mut head).await.map_err(map_io)?;
    head.truncate(read);
    let cloned_head = head.clone();
    let prefix = std::io::Cursor::new(cloned_head);
    let combined = prefix.chain(body);
    Ok((head, Box::pin(combined)))
}

async fn copy_dir_recursive(src: &Path, dst: &Path) -> StorageResult<()> {
    fs::create_dir(dst).await.map_err(map_io)?;
    let mut rd = fs::read_dir(src).await.map_err(map_io)?;
    while let Some(entry) = rd.next_entry().await.map_err(map_io)? {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let m = entry.metadata().await.map_err(map_io)?;
        if m.is_dir() {
            Box::pin(copy_dir_recursive(&from, &to)).await?;
        } else {
            fs::copy(&from, &to).await.map_err(map_io)?;
            xattr_io::write_etag(&to, &ETag::new())?;
        }
    }
    Ok(())
}

/// Get a stable per-file identifier for ETag fallback. On Unix this is the
/// inode; on other platforms we hash the file path bytes.
#[cfg(unix)]
fn stable_inode(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.ino()
}

#[cfg(not(unix))]
fn stable_inode(_md: &std::fs::Metadata) -> u64 {
    // No inode concept; the ETag fallback will collide if multiple files
    // share an mtime. Acceptable for the Windows fallback path.
    0
}

async fn recompute_mimetype(real: &Path, path: &str) -> Mimetype {
    if let Some(m) = mimetype::from_extension(path) {
        return m;
    }
    let head = match fs::read(real).await {
        Ok(mut v) => {
            v.truncate(4096);
            v
        }
        Err(_) => Vec::new(),
    };
    mimetype::detect(path, &head)
}
```

### Step 6: Wire LocalStorage into the trait suite

Append at the bottom of `crates/crabcloud-storage/tests/trait_suite.rs`:

```rust
#[tokio::test]
async fn local_backend_passes_trait_suite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    // Leak the TempDir so it survives until process exit. Each factory call
    // makes a fresh subdir under the leaked root.
    std::mem::forget(dir);
    let counter = std::sync::atomic::AtomicU32::new(0);

    run_storage_suite("local", || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sub = path.join(format!("storage-{n}"));
        std::fs::create_dir_all(&sub).unwrap();
        crabcloud_storage::local::LocalStorage::new(sub).unwrap()
    })
    .await;
}
```

**Important caveat:** the trait suite runs `multipart_happy_path`, `multipart_abort_drops_target`, `multipart_gap_rejected`, `multipart_duplicate_rejected` — but batch D's LocalStorage returns `StorageError::Other("multipart not yet implemented")`. To avoid trapping the batch D PR on this, gate the multipart assertions inside `run_storage_suite` behind a backend capability flag.

Modify the suite signature to accept a `caps: SuiteCaps`:

```rust
pub struct SuiteCaps {
    pub multipart: bool,
}

impl Default for SuiteCaps {
    fn default() -> Self {
        Self { multipart: true }
    }
}

pub async fn run_storage_suite<S: Storage + 'static>(
    name: &str,
    caps: SuiteCaps,
    factory: impl Fn() -> S + Send + Sync,
) {
    eprintln!("--- storage suite: {name} (multipart={}) ---", caps.multipart);

    path_invariants();
    write_then_read(&factory).await;
    write_overwrite_changes_etag(&factory).await;
    stat_after_write(&factory).await;
    read_range_returns_slice(&factory).await;
    mkdir_then_list_includes_dir(&factory).await;
    write_to_dir_lists_correctly(&factory).await;
    delete_file_then_stat_404(&factory).await;
    delete_empty_dir_ok_nonempty_errs(&factory).await;
    rename_moves(&factory).await;
    copy_preserves_contents_changes_etag(&factory).await;
    if caps.multipart {
        multipart_happy_path(&factory).await;
        multipart_abort_drops_target(&factory).await;
        multipart_gap_rejected(&factory).await;
        multipart_duplicate_rejected(&factory).await;
    }
    event_sink_emits_one_per_mutation(&factory).await;
}
```

And update the existing memory runner (already on master from batch C) — wait, memory IS on master at this point. We need to amend it. Modify the memory runner call:

```rust
#[tokio::test]
async fn memory_backend_passes_trait_suite() {
    run_storage_suite(
        "memory",
        SuiteCaps::default(),
        || crabcloud_storage::memory::MemoryStorage::new("test"),
    )
    .await;
}
```

And the new local runner:

```rust
#[tokio::test]
async fn local_backend_passes_trait_suite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    let counter = std::sync::atomic::AtomicU32::new(0);

    run_storage_suite(
        "local",
        SuiteCaps { multipart: false }, // multipart lands in batch E
        || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let sub = path.join(format!("storage-{n}"));
            std::fs::create_dir_all(&sub).unwrap();
            crabcloud_storage::local::LocalStorage::new(sub).unwrap()
        },
    )
    .await;
}
```

### Step 7: Run + commit + push + open Batch D PR

```
cargo test -p crabcloud-storage
cargo xtask check-all
```

Expected: memory suite still passes; local suite passes (11 assertions, multipart skipped); mimetype unit tests pass; all type-level tests pass.

```
git add crates/crabcloud-storage
git commit -m "feat(storage): LocalStorage core — atomic write + xattr ETag + mimetype

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-d
gh pr create --base master --head storage-batch-d \
  --title "storage: batch D — LocalStorage core (no multipart)" \
  --body "Sub-project 4a, batch D: LocalStorage covers stat/exists/list/read/read_range/put_file/mkdir/delete/rename/copy. Atomic-rename writes; xattr-persisted ETag with mtime+inode fallback; ~100-entry mimetype table + infer-based magic-byte sniffing. Trait suite passes (multipart gated behind a capability flag — batch E plugs it in)."
```

**STOP.**

---

## Task 5: LocalStorage multipart + local-specific tests (Batch E)

**Files:**
- Modify: `crates/crabcloud-storage/src/local/mod.rs` (replace the four `Err("multipart not yet implemented")` stubs)
- Create: `crates/crabcloud-storage/tests/local_specific.rs`
- Modify: `crates/crabcloud-storage/tests/trait_suite.rs` (flip Local's `SuiteCaps { multipart: true }`)

### Step 1: Branch

```
git checkout -b storage-batch-e origin/master
```

### Step 2: Implement multipart in `src/local/mod.rs`

Replace the four `Err(StorageError::Other("multipart not yet implemented".into()))` returns with real implementations.

Add to the top of `local/mod.rs` (after the existing imports):

```rust
use sha2::{Digest, Sha256};
```

Add `sha2` to `[dependencies]` in `crates/crabcloud-storage/Cargo.toml`:

```toml
sha2.workspace = true
```

(`sha2` is already a workspace dep — `sha2 = "0.11"`.)

Replace the four multipart methods with:

```rust
    async fn begin_multipart(
        &self,
        target: &StoragePath,
        _sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        let real_target = self.resolve(target)?;
        let parent = real_target.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", target.as_str()))
        })?;
        if !fs::try_exists(parent).await.map_err(map_io)? {
            return Err(StorageError::NotFound);
        }
        let mut id_bytes = [0u8; 16];
        rand::rng().fill(&mut id_bytes);
        let upload_id = format!("local-mp-{}", hex::encode(id_bytes));
        let temp_dir = parent.join(format!(".upload-{}", upload_id));
        fs::create_dir(&temp_dir).await.map_err(map_io)?;
        Ok(MultipartHandle {
            upload_id,
            target: target.clone(),
        })
    }

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        let real_target = self.resolve(&handle.target)?;
        let parent = real_target.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", handle.target.as_str()))
        })?;
        let temp_dir = parent.join(format!(".upload-{}", handle.upload_id));
        if !fs::try_exists(&temp_dir).await.map_err(map_io)? {
            return Err(StorageError::Multipart(format!(
                "unknown upload id: {}",
                handle.upload_id
            )));
        }
        let part_path = temp_dir.join(format!("part-{:08}", part_number));
        // Stream body to disk while hashing.
        use tokio::io::{AsyncWriteExt, BufWriter};
        let f = fs::File::create(&part_path).await.map_err(map_io)?;
        let mut writer = BufWriter::new(f);
        let mut body = body;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = body.read(&mut buf).await.map_err(map_io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            writer.write_all(&buf[..n]).await.map_err(map_io)?;
        }
        writer.flush().await.map_err(map_io)?;
        let etag = hex::encode(hasher.finalize());
        Ok(PartTag { part_number, etag })
    }

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        if parts.is_empty() {
            return Err(StorageError::Multipart("no parts".into()));
        }
        // Validate contiguous, starts at 1.
        let mut sorted: Vec<&PartTag> = parts.iter().collect();
        sorted.sort_by_key(|p| p.part_number);
        for (i, p) in sorted.iter().enumerate() {
            if (p.part_number as usize) != i + 1 {
                return Err(StorageError::Multipart(format!(
                    "expected contiguous parts starting at 1; got {} at index {i}",
                    p.part_number
                )));
            }
        }
        // Reject duplicates.
        for w in sorted.windows(2) {
            if w[0].part_number == w[1].part_number {
                return Err(StorageError::Multipart(format!(
                    "duplicate part {}",
                    w[0].part_number
                )));
            }
        }

        let real_target = self.resolve(&handle.target)?;
        let parent = real_target.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", handle.target.as_str()))
        })?;
        let temp_dir = parent.join(format!(".upload-{}", handle.upload_id));
        if !fs::try_exists(&temp_dir).await.map_err(map_io)? {
            return Err(StorageError::Multipart(format!(
                "unknown upload id: {}",
                handle.upload_id
            )));
        }

        // Verify each part's sha256 matches its supplied tag.
        for tag in &sorted {
            let part_path = temp_dir.join(format!("part-{:08}", tag.part_number));
            let bytes = fs::read(&part_path).await.map_err(map_io)?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let actual = hex::encode(hasher.finalize());
            if actual != tag.etag {
                return Err(StorageError::Multipart(format!(
                    "part {} integrity check failed",
                    tag.part_number
                )));
            }
        }

        // Concatenate into a sibling temp file under the target's directory.
        let final_temp = sibling_temp(&real_target)?;
        let guard = TempFileGuard::new(final_temp.clone());
        use tokio::io::{AsyncWriteExt, BufWriter};
        let f = fs::File::create(guard.path()).await.map_err(map_io)?;
        let mut writer = BufWriter::new(f);
        let mut head: Vec<u8> = Vec::new();
        for tag in &sorted {
            let part_path = temp_dir.join(format!("part-{:08}", tag.part_number));
            let bytes = fs::read(&part_path).await.map_err(map_io)?;
            if head.len() < 4096 {
                let want = 4096 - head.len();
                head.extend_from_slice(&bytes[..bytes.len().min(want)]);
            }
            writer.write_all(&bytes).await.map_err(map_io)?;
        }
        writer.flush().await.map_err(map_io)?;
        let handle_for_sync = writer.into_inner();
        handle_for_sync.sync_all().await.map_err(map_io)?;

        let etag = ETag::new();
        xattr_io::write_etag(guard.path(), &etag)?;
        let mimetype = mimetype::detect(handle.target.as_str(), &head);
        xattr_io::write_mimetype(guard.path(), &mimetype)?;

        atomic_rename(guard.path(), &real_target).await?;
        guard.forget();

        // Tear down the upload directory.
        let _ = fs::remove_dir_all(&temp_dir).await;

        let meta = self.metadata_of(&real_target, &handle.target).await?;
        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: handle.target.clone(),
            metadata: meta.clone(),
        })
        .await;
        Ok(meta)
    }

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()> {
        let real_target = self.resolve(&handle.target)?;
        let parent = real_target.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", handle.target.as_str()))
        })?;
        let temp_dir = parent.join(format!(".upload-{}", handle.upload_id));
        let _ = fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }
```

### Step 3: Flip Local's multipart capability flag

In `crates/crabcloud-storage/tests/trait_suite.rs`, change the local runner from `SuiteCaps { multipart: false }` to `SuiteCaps { multipart: true }`:

```rust
#[tokio::test]
async fn local_backend_passes_trait_suite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    let counter = std::sync::atomic::AtomicU32::new(0);

    run_storage_suite(
        "local",
        SuiteCaps::default(),
        || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let sub = path.join(format!("storage-{n}"));
            std::fs::create_dir_all(&sub).unwrap();
            crabcloud_storage::local::LocalStorage::new(sub).unwrap()
        },
    )
    .await;
}
```

### Step 4: Create `tests/local_specific.rs`

```rust
//! Local-FS-specific tests: atomic durability, xattr persistence, path escape.

#![cfg(unix)]

mod support;

use crabcloud_storage::local::LocalStorage;
use crabcloud_storage::{NoopEventSink, Storage, StoragePath};
use tempfile::tempdir;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn etag_persists_across_reload() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("p.txt").unwrap();
    storage
        .put_file(&path, body(b"hello".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let first = storage.stat(&path).await.unwrap();

    let reloaded = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let second = reloaded.stat(&path).await.unwrap();
    assert_eq!(first.etag, second.etag);
}

#[tokio::test]
async fn mimetype_persists_across_reload() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("hello.txt").unwrap();
    storage
        .put_file(&path, body(b"hi".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let reloaded = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        reloaded.stat(&path).await.unwrap().mimetype.as_str(),
        "text/plain"
    );
}

#[tokio::test]
async fn xattr_stripped_falls_back_to_mtime_inode_etag() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("fallback.txt").unwrap();
    storage
        .put_file(&path, body(b"hi".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let real_path = dir.path().join("fallback.txt");
    let _ = xattr::remove(&real_path, "user.crabcloud.etag");
    // After xattr strip, ETag should be deterministic-from-mtime/inode and
    // non-empty. Two stats should agree.
    let a = storage.stat(&path).await.unwrap();
    let b = storage.stat(&path).await.unwrap();
    assert_eq!(a.etag, b.etag);
    assert_eq!(a.etag.as_str().len(), 40);
}

#[tokio::test]
async fn atomic_write_temp_cleaned_on_drop() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let path = StoragePath::new("clean.txt").unwrap();
    // Successful write — should leave no .tmp-crabcloud-* siblings.
    storage
        .put_file(&path, body(b"x".to_vec()), &NoopEventSink)
        .await
        .unwrap();
    let mut leftover = false;
    let mut rd = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = rd.next_entry().await.unwrap() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-crabcloud-")
        {
            leftover = true;
        }
    }
    assert!(!leftover, "found leftover .tmp-crabcloud-* file");
}

#[tokio::test]
async fn path_escape_via_canonicalize_rejected() {
    let outer = tempdir().unwrap();
    let inner = outer.path().join("inner");
    tokio::fs::create_dir(&inner).await.unwrap();
    let storage = LocalStorage::new(inner.clone()).unwrap();

    // Create a real escape target outside `inner` and a symlink inside that
    // points to it. resolve() canonicalize + starts_with(root) check rejects.
    let target_outside = outer.path().join("OUTSIDE");
    tokio::fs::write(&target_outside, b"secret").await.unwrap();
    let link_in = inner.join("escape");
    std::os::unix::fs::symlink(&target_outside, &link_in).unwrap();

    let res = storage
        .stat(&StoragePath::new("escape").unwrap())
        .await;
    assert!(
        matches!(res, Err(crabcloud_storage::StorageError::InvalidPath(_))),
        "expected InvalidPath, got {:?}",
        res
    );
}

#[tokio::test]
async fn multipart_abort_drops_upload_dir() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let target = StoragePath::new("aborted.bin").unwrap();
    let handle = storage.begin_multipart(&target, &NoopEventSink).await.unwrap();
    storage
        .put_part(&handle, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let upload_id = handle.upload_id.clone();
    storage.abort_multipart(handle).await.unwrap();

    // Upload tempdir should be gone.
    let mut found = false;
    let mut rd = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = rd.next_entry().await.unwrap() {
        if entry
            .file_name()
            .to_string_lossy()
            .contains(&upload_id)
        {
            found = true;
        }
    }
    assert!(!found, "upload tempdir not cleaned up");
}

#[tokio::test]
async fn multipart_corrupted_part_rejected_at_commit() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path().to_path_buf()).unwrap();
    let target = StoragePath::new("corrupt.bin").unwrap();
    let handle = storage.begin_multipart(&target, &NoopEventSink).await.unwrap();
    let t1 = storage
        .put_part(&handle, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    // Tamper with the part file directly.
    let parent = dir.path().to_path_buf();
    let temp_dir = parent.join(format!(".upload-{}", handle.upload_id));
    let part_file = temp_dir.join("part-00000001");
    tokio::fs::write(&part_file, b"BBB").await.unwrap();

    let err = storage
        .commit_multipart(handle, vec![t1], &NoopEventSink)
        .await
        .unwrap_err();
    assert!(matches!(err, crabcloud_storage::StorageError::Multipart(_)));
}
```

### Step 5: Run + commit + push + open Batch E PR

```
cargo test -p crabcloud-storage
cargo xtask check-all
```

Expected: Local trait suite now passes the multipart assertions too; ~7 local-specific tests pass on Unix; whole crate is green.

```
git add crates/crabcloud-storage
git commit -m "feat(storage): LocalStorage multipart + local-specific tests

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-e
gh pr create --base master --head storage-batch-e \
  --title "storage: batch E — LocalStorage multipart + local-specific tests" \
  --body "Sub-project 4a, batch E: LocalStorage multipart (begin/put_part/commit/abort) via per-upload tempdir + sha256 part integrity check + final atomic-rename. Local-specific tests cover xattr ETag/mimetype persistence across reload, mtime+inode fallback when xattr is stripped, atomic-write temp cleanup, symlink-based path escape rejection, multipart abort + corrupted-part rejection. Trait suite green for both backends including multipart."
```

**STOP.**

---

## Task 6: Acceptance docs (Batch F)

**Files:**
- Create: `docs/superpowers/plans/2026-05-12-storage-trait-and-backends-implementation.changelog.md`
- Modify: `README.md`
- Create: `docs/superpowers/specs/2026-05-12-storage-trait-and-backends-design.followup-4b.md`

### Step 1: Branch

```
git checkout -b storage-batch-f origin/master
```

### Step 2: Write the changelog

Create `docs/superpowers/plans/2026-05-12-storage-trait-and-backends-implementation.changelog.md`:

```markdown
# Sub-project 4a — Changelog

Completed: 2026-05-12

## What works

- New `crabcloud-storage` crate; pure primitives (no DB, HTTP, or Dioxus deps).
- `Storage` async trait covering `stat`/`exists`/`list`/`read`/`read_range`/`put_file`/`mkdir`/`delete`/`rename`/`copy` + `begin_multipart`/`put_part`/`commit_multipart`/`abort_multipart`. Object-safe.
- `MemoryStorage` backend: `Arc<RwLock<MemTree>>` around a `BTreeMap<StoragePath, MemEntry>`; multipart via per-handle `Mutex<BTreeMap<u32, Bytes>>`; implicit-mkdir for parent directories.
- `LocalStorage` backend: atomic-rename writes (tempfile + fsync + rename + parent-fsync); xattr-persisted ETag with mtime+inode fallback; ~100-entry mimetype table (extension lookup) + `infer`-based magic-byte sniff fallback; range reads; recursive copy; multipart via per-upload tempdir + sha256 part integrity check.
- `StoragePath` newtype with normalization (no `.`, no `..`, no leading `/`, no NUL, no backslash, ≤4096 chars).
- `EventSink` trait + `NoopEventSink`; mutating ops emit `StorageEvent::{Written, DirCreated, Deleted, Moved, Copied}` with `storage_id`, `path`, and (where applicable) `metadata`.
- Parametrized trait test suite — 15 assertions covering happy paths + edge cases + multipart + event emission. Both backends pass.
- Local-specific tests for xattr persistence, mtime+inode fallback, atomic temp cleanup, symlink-based path escape rejection, multipart abort + corrupted-part rejection.
- Memory-specific tests for 100-way concurrent writes (distinct + same path).

## What's deferred

- `oc_filecache` schema + scanner — **sub-project 4b**.
- S3 backend — **sub-project 4b**.
- Real (channel-backed) `EventSink` consumer — **sub-project 4b**.
- Mount composition (`View` layer) — **sub-project 4c**.
- Chunked-upload protocol translation (Nextcloud's `/dav/uploads/...` flow) — **sub-project 4c**.
- WebDAV — sub-project 5.
- Trash, versions, WebDAV LOCK/UNLOCK — separate later sub-projects.
- Server-side encryption seam — later sub-project.
- Sharing-aware permissions composition — later sub-project.

## Known limitations

- ETag xattr is **Unix-only**. On Windows, `LocalStorage::stat` falls back to the mtime+inode-derived ETag — deterministic and changes on mutation, but no random per-write entropy. (Windows inode is `0` because there's no native concept; on Windows the fallback aliases all files with identical mtime.)
- Mimetype table is ~100 entries (most-used). Nextcloud's upstream `mimetypemapping.dist.json` has ~400; we'll expand additively as test coverage drives.
- `MemoryStorage` implicitly creates parent directories on `put_file`. `LocalStorage` requires explicit `mkdir` for parents. The asymmetry is documented in the trait suite — each backend is tested against its own contract.
- No `LOCK`/`UNLOCK`. WebDAV-compliant locking is a separate sub-project.
- `LocalStorage::resolve` defends against symlink escape via post-canonicalize `starts_with(root)`. There's still a TOCTOU window between `canonicalize` and the actual open syscall — `openat`-style hardening is deferred.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean | OK (CI) |
| 2 | `Storage` trait object-safe (`Arc<dyn Storage>` compiles) | OK (`lib.rs::tests::storage_trait_is_object_safe`) |
| 3 | Both backends pass full parametrized trait suite | OK (`tests/trait_suite.rs::{memory,local}_backend_passes_trait_suite`) |
| 4 | Atomic write: no leftover temp files; rename is atomic | OK (`tests/local_specific.rs::atomic_write_temp_cleaned_on_drop`) |
| 5 | ETag is 40-char lowercase hex matching Nextcloud format | OK (`meta.rs::tests::etag_new_is_40_hex_chars`) |
| 6 | Mimetype detection for known extensions, magic-byte sniff, octet-stream fallback | OK (`local/mimetype.rs::tests::*`) |
| 7 | Range reads return exactly the requested slice | OK (trait suite) |
| 8 | Multipart happy + abort + gap + duplicate | OK (trait suite) |
| 9 | EventSink emissions match every mutation 1:1 | OK (trait suite) |
| 10 | Path escape rejected (constructor + resolve defense) | OK (path.rs::tests + tests/local_specific.rs::path_escape_via_canonicalize_rejected) |
| 11 | `-D warnings` clean | OK (CI) |
| 12 | `git grep -i rustcloud` empty | OK |
| 13 | New crate documented in README's workspace-layout bullet | OK (batch F) |
```

### Step 3: Modify README.md

Read `README.md`. Find the workspace-layout block (the list of `crates/...` crates). Insert `crabcloud-storage` in alphabetical order:

```
crates/
  crabcloud-cache         in-memory + (future) Redis cache
  crabcloud-config        TOML config loader, secret handling
  crabcloud-core          AppState + AppStateBuilder, error model
  crabcloud-db            multi-dialect SQLite/MySQL/Postgres pool + migrations
  crabcloud-http          axum routes, middleware, OCS surface, session, login
  crabcloud-i18n          locale catalog, message keys
  crabcloud-ocs           Open Collaboration Services envelope (JSON/XML)
  crabcloud-server        binary entrypoint (axum + dioxus mounting)
  crabcloud-storage       Storage trait + LocalStorage + MemoryStorage backends
  crabcloud-ui            Dioxus Fullstack browser UI (SSR + hydration + server fns)
  crabcloud-users         user/group/auth domain, app passwords, admin OCS
```

(Adjust the descriptions to match what's actually in the README; the goal is to insert the `crabcloud-storage` line in the right place, not rewrite the others.)

### Step 4: Write the 4b follow-up notes

Create `docs/superpowers/specs/2026-05-12-storage-trait-and-backends-design.followup-4b.md`:

```markdown
# Sub-project 4b prep — File cache + S3 + event consumer

Notes captured during 4a implementation that should inform the 4b spec when we brainstorm it.

## Filecache schema sketch

Mirror upstream Nextcloud's `oc_filecache` shape:

| Column | Type | Notes |
|---|---|---|
| fileid | BIGINT PK | autoincrement |
| storage | INT | FK → `oc_storages.numeric_id` |
| path | TEXT | the same `StoragePath::as_str()` |
| path_hash | CHAR(32) | md5 of path; indexed for path lookups |
| parent | BIGINT | self-FK → fileid; nullable for root |
| name | TEXT | basename |
| mimetype | INT | FK → `oc_mimetypes.id` (interned) |
| mimepart | INT | FK → `oc_mimetypes.id` for "type/" half |
| size | BIGINT | bytes; -1 for incomplete |
| mtime | INT | unix seconds |
| storage_mtime | INT | mtime as observed on the backing storage |
| encrypted | INT | 0 in 4b; future encryption sub-project |
| etag | VARCHAR(40) | matches `ETag::as_str()` |
| permissions | INT | bitmap; `Permissions::bits()` |
| checksum | TEXT | nullable; future checksum sub-project |

Auxiliary tables: `oc_storages` (numeric_id PK, id VARCHAR(64)), `oc_mimetypes` (id PK, mimetype VARCHAR).

## Event consumer shape

Add `ChannelEventSink` to `crabcloud-storage` or a new `crabcloud-storage-events` crate:

```rust
pub struct ChannelEventSink {
    tx: tokio::sync::broadcast::Sender<StorageEvent>,
}

impl ChannelEventSink {
    pub fn new(capacity: usize) -> Self { ... }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StorageEvent> { ... }
}

#[async_trait::async_trait]
impl EventSink for ChannelEventSink {
    async fn emit(&self, event: StorageEvent) {
        let _ = self.tx.send(event);
    }
}
```

Sub-project 4b's filecache scanner subscribes to one receiver and updates `oc_filecache` rows on each event. Lag is OK — the cache is eventually consistent with storage state.

## S3 backend sketch

Use `aws-sdk-s3` (official, async, multipart-first). Map:

- `id()` → `format!("s3::{bucket}/{prefix}")`
- `put_file` short-circuits to `PutObjectRequest` for small bodies; multipart-or-die for large.
- `begin_multipart` → `CreateMultipartUpload`; `upload_id` ← S3's UploadId.
- `put_part` → `UploadPart`; `PartTag.etag` ← S3 part ETag.
- `commit_multipart` → `CompleteMultipartUpload(parts: parts.into_iter().map(|p| CompletedPart{...}))`.
- `abort_multipart` → `AbortMultipartUpload`.

Stat/list use `HeadObject` + `ListObjectsV2`. ETag from S3's ETag header (already hex-ish; we may need to normalize). Mimetype from S3 `Content-Type` (set on PUT from the same detect logic used by Local).

S3 doesn't support directories natively — we use the common `<prefix>/` empty-object convention, or fold directories into the filecache layer (skip the empty-object marker; filecache rows track directories).

## Scanner-driven drift recovery

In 4b add a `Scanner` that walks a storage from root and reconciles cache rows. Triggered by:

- Operator CLI (`crabcloud files:scan <storage>`).
- Startup-time check of last-scan timestamp (every N hours).
- 4b's broadcast channel having a stale `RecvError::Lagged` (recover by full-scanning the affected subtree).

## Open questions for 4b brainstorming

- Folder-size aggregation: write-through or scan-only? Nextcloud uses write-through with parent ETag bumping.
- ETag propagation: every mutation should bump every ancestor's ETag so desktop clients see "something changed" at the top. Write-through during sink consumption is the natural place.
- Cache-miss policy: on `stat` for a path not in cache, do we walk + populate (expensive) or 404 (consistency-flaky)? Recommend populate-with-locked-claim.
```

### Step 5: Run + commit + push + open Batch F PR

```
cargo xtask check-all
git add docs/superpowers README.md
git commit -m "docs(storage): sub-project 4a acceptance — changelog + README + 4b prep notes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"

git push -u origin storage-batch-f
gh pr create --base master --head storage-batch-f \
  --title "storage: batch F — sub-project 4a acceptance docs" \
  --body "Sub-project 4a final batch: changelog with full acceptance table, README workspace-layout bullet for crabcloud-storage, and prep notes for the 4b spec (filecache schema sketch, event consumer shape, S3 backend mapping, scanner drift recovery, open questions for 4b brainstorming)."
```

**STOP.**

---

## Final acceptance

After all 6 PRs land:

1. `git pull --ff-only origin master`.
2. `cargo xtask check-all` green.
3. CI green on master (all 5 checks).
4. Mark the 4a sub-project complete; update memory.
5. Brainstorm 4b (filecache + S3 + scanner) when ready.

## Open questions deferred

- See changelog "What's deferred".
- See the 4b prep doc for design decisions to make when brainstorming 4b.
- `phf_codegen` build script leaks strings (one-shot process); evaluate if a different generator can do it without leaks. Not critical.
