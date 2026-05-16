# Previews + Thumbnails Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship on-demand thumbnail generation for image (JPEG / PNG / GIF / WebP) and PDF source files on both the authenticated Files surface (`GET /api/files/preview/{fileid}?size=N`) and the anonymous public-link viewer (`GET /s/{token}/preview/{*path}?size=N`). Files UI replaces generic icons with inline `<img>` thumbnails.

**Architecture:** A new `crabcloud-preview` crate owns the per-mime provider trait, image + PDF backends (image crate + hayro), per-key dedup lock, and on-disk cache under `<data_dir>/appdata/preview/<storage_id>/<fileid>/<size>-<etag>.jpg`. Two HTTP handlers (authed + public) share `PreviewCache::get_or_render`. Files UI integrates via conditional `<img>` rendering on previewable mime types with `onerror` fallback to the generic icon.

**Tech Stack:** Rust 1.95, `image = "0.25"`, `hayro = "0.7"` (pure-Rust PDF renderer), `dashmap`, `tokio` (`sync`, `fs`, `task::spawn_blocking`), `tokio-util` (`ReaderStream`), axum 0.8, Dioxus 0.7.

**Spec:** `docs/superpowers/specs/2026-05-16-previews-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp10/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-preview-crate`, `b-authed-handler`, `c-public-handler`, `d-ui-integration`.
- **Commit cadence:** Commit at every "Commit" step.
- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- **Merge:** After CI green: `gh pr merge --squash --delete-branch`.
- **Test fixtures:** Use the shared `crates/crabcloud-http/tests/support/mod.rs` (introduced in the test-fixture cleanup PR #152). New preview-specific helpers join this file rather than getting inlined in each test target.
- **Established workaround:** Tests building `AppState` set `cfg.filecache.enabled = false`. See `crates/crabcloud-http/tests/support/mod.rs::make_state`.
- **Pre-existing patterns to mirror:**
  - **Presentation crate shape:** `crates/crabcloud-zip` (SP9) — small focused modules, `lib.rs` is a thin facade.
  - **Authed handler shape:** `crates/crabcloud-http/src/routes/files_zip.rs` (SP9 Batch B). Uses `AuthenticatedUser` for proper 401.
  - **Public-link handler shape:** `crates/crabcloud-http/src/routes/public_link/zip.rs` (SP9 Batch C). Sibling-module organization; `mod.rs` holds shared helpers.
  - **Per-key dedup:** none yet in the codebase; this batch establishes the pattern.

---

## File-by-file map

### New crate: `crabcloud-preview`

```
crates/crabcloud-preview/
├── Cargo.toml
├── src/
│   ├── lib.rs                      — re-exports + crate doc
│   ├── error.rs                    — PreviewError
│   ├── ladder.rs                   — LADDER = [64, 256, 1024]; round_up_to_ladder
│   ├── provider.rs                 — PreviewProvider trait + provider_for_mime
│   ├── providers/
│   │   ├── mod.rs                  — module facade
│   │   ├── image.rs                — ImageProvider impl
│   │   └── pdf.rs                  — PdfProvider impl (hayro)
│   └── cache.rs                    — PreviewCache::get_or_render + dedup lock
└── tests/                          — unit tests inline via #[cfg(test)] mod tests
```

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-preview` member, `image = "0.25"`, `hayro = "0.7"` to `[workspace.dependencies]`.
- `crates/crabcloud-config/src/types.rs` — `preview_root: PathBuf` + `preview_max_pixels: u32` fields.
- `crates/crabcloud-config/src/test_support.rs` — fills the new fields.
- `crates/crabcloud-http/src/routes/files_preview.rs` (new) — authed handler.
- `crates/crabcloud-http/src/routes/public_link/preview.rs` (new) — public handler.
- `crates/crabcloud-http/src/routes/public_link/mod.rs` — registers `mod preview;` + new routes.
- `crates/crabcloud-http/src/routes/mod.rs` — `pub mod files_preview;`.
- `crates/crabcloud-http/src/router.rs` — wires `files_preview::router()` into the authed surface.
- `crates/crabcloud-http/Cargo.toml` — adds `crabcloud-preview` dep.
- `crates/crabcloud-http/tests/support/mod.rs` — `seed_jpeg`, `seed_png`, `seed_pdf` helpers.
- `crates/crabcloud-http/tests/files_preview_e2e.rs` (new) — authed e2e tests.
- `crates/crabcloud-http/tests/public_link_e2e.rs` — adds public-preview e2e tests.
- `crates/crabcloud-app/src/components/file_row.rs` (or wherever `FileRow` lives) — inline `<img>` for previewable mimes.
- `crates/crabcloud-app/src/pages/public_link.rs` — public listing rows render thumbnails.
- `crates/crabcloud-app/tests/server_fns_files.rs` — snapshot includes `<img>` tag.

---

# Batch A — `crabcloud-preview` foundation crate

**Branch:** `sp10/a-preview-crate`

**Goal:** Stand up the new crate with errors, ladder, provider trait, image + PDF backends, and the on-disk preview cache. No HTTP wiring.

### Task A1: Create the crate skeleton

**Files:**
- Create: `crates/crabcloud-preview/Cargo.toml`
- Create: `crates/crabcloud-preview/src/lib.rs`
- Create: `crates/crabcloud-preview/src/error.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Add the crate to the workspace and register `image` + `hayro`**

Edit workspace `Cargo.toml`:
1. Add `"crates/crabcloud-preview",` to `members`.
2. Add to `[workspace.dependencies]`:
   ```toml
   image = { version = "0.25", default-features = false, features = ["jpeg", "png", "gif", "webp"] }
   hayro = "0.7"
   ```
3. Add to the internal workspace deps section:
   ```toml
   crabcloud-preview = { path = "crates/crabcloud-preview" }
   ```

- [ ] **Step 2: Write `crates/crabcloud-preview/Cargo.toml`**

```toml
[package]
name = "crabcloud-preview"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait = { workspace = true }
crabcloud-fs = { workspace = true }
crabcloud-storage = { workspace = true }
dashmap = { workspace = true }
hayro = { workspace = true }
image = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync", "fs", "rt", "io-util"] }
tokio-util = { workspace = true, features = ["io"] }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
tempfile = { workspace = true }
crabcloud-filecache = { workspace = true }
crabcloud-users = { workspace = true }
crabcloud-config = { workspace = true }
crabcloud-db = { workspace = true }
```

- [ ] **Step 3: Write `src/lib.rs`**

```rust
//! On-demand thumbnail generation for image and PDF source files.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-previews-design.md`.
//!
//! Public entry point is [`PreviewCache::get_or_render`]. Providers
//! ([`ImageProvider`], [`PdfProvider`]) dispatch by source mime through
//! [`provider_for_mime`]. Output is always JPEG.

mod cache;
mod error;
mod ladder;
mod provider;
mod providers;

pub use cache::PreviewCache;
pub use error::PreviewError;
pub use ladder::{round_up_to_ladder, LADDER};
pub use provider::{provider_for_mime, PreviewProvider, ProviderResult};
pub use providers::{ImageProvider, PdfProvider};
```

- [ ] **Step 4: Write `src/error.rs`**

```rust
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("mime not supported: {0}")]
    Unsupported(String),
    #[error("requested size {0} is above the maximum supported ladder rung")]
    SizeOutOfRange(u32),
    #[error("source image too large ({width}x{height}, max {max} pixels)")]
    SourceTooLarge { width: u32, height: u32, max: u32 },
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("PDF render failed: {0}")]
    PdfRender(String),
    #[error("source path not found: {0:?}")]
    SourceNotFound(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Fs(#[from] crabcloud_fs::FsError),
}
```

- [ ] **Step 5: Verify crate builds**

```bash
cargo build -p crabcloud-preview
```

Expected: clean, possibly warnings about unused module re-exports.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/crabcloud-preview/
git commit -m "preview: crate skeleton with error type and workspace integration"
```

### Task A2: Ladder

**Files:**
- Create: `crates/crabcloud-preview/src/ladder.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! Fixed thumbnail size ladder. Three rungs: 64, 256, 1024 px.

use crate::error::PreviewError;

pub const LADDER: &[u32] = &[64, 256, 1024];

/// Round `requested` UP to the next ladder rung. Returns
/// `Err(SizeOutOfRange)` if `requested` is above the top of the ladder.
/// `requested = 0` is treated as the smallest rung (defensive).
pub fn round_up_to_ladder(requested: u32) -> Result<u32, PreviewError> {
    if requested == 0 {
        return Ok(LADDER[0]);
    }
    if requested > *LADDER.last().expect("ladder non-empty") {
        return Err(PreviewError::SizeOutOfRange(requested));
    }
    for &rung in LADDER {
        if requested <= rung {
            return Ok(rung);
        }
    }
    unreachable!("requested <= last ladder rung was checked above")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up_within_range() {
        assert_eq!(round_up_to_ladder(1).unwrap(), 64);
        assert_eq!(round_up_to_ladder(16).unwrap(), 64);
        assert_eq!(round_up_to_ladder(64).unwrap(), 64);
        assert_eq!(round_up_to_ladder(65).unwrap(), 256);
        assert_eq!(round_up_to_ladder(256).unwrap(), 256);
        assert_eq!(round_up_to_ladder(257).unwrap(), 1024);
        assert_eq!(round_up_to_ladder(1024).unwrap(), 1024);
    }

    #[test]
    fn zero_returns_smallest_rung() {
        assert_eq!(round_up_to_ladder(0).unwrap(), 64);
    }

    #[test]
    fn rejects_above_top_rung() {
        match round_up_to_ladder(1025) {
            Err(PreviewError::SizeOutOfRange(1025)) => {}
            other => panic!("expected SizeOutOfRange(1025), got {other:?}"),
        }
        match round_up_to_ladder(u32::MAX) {
            Err(PreviewError::SizeOutOfRange(_)) => {}
            other => panic!("expected SizeOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn ladder_is_strictly_increasing() {
        for w in LADDER.windows(2) {
            assert!(w[0] < w[1], "ladder must be strictly increasing");
        }
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-preview ladder::tests
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-preview/src/ladder.rs
git commit -m "preview: ladder rounding (64/256/1024 pixel rungs)"
```

### Task A3: Provider trait + dispatch

**Files:**
- Create: `crates/crabcloud-preview/src/provider.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! Provider trait and per-mime dispatch.

use crate::error::PreviewError;
use async_trait::async_trait;

pub type ProviderResult<T> = Result<T, PreviewError>;

/// Renders a thumbnail of a source file's bytes at a target size.
/// Implementations should run any CPU-bound work inside
/// `tokio::task::spawn_blocking` so they don't block the async runtime.
#[async_trait]
pub trait PreviewProvider: Send + Sync {
    /// `source_bytes` is the raw source file (image, PDF, etc).
    /// `size_px` is the target longest-edge in pixels (already snapped to
    /// the ladder by the cache layer; provider doesn't re-snap).
    /// `max_pixels` is an upstream safety budget on the decoded source size.
    /// Output is JPEG-encoded bytes ready to write to disk and serve.
    async fn render(
        &self,
        source_bytes: Vec<u8>,
        size_px: u32,
        max_pixels: u32,
    ) -> ProviderResult<Vec<u8>>;
}

/// Dispatch a source mime to its provider. Returns `None` for any mime we
/// don't currently know how to thumbnail; the handler maps `None` to HTTP
/// 415. Mime matching is case-insensitive prefix.
pub fn provider_for_mime(mime: &str) -> Option<&'static dyn PreviewProvider> {
    let lc = mime.to_ascii_lowercase();
    if lc.starts_with("image/jpeg")
        || lc.starts_with("image/png")
        || lc.starts_with("image/gif")
        || lc.starts_with("image/webp")
    {
        Some(&crate::providers::ImageProvider)
    } else if lc.starts_with("application/pdf") {
        Some(&crate::providers::PdfProvider)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_mimes_dispatch() {
        for mime in &[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "IMAGE/JPEG",
            "image/jpeg; charset=binary",
        ] {
            assert!(provider_for_mime(mime).is_some(), "{mime} should dispatch");
        }
    }

    #[test]
    fn pdf_mime_dispatches() {
        assert!(provider_for_mime("application/pdf").is_some());
    }

    #[test]
    fn unsupported_mimes_return_none() {
        for mime in &[
            "video/mp4",
            "application/zip",
            "application/octet-stream",
            "text/plain",
            "image/svg+xml",
            "image/heic",
            "",
        ] {
            assert!(provider_for_mime(mime).is_none(), "{mime} should not dispatch");
        }
    }
}
```

- [ ] **Step 2: Run tests**

The tests reference `crate::providers::ImageProvider` and `crate::providers::PdfProvider`, which don't exist yet. Stub them so the build passes:

Create `crates/crabcloud-preview/src/providers/mod.rs`:

```rust
//! Per-mime provider implementations.

mod image;
mod pdf;

pub use image::ImageProvider;
pub use pdf::PdfProvider;
```

Create `crates/crabcloud-preview/src/providers/image.rs`:

```rust
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;

pub struct ImageProvider;

#[async_trait]
impl PreviewProvider for ImageProvider {
    async fn render(&self, _source: Vec<u8>, _size_px: u32, _max_pixels: u32) -> ProviderResult<Vec<u8>> {
        unimplemented!("filled in Task A4")
    }
}
```

Create `crates/crabcloud-preview/src/providers/pdf.rs`:

```rust
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;

pub struct PdfProvider;

#[async_trait]
impl PreviewProvider for PdfProvider {
    async fn render(&self, _source: Vec<u8>, _size_px: u32, _max_pixels: u32) -> ProviderResult<Vec<u8>> {
        unimplemented!("filled in Task A5")
    }
}
```

Now run:

```bash
cargo test -p crabcloud-preview provider::tests
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-preview/src/provider.rs crates/crabcloud-preview/src/providers/
git commit -m "preview: PreviewProvider trait + provider_for_mime dispatch + stubs"
```

### Task A4: `ImageProvider`

**Files:**
- Modify: `crates/crabcloud-preview/src/providers/image.rs`

- [ ] **Step 1: Write impl + tests**

Replace the file content with:

```rust
//! Image source provider. Decodes JPEG/PNG/GIF/WebP, resizes to fit the
//! target longest-edge, encodes the result as JPEG (quality 80).

use crate::error::PreviewError;
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::ImageReader;
use std::io::Cursor;

pub struct ImageProvider;

const JPEG_QUALITY: u8 = 80;

#[async_trait]
impl PreviewProvider for ImageProvider {
    async fn render(
        &self,
        source_bytes: Vec<u8>,
        size_px: u32,
        max_pixels: u32,
    ) -> ProviderResult<Vec<u8>> {
        let result = tokio::task::spawn_blocking(move || -> ProviderResult<Vec<u8>> {
            // 1. Decode with format auto-detection. ImageReader::with_format_decoded
            //    or with_guessed_format reads the magic bytes and picks the codec.
            let reader = ImageReader::new(Cursor::new(&source_bytes))
                .with_guessed_format()
                .map_err(|e| PreviewError::Decode(format!("format guess: {e}")))?;
            // 2. Peek dimensions before the full decode so we can reject
            //    sources that would blow up memory.
            let (w, h) = reader
                .clone()
                .into_dimensions()
                .map_err(|e| PreviewError::Decode(format!("dimensions: {e}")))?;
            if w.saturating_mul(h) > max_pixels {
                return Err(PreviewError::SourceTooLarge {
                    width: w,
                    height: h,
                    max: max_pixels,
                });
            }
            let img = reader
                .decode()
                .map_err(|e| PreviewError::Decode(format!("decode: {e}")))?;
            // 3. Resize so the longest edge equals size_px. `Lanczos3` is
            //    high quality; switch to `Triangle` if profiling shows
            //    decoding+resize is too slow.
            let thumb = img.thumbnail(size_px, size_px);
            // 4. Encode JPEG q80.
            let mut out = Vec::with_capacity(64 * 1024);
            let mut encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            // `image::DynamicImage::write_with_encoder` is the API in
            // `image = "0.25"`. The exact method name may differ in newer
            // patch releases — if it doesn't compile, try
            // `thumb.to_rgb8().write_with_encoder(encoder)` instead.
            thumb
                .write_with_encoder(encoder)
                .map_err(|e| PreviewError::Encode(format!("jpeg: {e}")))?;
            Ok(out)
        })
        .await
        .map_err(|e| {
            PreviewError::Decode(format!("image provider task join: {e}"))
        })??;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn synthesize_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    fn synthesize_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn synthesize_animated_gif(_w: u32, _h: u32) -> Vec<u8> {
        // Build a 2-frame 64x32 GIF in memory. `image::codecs::gif` exposes a
        // streaming encoder; we use it to drop both frames in.
        use image::codecs::gif::GifEncoder;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = GifEncoder::new(&mut buf);
            for tint in [0u8, 200u8] {
                let frame = image::RgbaImage::from_pixel(64, 32, image::Rgba([tint, tint, tint, 255]));
                let f = image::Frame::new(frame);
                enc.encode_frame(f).unwrap();
            }
        }
        buf.into_inner()
    }

    #[tokio::test]
    async fn resizes_jpeg_to_target_max_dim() {
        let bytes = synthesize_jpeg(800, 600);
        let out = ImageProvider
            .render(bytes, 256, 64 * 1024 * 1024)
            .await
            .unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert!(img.width() <= 256);
        assert!(img.height() <= 256);
        assert!(img.width() == 256 || img.height() == 256);
    }

    #[tokio::test]
    async fn preserves_aspect_ratio() {
        // 1024x512 → 256x128 at size=256.
        let bytes = synthesize_jpeg(1024, 512);
        let out = ImageProvider
            .render(bytes, 256, 64 * 1024 * 1024)
            .await
            .unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!(img.width(), 256);
        assert_eq!(img.height(), 128);
    }

    #[tokio::test]
    async fn handles_png_alpha_source() {
        let bytes = synthesize_png(400, 300);
        let out = ImageProvider
            .render(bytes, 64, 64 * 1024 * 1024)
            .await
            .unwrap();
        // Output is JPEG, so alpha is folded.
        let img = image::load_from_memory(&out).unwrap();
        assert!(img.width() <= 64);
    }

    #[tokio::test]
    async fn strips_animation_to_single_frame() {
        let bytes = synthesize_animated_gif(64, 32);
        let out = ImageProvider
            .render(bytes, 64, 64 * 1024 * 1024)
            .await
            .unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert!(img.width() <= 64);
        assert!(img.height() <= 64);
    }

    #[tokio::test]
    async fn rejects_source_above_max_pixels() {
        let bytes = synthesize_jpeg(500, 500);
        let r = ImageProvider
            .render(bytes, 64, 100_000) // cap below 500*500 = 250_000
            .await;
        match r {
            Err(PreviewError::SourceTooLarge { width: 500, height: 500, .. }) => {}
            other => panic!("expected SourceTooLarge, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-preview providers::image::tests
```

Expected: 5 tests pass. If the `write_with_encoder` API has shifted in `image = "0.25.x"`, follow the inline note and try the alternate form `thumb.to_rgb8().write_with_encoder(encoder)` (decoded to RGB8 first; drops alpha which is fine for JPEG output).

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-preview/src/providers/image.rs
git commit -m "preview: ImageProvider (image crate, JPEG output, max-pixel safety cap)"
```

### Task A5: `PdfProvider`

**Files:**
- Modify: `crates/crabcloud-preview/src/providers/pdf.rs`

- [ ] **Step 1: Inspect the hayro API surface**

Before implementing, verify hayro 0.7's API. Pin the version with `cargo doc -p hayro --no-deps --open` (or just `cargo expand` on a tiny test). Expected shape (subject to verification):

```rust
let pdf = hayro::Pdf::from_bytes(&pdf_bytes)?;
let page = pdf.page(0)?;
let (w, h) = page.size();   // points or pixels?
let scale = ...;             // pick scale so longest_edge maps to size_px
let rendered = page.render(scale)?;   // returns image::RgbaImage or raw RGBA Vec
```

If the API differs significantly from this sketch, adapt — the goal is "decode PDF, render page 0 to RGBA pixels, then resize+encode via the existing image-crate path."

- [ ] **Step 2: Write impl + tests**

```rust
//! PDF source provider via hayro (pure-Rust PDF renderer). Renders page 0
//! at a scale that maps the longest edge to the target size, then funnels
//! the result through `image::imageops::resize` + JPEG encode (q80) so the
//! output matches `ImageProvider`'s shape byte-for-byte.

use crate::error::PreviewError;
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;

pub struct PdfProvider;

const JPEG_QUALITY: u8 = 80;

#[async_trait]
impl PreviewProvider for PdfProvider {
    async fn render(
        &self,
        source_bytes: Vec<u8>,
        size_px: u32,
        max_pixels: u32,
    ) -> ProviderResult<Vec<u8>> {
        let result = tokio::task::spawn_blocking(move || -> ProviderResult<Vec<u8>> {
            // 1. Load the PDF.
            let pdf = hayro::Pdf::from_bytes(&source_bytes)
                .map_err(|e| PreviewError::PdfRender(format!("load: {e}")))?;
            // 2. Get page 0.
            let page = pdf
                .page(0)
                .map_err(|e| PreviewError::PdfRender(format!("no page 0: {e}")))?;
            // 3. Pick a scale so the longest edge maps to size_px.
            //    `page.size()` returns the unscaled dimensions. The exact
            //    units (points vs pixels) depend on the hayro API.
            let (w, h) = page.size();
            let max_edge = w.max(h);
            if max_edge == 0.0 {
                return Err(PreviewError::PdfRender("page has zero size".into()));
            }
            let scale = (size_px as f32) / max_edge as f32;
            let scaled_w = ((w as f32) * scale).round() as u32;
            let scaled_h = ((h as f32) * scale).round() as u32;
            if scaled_w.saturating_mul(scaled_h) > max_pixels {
                return Err(PreviewError::SourceTooLarge {
                    width: scaled_w,
                    height: scaled_h,
                    max: max_pixels,
                });
            }
            // 4. Render. The hayro API may return RgbaImage directly, or
            //    raw RGBA Vec<u8> + dimensions. Both shapes are accepted
            //    below via DynamicImage construction.
            let rendered = page
                .render(scale)
                .map_err(|e| PreviewError::PdfRender(format!("render: {e}")))?;
            // Adjust the following construction to whatever hayro returns.
            // For an `image::RgbaImage`:
            //     let dynimg = DynamicImage::ImageRgba8(rendered);
            // For raw RGBA Vec<u8>:
            //     let dynimg = DynamicImage::ImageRgba8(
            //         image::RgbaImage::from_raw(scaled_w, scaled_h, rendered)
            //             .ok_or_else(|| PreviewError::PdfRender("buffer/dim mismatch".into()))?,
            //     );
            let dynimg: DynamicImage = into_dynamic(rendered, scaled_w, scaled_h)?;
            // 5. Resize to exact long-edge=size_px (hayro's scale step may
            //    be off-by-one). thumbnail uses Lanczos3.
            let resized = dynimg.thumbnail(size_px, size_px);
            // 6. JPEG encode.
            let mut out = Vec::with_capacity(64 * 1024);
            let encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            resized
                .write_with_encoder(encoder)
                .map_err(|e| PreviewError::Encode(format!("jpeg: {e}")))?;
            Ok(out)
        })
        .await
        .map_err(|e| PreviewError::PdfRender(format!("provider task join: {e}")))??;
        Ok(result)
    }
}

/// Bridge hayro's render output into `image::DynamicImage`. Implementation
/// depends on the exact hayro return type; see the comments above for
/// both common shapes. Adapt this helper once hayro 0.7's API is
/// inspected — the rest of the file doesn't care.
fn into_dynamic(rendered: /* TODO: hayro::Rendered or RgbaImage */ image::RgbaImage, _w: u32, _h: u32) -> ProviderResult<DynamicImage> {
    Ok(DynamicImage::ImageRgba8(rendered))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid 1-page PDF in memory. Uses `printpdf` if
    /// available; otherwise hardcodes a tiny PDF. Since we don't want
    /// another dev-dep, hardcode the bytes.
    fn synthesize_one_page_pdf() -> Vec<u8> {
        // Minimal PDF/1.4 with one blank A4 page. Verified to be loadable
        // by hayro and qpdf.
        const PDF: &[u8] = b"\
            %PDF-1.4\n\
            1 0 obj <</Type/Catalog/Pages 2 0 R>> endobj\n\
            2 0 obj <</Type/Pages/Kids [3 0 R]/Count 1>> endobj\n\
            3 0 obj <</Type/Page/Parent 2 0 R/MediaBox [0 0 100 100]/Contents 4 0 R/Resources<<>>>> endobj\n\
            4 0 obj <</Length 0>> stream\nendstream endobj\n\
            xref\n\
            0 5\n\
            0000000000 65535 f \n\
            0000000010 00000 n \n\
            0000000053 00000 n \n\
            0000000102 00000 n \n\
            0000000182 00000 n \n\
            trailer <</Size 5/Root 1 0 R>>\n\
            startxref\n\
            235\n\
            %%EOF\n";
        PDF.to_vec()
    }

    #[tokio::test]
    async fn renders_first_page_as_jpeg() {
        let bytes = synthesize_one_page_pdf();
        let out = PdfProvider
            .render(bytes, 256, 64 * 1024 * 1024)
            .await
            .unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert!(img.width() <= 256);
        assert!(img.height() <= 256);
        assert!(img.width() == 256 || img.height() == 256);
    }

    #[tokio::test]
    async fn rejects_garbage_pdf() {
        let r = PdfProvider
            .render(b"not a pdf at all".to_vec(), 64, 64 * 1024 * 1024)
            .await;
        assert!(matches!(r, Err(PreviewError::PdfRender(_))));
    }

    #[tokio::test]
    async fn rejects_scaled_above_max_pixels() {
        let bytes = synthesize_one_page_pdf();
        // Tiny cap forces SourceTooLarge.
        let r = PdfProvider.render(bytes, 1024, 100).await;
        match r {
            Err(PreviewError::SourceTooLarge { .. }) => {}
            other => panic!("expected SourceTooLarge, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p crabcloud-preview providers::pdf::tests
```

If hayro's API doesn't match the sketch, the build will fail with a compiler error in `pdf.rs`. Open the file, follow the inline comments, and substitute the actual return type / method names. The interface that the tests assert is fixed (`PreviewProvider::render` produces JPEG bytes); only the body adapts.

If `synthesize_one_page_pdf` fails to load through hayro (the hand-crafted PDF is minimal and some renderers reject it), generate a real PDF at test-init using a tiny dev-dep like `printpdf = "0.7"`. Add to `[dev-dependencies]` and synthesize via:

```rust
fn synthesize_one_page_pdf() -> Vec<u8> {
    use printpdf::{PdfDocument, Mm};
    let (doc, page1, layer1) = PdfDocument::new("test", Mm(100.0), Mm(100.0), "layer");
    doc.get_page(page1).get_layer(layer1).end_layer();
    let mut buf = Vec::new();
    doc.save(&mut std::io::BufWriter::new(&mut buf)).unwrap();
    buf
}
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-preview/src/providers/pdf.rs crates/crabcloud-preview/Cargo.toml
git commit -m "preview: PdfProvider (hayro page-0 render + JPEG q80 output)"
```

### Task A6: `PreviewCache::get_or_render`

**Files:**
- Create: `crates/crabcloud-preview/src/cache.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! On-disk preview cache + per-key dedup lock.
//!
//! Cache layout:
//!   <preview_root>/<storage_id>/<fileid>/<size>-<source_etag>.jpg
//!
//! Reads check the exact path; writes atomically rename a tempfile into
//! place and then sweep any sibling files matching `<size>-*` that don't
//! match the current etag. The dedup `DashMap` ensures concurrent first-
//! request renders for the same (storage_id, fileid, size) share one task.

use crate::error::PreviewError;
use crate::ladder::round_up_to_ladder;
use crate::provider::{PreviewProvider, ProviderResult};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

#[derive(Debug, Clone, Copy)]
pub struct PreviewCachedEntry {
    pub size_px: u32,
    pub bytes: u64,
}

type DedupKey = (String, i64, u32);
type DedupCell = Arc<OnceCell<ProviderResult<PathBuf>>>;

pub struct PreviewCache {
    root: PathBuf,
    max_pixels: u32,
    locks: DashMap<DedupKey, DedupCell>,
}

impl PreviewCache {
    pub fn new(root: PathBuf, max_pixels: u32) -> Self {
        Self { root, max_pixels, locks: DashMap::new() }
    }

    /// Returns a path to a JPEG containing the requested preview. If the
    /// cache file already exists, returns immediately. Otherwise reads
    /// `source_bytes` via the caller-supplied closure (so the cache layer
    /// doesn't depend on `View`), dispatches to `provider`, and writes
    /// the result atomically.
    pub async fn get_or_render<F, Fut>(
        &self,
        storage_id: &str,
        fileid: i64,
        requested_size: u32,
        source_etag: &str,
        source_mime: &str,
        provider: &'static dyn PreviewProvider,
        read_source: F,
    ) -> Result<(PathBuf, u32), PreviewError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, PreviewError>>,
    {
        let _ = source_mime; // mime is the caller's discriminator; cache stores by id
        let size = round_up_to_ladder(requested_size)?;
        let cache_path = self.path_for(storage_id, fileid, size, source_etag);

        // Fast path: cache hit.
        if tokio::fs::try_exists(&cache_path).await? {
            return Ok((cache_path, size));
        }

        // Dedup lock — concurrent first-request renders share one task.
        let key: DedupKey = (storage_id.to_string(), fileid, size);
        let cell: DedupCell = self
            .locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let result = cell
            .get_or_init(|| async {
                self.render_and_write(
                    storage_id,
                    fileid,
                    size,
                    source_etag,
                    provider,
                    read_source().await?,
                )
                .await
            })
            .await
            .clone();
        // Drop the dedup entry — subsequent reads hit the on-disk cache.
        self.locks.remove(&key);
        let path = result?;
        Ok((path, size))
    }

    fn path_for(&self, storage_id: &str, fileid: i64, size: u32, etag: &str) -> PathBuf {
        let safe_storage = sanitize_path_component(storage_id);
        self.root
            .join(safe_storage)
            .join(fileid.to_string())
            .join(format!("{size}-{etag}.jpg"))
    }

    async fn render_and_write(
        &self,
        storage_id: &str,
        fileid: i64,
        size: u32,
        etag: &str,
        provider: &'static dyn PreviewProvider,
        source_bytes: Vec<u8>,
    ) -> ProviderResult<PathBuf> {
        let cache_path = self.path_for(storage_id, fileid, size, etag);
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let jpeg = provider.render(source_bytes, size, self.max_pixels).await?;
        // Atomic write: temp file in the same directory + rename.
        let tmp = cache_path.with_extension("jpg.tmp");
        tokio::fs::write(&tmp, &jpeg).await?;
        tokio::fs::rename(&tmp, &cache_path).await?;
        // Sweep stale siblings (same fileid/size, different etag).
        self.sweep_stale_siblings(cache_path.parent().unwrap(), size, etag)
            .await
            .ok(); // best-effort
        Ok(cache_path)
    }

    async fn sweep_stale_siblings(
        &self,
        dir: &Path,
        size: u32,
        keep_etag: &str,
    ) -> std::io::Result<()> {
        let prefix = format!("{size}-");
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = match name.to_str() {
                Some(s) => s,
                None => continue,
            };
            if !name_str.starts_with(&prefix) || !name_str.ends_with(".jpg") {
                continue;
            }
            let middle = &name_str[prefix.len()..name_str.len() - 4];
            if middle != keep_etag {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        Ok(())
    }
}

/// Strip characters that would be hostile to a path component. Storage ids
/// in Crabcloud are restricted to ASCII (per the existing `oc_storages`
/// schema) but defensive sanitization costs nothing.
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ImageProvider;
    use image::ImageFormat;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn synth_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(100, 80, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    #[tokio::test]
    async fn cache_miss_renders_and_writes_then_hits() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let bytes = synth_jpeg();

        let (p1, size1) = cache
            .get_or_render(
                "sid",
                7,
                64,
                "abc123def456",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert_eq!(size1, 64);
        assert!(p1.exists());

        // Second call MUST NOT invoke read_source (we'd panic).
        let (p2, _) = cache
            .get_or_render(
                "sid",
                7,
                64,
                "abc123def456",
                "image/jpeg",
                &ImageProvider,
                || async { panic!("read_source called on cache hit") },
            )
            .await
            .unwrap();
        assert_eq!(p1, p2);
    }

    #[tokio::test]
    async fn cache_rounds_up_to_ladder() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let bytes = synth_jpeg();
        let (path, size) = cache
            .get_or_render(
                "sid",
                1,
                200,
                "etag1234567890aa",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert_eq!(size, 256);
        assert!(path.to_string_lossy().contains("256-etag1234567890aa.jpg"));
    }

    #[tokio::test]
    async fn cache_rejects_oversize_request() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let r = cache
            .get_or_render(
                "sid",
                1,
                4096,
                "etag",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(vec![]) },
            )
            .await;
        assert!(matches!(r, Err(PreviewError::SizeOutOfRange(4096))));
    }

    #[tokio::test]
    async fn cache_sweeps_stale_etag_siblings() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        // Seed an old preview file directly.
        let dir = tmp.path().join("sid").join("3");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let stale = dir.join("64-oldetag.jpg");
        tokio::fs::write(&stale, b"old").await.unwrap();

        let bytes = synth_jpeg();
        let (fresh_path, _) = cache
            .get_or_render(
                "sid",
                3,
                64,
                "newetag",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert!(fresh_path.exists());
        assert!(!stale.exists(), "stale sibling must be deleted after fresh render");
    }

    #[tokio::test]
    async fn dedup_lock_serializes_concurrent_renders() {
        let tmp = TempDir::new().unwrap();
        let cache = Arc::new(PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024));
        let counter = Arc::new(AtomicUsize::new(0));

        // Spawn N concurrent renders of the same (sid, fileid, size).
        let mut tasks = Vec::new();
        for _ in 0..10 {
            let cache = cache.clone();
            let counter = counter.clone();
            let bytes = synth_jpeg();
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_render(
                        "sid",
                        9,
                        64,
                        "concurrentetag123",
                        "image/jpeg",
                        &ImageProvider,
                        || async {
                            counter.fetch_add(1, Ordering::SeqCst);
                            Ok(bytes)
                        },
                    )
                    .await
                    .unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        // Only the winning task actually called read_source.
        assert_eq!(counter.load(Ordering::SeqCst), 1, "dedup must collapse concurrent reads");
    }
}
```

Note: the `OnceCell::get_or_init` future requires the closure to return a `ProviderResult<PathBuf>`. The `cell.get_or_init` returns `&ProviderResult<PathBuf>`, so the `.clone()` is needed because `PathBuf` is `Clone` and we want to escape the reference. Verify the borrow checker passes; if not, restructure to store an owned `PathBuf` and clone at the read site.

Also note `tokio::sync::OnceCell::get_or_init` takes a closure returning a future that produces the value. If the value is `Result<_, E>`, the cell stores the result; subsequent calls return the same result. Make sure `PreviewError` is `Clone` for this — if not, swap `OnceCell<Result>` for `OnceCell<Arc<Result>>` or similar.

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-preview cache::tests
```

Expected: 5 tests pass. If `PreviewError: !Clone` breaks the `cell.get_or_init().clone()`, derive `Clone` for `PreviewError` (and `From<std::io::Error>` becomes `From<Arc<std::io::Error>>` or similar). Simpler: store `Result<PathBuf, Arc<PreviewError>>` in the cell.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-preview/src/cache.rs
git commit -m "preview: PreviewCache::get_or_render with per-key dedup + stale sibling sweep"
```

### Task A7: Pre-PR sweep + PR

- [ ] **Step 1: Sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Fix any drift. The workspace `unused_crate_dependencies = "warn"` may flag dev-deps that test scaffolding pulls in but tests don't use. Use `#[cfg(test)] use foo as _;` anchors in `lib.rs` for each.

- [ ] **Step 2: Push + open PR**

```bash
git push -u origin sp10/a-preview-crate
gh pr create --title "sp10(a): crabcloud-preview foundation (ladder, providers, cache)" --body "$(cat <<'EOF'
## Summary
- New `crabcloud-preview` crate scaffolding.
- `LADDER = [64, 256, 1024]` + `round_up_to_ladder`.
- `PreviewProvider` trait + `provider_for_mime` dispatch.
- `ImageProvider`: image crate, JPEG q80, max-pixel safety cap, animation stripped.
- `PdfProvider`: hayro page-0 render → JPEG q80.
- `PreviewCache::get_or_render`: on-disk cache, atomic temp-file write, stale-sibling sweep, per-key dedup lock via DashMap + tokio OnceCell.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-preview` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Merge after green.**

---

# Batch B — Authed handler + config

**Branch:** `sp10/b-authed-handler`

**Goal:** Add `FileConfig::preview_root` + `preview_max_pixels`. Authed `GET /api/files/preview/{fileid}?size=N` handler that looks up the filecache row, dispatches via `provider_for_mime`, delegates to `PreviewCache::get_or_render`. E2E tests across all the spec-§4.2 cases.

### Task B1: `FileConfig` fields + preview-root default + AppState wiring

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`
- Modify: `crates/crabcloud-config/src/test_support.rs`
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Add fields**

In `crates/crabcloud-config/src/types.rs`, in the `FileConfig` struct, add:

```rust
    /// Filesystem root for the preview cache. Defaults to
    /// `<datadirectory>/appdata/preview`. Operators can override to point
    /// at a different mount.
    #[serde(default)]
    pub preview_root: Option<std::path::PathBuf>,
    /// Maximum decoded source dimensions, in total pixels. Sources whose
    /// decoded `width * height` exceeds this limit are rejected with 413.
    /// Default: 64 megapixels (~8000x8000).
    #[serde(default = "default_preview_max_pixels")]
    pub preview_max_pixels: u32,
```

And the default helper:

```rust
fn default_preview_max_pixels() -> u32 {
    64 * 1024 * 1024
}
```

- [ ] **Step 2: Test-support init**

In `crates/crabcloud-config/src/test_support.rs::minimal_sqlite_config`, add:

```rust
        preview_root: None,
        preview_max_pixels: 64 * 1024 * 1024,
```

(Place alongside the other field initializations.)

- [ ] **Step 3: AppState wiring**

In `crates/crabcloud-core/src/state.rs`:

1. Add an import: `use crabcloud_preview::PreviewCache;`
2. Add to the `AppState` struct (alongside `publiclinks_auth`):
   ```rust
       /// Preview cache for thumbnail / first-page-PDF previews.
       pub preview: Arc<PreviewCache>,
   ```
3. In `AppStateBuilder::build()`, AFTER the existing `publiclinks_auth` construction (so the data directory exists), add:
   ```rust
   let preview_root = self
       .config
       .preview_root
       .clone()
       .unwrap_or_else(|| self.config.datadirectory.join("appdata").join("preview"));
   tokio::fs::create_dir_all(&preview_root).await.ok();
   let preview = Arc::new(PreviewCache::new(
       preview_root,
       self.config.preview_max_pixels,
   ));
   ```
4. Include `preview` in the returned `AppState { ... }`.

Add to `crates/crabcloud-core/Cargo.toml`:

```toml
crabcloud-preview = { workspace = true }
```

- [ ] **Step 4: Build**

```bash
cargo build -p crabcloud-config -p crabcloud-core
```

Hunt for any other `FileConfig { ... }` literals across the workspace and add the two new fields:

```bash
rg "FileConfig \{" crates --type rust
```

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-config/ crates/crabcloud-core/
git commit -m "config+core: preview_root + preview_max_pixels + AppState.preview wiring"
```

### Task B2: Authed handler

**Files:**
- Create: `crates/crabcloud-http/src/routes/files_preview.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs`
- Modify: `crates/crabcloud-http/src/router.rs`
- Modify: `crates/crabcloud-http/Cargo.toml`

- [ ] **Step 1: Add deps**

In `crates/crabcloud-http/Cargo.toml`, add to `[dependencies]`:

```toml
crabcloud-preview = { workspace = true }
```

(other deps like `tokio-util` are already there.)

- [ ] **Step 2: Register the module**

In `crates/crabcloud-http/src/routes/mod.rs`, add `pub mod files_preview;` next to the existing `pub mod files_zip;`.

- [ ] **Step 3: Write the handler**

Create `crates/crabcloud-http/src/routes/files_preview.rs`:

```rust
//! `GET /api/files/preview/{fileid}?size=N` — authenticated thumbnail
//! download. Resolves the source via filecache, dispatches by mime,
//! returns the cached preview (or generates one).

use crate::extractors::auth::AuthenticatedUser;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use crabcloud_core::AppState;
use crabcloud_fs::path::UserPath;
use crabcloud_preview::{provider_for_mime, PreviewError};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/files/preview/{fileid}", get(handler))
}

#[derive(Deserialize)]
struct SizeQuery {
    #[serde(default = "default_size")]
    size: u32,
}

fn default_size() -> u32 {
    64
}

async fn handler(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    Path(fileid): Path<i64>,
    Query(q): Query<SizeQuery>,
    headers: HeaderMap,
) -> Response {
    let uid = crabcloud_users::UserId::new(authed.user_id.as_str())
        .expect("AuthenticatedUser.user_id is a validated UserId");

    // Look up filecache row by fileid; 404 if missing.
    let row = match state.filecache.lookup_by_id(fileid).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    // Authorize: the row's storage_id must match a storage the user has
    // a mount on. Cheapest: build the View and let it tell us.
    let view = match state.view_for(&uid).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    if !view.mounts().iter().any(|m| m.storage.id() == row.storage_id) {
        // file_id leak resistance: don't distinguish "wrong owner" from "not found".
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    // Provider dispatch by source mime.
    let provider = match provider_for_mime(row.mimetype.as_str()) {
        Some(p) => p,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
    };

    // Snap size to ladder (also catches > 1024).
    let snapped = match crabcloud_preview::round_up_to_ladder(q.size) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "").into_response(),
    };

    // Conditional GET: check If-None-Match against the composite ETag.
    let composite_etag = format!("\"{}-{}\"", row.etag.as_str(), snapped);
    if let Some(req_etag) = headers.get(header::IF_NONE_MATCH).and_then(|h| h.to_str().ok()) {
        if req_etag == composite_etag {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }

    // Get-or-render. The closure reads the source via the View.
    let source_path = match user_path_for_row(&view, &row) {
        Some(p) => p,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    let render_result = state
        .preview
        .get_or_render(
            &row.storage_id,
            row.fileid,
            q.size,
            row.etag.as_str(),
            row.mimetype.as_str(),
            provider,
            || async {
                use tokio::io::AsyncReadExt;
                let mut reader = view.read(&source_path).await.map_err(PreviewError::Fs)?;
                let mut buf = Vec::with_capacity(row.size as usize);
                reader.read_to_end(&mut buf).await.map_err(PreviewError::Io)?;
                Ok(buf)
            },
        )
        .await;

    let (cache_path, _size) = match render_result {
        Ok(t) => t,
        Err(PreviewError::SizeOutOfRange(_)) => return (StatusCode::BAD_REQUEST, "").into_response(),
        Err(PreviewError::Unsupported(_)) => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
        Err(PreviewError::SourceTooLarge { .. }) => return (StatusCode::PAYLOAD_TOO_LARGE, "").into_response(),
        Err(PreviewError::SourceNotFound(_)) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, fileid, "preview render failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // Stream the cache file as the response body.
    serve_cache_file(cache_path, composite_etag).await
}

/// Convert a filecache row into a user-facing path for `View::read`. The
/// row's `path` is the storage-relative path; we must surface it as the
/// user-facing path through whichever mount surfaces that storage. For
/// the simple home-mount case this is just `/<path>`. For share mounts
/// the recipient sees `/<share-basename>/...` so we walk the mounts and
/// build the right prefix.
fn user_path_for_row(view: &crabcloud_fs::View, row: &crabcloud_filecache::FilecacheRow) -> Option<UserPath> {
    use crabcloud_storage::StoragePath;
    // Find the mount whose storage.id() matches AND whose backing inner-
    // storage path is a prefix of `row.path`. For home mounts that's
    // `/<row.path>` directly; for SharedSubrootStorage we need to subtract
    // the inner prefix.
    for mount in view.mounts() {
        if mount.storage.id() != row.storage_id {
            continue;
        }
        // The cleanest approach: ask the mount for its inner-storage view
        // via Storage::inner_storage. If it returns Some((_, owner_path)),
        // strip `owner_path` from `row.path` to get the recipient-visible
        // suffix.
        if let Some((_, owner_path)) = mount.storage.inner_storage() {
            let owner = owner_path.as_str();
            let row_str = row.path.as_str();
            let suffix = if owner.is_empty() {
                row_str.to_string()
            } else if row_str == owner {
                String::new()
            } else if let Some(stripped) = row_str.strip_prefix(&format!("{owner}/")) {
                stripped.to_string()
            } else {
                continue;
            };
            let candidate = if mount.path_prefix.is_root() {
                format!("/{suffix}")
            } else if suffix.is_empty() {
                format!("/{}", mount.path_prefix.as_str())
            } else {
                format!("/{}/{}", mount.path_prefix.as_str(), suffix)
            };
            return UserPath::new(candidate.trim_end_matches('/')).ok().or_else(|| UserPath::new("/").ok());
        }
        // Home mount: the row's path IS the user-facing path.
        let candidate = format!("/{}", row.path.as_str());
        return UserPath::new(candidate).ok();
    }
    None
}

async fn serve_cache_file(path: std::path::PathBuf, etag: String) -> Response {
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&meta.len().to_string()).unwrap(),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    (StatusCode::OK, headers, body).into_response()
}
```

- [ ] **Step 4: Wire into `router.rs`**

In `crates/crabcloud-http/src/router.rs`, find where `files_zip::router()` is merged (it sits inside the auth'd surface). Add the parallel `files_preview::router()`:

```rust
        .merge(crate::routes::files_preview::router().with_state(state.clone()))
```

next to the existing `files_zip` merge.

- [ ] **Step 5: Build**

```bash
cargo build -p crabcloud-http
```

- [ ] **Step 6: Commit**

```bash
git add crates/crabcloud-http/
git commit -m "http: authed /api/files/preview/{fileid} handler"
```

### Task B3: E2E tests

**Files:**
- Modify: `crates/crabcloud-http/tests/support/mod.rs`
- Create: `crates/crabcloud-http/tests/files_preview_e2e.rs`

- [ ] **Step 1: Support helpers**

Append to `crates/crabcloud-http/tests/support/mod.rs`:

```rust
/// Seed a JPEG file under uid's home at `path`, returning its filecache row.
pub async fn seed_jpeg(
    state: &crabcloud_core::AppState,
    uid: &crabcloud_users::UserId,
    path: &str,
    width: u32,
    height: u32,
) -> crabcloud_filecache::FilecacheRow {
    use std::io::Cursor;
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .unwrap();
    seed_file_with_mime(state, uid, path, &buf.into_inner(), "image/jpeg").await
}

/// Seed a file with an explicit mime, returning its filecache row.
pub async fn seed_file_with_mime(
    state: &crabcloud_core::AppState,
    uid: &crabcloud_users::UserId,
    path: &str,
    bytes: &[u8],
    mime: &str,
) -> crabcloud_filecache::FilecacheRow {
    seed_file(state, uid, path, bytes).await;
    let storage_id = state
        .storage_factory
        .home_storage(uid)
        .await
        .unwrap()
        .id()
        .to_string();
    let sp = crabcloud_storage::StoragePath::new(path.trim_start_matches('/')).unwrap();
    let row = state
        .filecache
        .lookup(&storage_id, &sp)
        .await
        .unwrap()
        .expect("row should exist after seed_file");
    // If the mime differs from what the storage backend reports, mutate
    // it in the filecache via a stat refresh. For MemoryStorage seeded
    // via `seed_file`, the mime is sniffed from the bytes — `image::Rgb`
    // JPEGs sniff correctly. For seeded PDFs, ensure your synthesizer
    // outputs `%PDF-1.4\n...` at the start.
    let _ = mime;
    row
}
```

You'll also need to add `image = { workspace = true }` to `crates/crabcloud-http/Cargo.toml`'s `[dev-dependencies]`.

- [ ] **Step 2: Write the e2e tests**

Create `crates/crabcloud-http/tests/files_preview_e2e.rs`:

```rust
//! E2E for the authed preview endpoint.

mod support;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use std::io::Cursor;
use support::{bearer, make_state, seed_file_with_mime, seed_jpeg, seed_user};
use tower::ServiceExt;

#[tokio::test]
async fn preview_returns_jpeg_for_image_file() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, &uid, "/cat.jpg", 800, 600).await;
    let router = crabcloud_http::build_router(state.clone());
    let token = bearer(&state, &uid).await;

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "image/jpeg");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let img = image::load_from_memory(&body).unwrap();
    assert!(img.width() <= 64 && img.height() <= 64);
    assert!(img.width() == 64 || img.height() == 64);
}

#[tokio::test]
async fn preview_unsupported_mime_returns_415() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let row = seed_file_with_mime(&state, &uid, "/archive.zip", b"PK\x03\x04junk", "application/zip").await;
    let router = crabcloud_http::build_router(state.clone());
    let token = bearer(&state, &uid).await;

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn preview_unknown_fileid_returns_404() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let router = crabcloud_http::build_router(state.clone());
    let token = bearer(&state, &uid).await;

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/preview/999999?size=64")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_cross_user_fileid_returns_404() {
    let (state, _tmp) = make_state().await;
    let alice = seed_user(&state, "alice").await;
    let bob = seed_user(&state, "bob").await;
    let alice_row = seed_jpeg(&state, &alice, "/cat.jpg", 200, 200).await;
    let router = crabcloud_http::build_router(state.clone());
    let bob_token = bearer(&state, &bob).await;

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", alice_row.fileid))
                .header("Authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND); // NOT 403 — no leak
}

#[tokio::test]
async fn preview_size_too_large_returns_400() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, &uid, "/cat.jpg", 100, 100).await;
    let router = crabcloud_http::build_router(state.clone());
    let token = bearer(&state, &uid).await;

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=2048", row.fileid))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_no_auth_returns_401() {
    let (state, _tmp) = make_state().await;
    let _uid = seed_user(&state, "alice").await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/files/preview/1?size=64")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn preview_etag_revalidation_returns_304() {
    let (state, _tmp) = make_state().await;
    let uid = seed_user(&state, "alice").await;
    let row = seed_jpeg(&state, &uid, "/cat.jpg", 200, 200).await;
    let router = crabcloud_http::build_router(state.clone());
    let token = bearer(&state, &uid).await;

    let r1 = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let etag = r1.headers().get(header::ETAG).unwrap().to_str().unwrap().to_string();

    let r2 = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/preview/{}?size=64", row.fileid))
                .header("Authorization", format!("Bearer {token}"))
                .header("If-None-Match", &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::NOT_MODIFIED);
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p crabcloud-http --test files_preview_e2e
```

Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-http/
git commit -m "http(tests): e2e for authed preview endpoint (7 cases)"
```

### Task B4: Pre-PR sweep + PR

Standard. PR title: `sp10(b): config + authed /api/files/preview/{fileid}`.

---

# Batch C — Public-link handler

**Branch:** `sp10/c-public-handler`

**Goal:** Public `GET /s/{token}/preview/{*path}?size=N` handler in `public_link/preview.rs`. Read bit + password gate + standard PublicLink view construction.

### Task C1: Public handler

**Files:**
- Create: `crates/crabcloud-http/src/routes/public_link/preview.rs`
- Modify: `crates/crabcloud-http/src/routes/public_link/mod.rs`

- [ ] **Step 1: Register the module**

In `crates/crabcloud-http/src/routes/public_link/mod.rs`, add `mod preview;` next to the existing handler modules, and in `pub fn router()`:

```rust
        .route("/s/{token}/preview/{*path}", axum::routing::get(preview::handler))
```

- [ ] **Step 2: Write the handler**

Create `crates/crabcloud-http/src/routes/public_link/preview.rs`:

```rust
//! `GET /s/{token}/preview/{*path}?size=N` — anonymous thumbnail
//! download. Same provider/cache backend as the authed endpoint, gated
//! by the public-link read bit + password state.

use crabcloud_publiclinks::PublicLinkAuthContext;
use super::{build_view, fs_err_to_response};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::AppState;
use crabcloud_fs::path::UserPath;
use crabcloud_preview::{provider_for_mime, PreviewError};
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::FileKind;
use serde::Deserialize;

#[derive(Deserialize)]
struct PreviewQuery {
    #[serde(default = "default_size")]
    size: u32,
}

fn default_size() -> u32 {
    64
}

pub async fn handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    Path((_token, raw_path)): Path<(String, String)>,
    Query(q): Query<PreviewQuery>,
    headers: HeaderMap,
) -> Response {
    if ctx.password_gate_required {
        return (StatusCode::FORBIDDEN, "password_required").into_response();
    }
    let perms = SharePermissions::from_wire(ctx.permissions);
    if !perms.contains_read() {
        return (StatusCode::FORBIDDEN, "read_not_granted").into_response();
    }

    let user_path = match UserPath::new(format!("/{}", raw_path.trim_start_matches('/'))) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    };

    let view = match build_view(&state, &ctx).await {
        Ok(v) => v,
        Err(e) => return fs_err_to_response(e),
    };

    // Stat for mime + etag + storage_id + fileid.
    let meta = match view.stat(&user_path).await {
        Ok(m) => m,
        Err(e) => return fs_err_to_response(e),
    };
    if !matches!(meta.kind, FileKind::File) {
        return (StatusCode::BAD_REQUEST, "not a file").into_response();
    }
    // Resolve the storage row to get a stable storage_id + fileid + etag.
    let (cache_storage, cache_path) = match view.cache_key_for(&user_path) {
        Ok(t) => t,
        Err(e) => return fs_err_to_response(e),
    };
    let storage_id = cache_storage.id().to_string();
    let row = match state.filecache.lookup(&storage_id, &cache_path).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response(),
    };

    let provider = match provider_for_mime(row.mimetype.as_str()) {
        Some(p) => p,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
    };

    let snapped = match crabcloud_preview::round_up_to_ladder(q.size) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "").into_response(),
    };
    let composite_etag = format!("\"{}-{}\"", row.etag.as_str(), snapped);
    if let Some(req_etag) = headers.get(header::IF_NONE_MATCH).and_then(|h| h.to_str().ok()) {
        if req_etag == composite_etag {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }

    let view_for_read = view; // moved into closure
    let user_path_for_read = user_path.clone();
    let row_size = row.size;
    let render_result = state
        .preview
        .get_or_render(
            &row.storage_id,
            row.fileid,
            q.size,
            row.etag.as_str(),
            row.mimetype.as_str(),
            provider,
            || async move {
                use tokio::io::AsyncReadExt;
                let mut reader = view_for_read
                    .read(&user_path_for_read)
                    .await
                    .map_err(PreviewError::Fs)?;
                let mut buf = Vec::with_capacity(row_size as usize);
                reader.read_to_end(&mut buf).await.map_err(PreviewError::Io)?;
                Ok(buf)
            },
        )
        .await;

    let (cache_file, _) = match render_result {
        Ok(t) => t,
        Err(PreviewError::SizeOutOfRange(_)) => return (StatusCode::BAD_REQUEST, "").into_response(),
        Err(PreviewError::Unsupported(_)) => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "").into_response(),
        Err(PreviewError::SourceTooLarge { .. }) => return (StatusCode::PAYLOAD_TOO_LARGE, "").into_response(),
        Err(PreviewError::SourceNotFound(_)) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "public preview render failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    crate::routes::files_preview::serve_cache_file(cache_file, composite_etag).await
}
```

This calls `crate::routes::files_preview::serve_cache_file` (a `pub(crate)` helper). Promote `serve_cache_file` from `async fn` to `pub(crate) async fn` in `crates/crabcloud-http/src/routes/files_preview.rs` so both surfaces share the response builder.

- [ ] **Step 3: Build**

```bash
cargo build -p crabcloud-http
```

If the `Path` extractor doesn't accept `(String, String)` for a route with `{token}/preview/{*path}`, switch to two separate `Path` extractors or to `axum::extract::OriginalUri`. The plan assumes axum's standard tuple extraction.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-http/src/routes/
git commit -m "http(public_link): GET /s/{token}/preview/{*path} handler"
```

### Task C2: E2E tests

**Files:**
- Modify: `crates/crabcloud-http/tests/public_link_e2e.rs`
- Modify: `crates/crabcloud-http/tests/support/mod.rs` (add helpers if missing)

- [ ] **Step 1: Add tests**

Append to `crates/crabcloud-http/tests/public_link_e2e.rs`:

```rust
#[tokio::test]
async fn public_preview_read_link_returns_jpeg() {
    use std::io::Cursor;
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    let _row = support::seed_jpeg(&state, &uid, "/Photos/cat.jpg", 500, 400).await;
    let token = support::create_link(&state, &uid, "/Photos", 1, None, None).await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/preview/cat.jpg?size=64"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let img = image::load_from_memory(&body).unwrap();
    assert!(img.width() <= 64 && img.height() <= 64);
}

#[tokio::test]
async fn public_preview_create_only_link_returns_403() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    let _ = support::seed_jpeg(&state, &uid, "/Drop/cat.jpg", 100, 100).await;
    let token = support::create_link(&state, &uid, "/Drop", 4, None, None).await; // perms=4 file-drop
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/preview/cat.jpg?size=64"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_preview_password_gated_no_cookie_returns_403() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    let _ = support::seed_jpeg(&state, &uid, "/Photos/cat.jpg", 100, 100).await;
    let token = support::create_link(&state, &uid, "/Photos", 1, Some("hunter2".to_string()), None).await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/preview/cat.jpg?size=64"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert!(std::str::from_utf8(&body).unwrap().contains("password_required"));
}

#[tokio::test]
async fn public_preview_expired_token_returns_404() {
    use chrono::NaiveDate;
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    let _ = support::seed_jpeg(&state, &uid, "/Photos/cat.jpg", 100, 100).await;
    let past = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let token = support::create_link(&state, &uid, "/Photos", 1, None, Some(past)).await;
    let router = crabcloud_http::build_router(state.clone());
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/preview/cat.jpg?size=64"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_preview_unsupported_mime_returns_415() {
    let (state, _tmp) = support::make_state().await;
    let uid = support::seed_user(&state, "alice").await;
    let _ = support::seed_file_with_mime(&state, &uid, "/Photos/archive.zip", b"PK\x03\x04junk", "application/zip").await;
    let token = support::create_link(&state, &uid, "/Photos", 1, None, None).await;
    let router = crabcloud_http::build_router(state.clone());

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/s/{token}/preview/archive.zip?size=64"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-http --test public_link_e2e
```

Expected: pre-existing tests pass + 5 new preview tests.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-http/tests/
git commit -m "http(tests): e2e for public-link preview endpoint (5 cases)"
```

### Task C3: Pre-PR sweep + PR

Standard. PR title: `sp10(c): public-link /s/{token}/preview/{*path}`.

---

# Batch D — Files UI integration

**Branch:** `sp10/d-ui-integration`

**Goal:** `FileRow` and `PublicListing` render inline thumbnails for previewable mimes via `<img onerror=fallback>`.

### Task D1: Mime-allowlist helper

**Files:**
- Modify: `crates/crabcloud-app/src/components/file_row.rs` (or wherever FileRow lives — find it via `rg "fn FileRow" crates/crabcloud-app`)

- [ ] **Step 1: Find the FileRow component**

```bash
rg "fn FileRow|component FileRow|#\[component\]" crates/crabcloud-app/src
```

Locate the function that returns the per-row JSX. Note the entry's mime field name (likely `entry.mimetype` or `entry.mime`).

- [ ] **Step 2: Add the previewable check**

In the same file, add a helper near the top:

```rust
fn is_previewable_mime(mime: &str) -> bool {
    let lc = mime.to_ascii_lowercase();
    lc.starts_with("image/jpeg")
        || lc.starts_with("image/png")
        || lc.starts_with("image/gif")
        || lc.starts_with("image/webp")
        || lc.starts_with("application/pdf")
}
```

(Mirror exactly the server-side `provider_for_mime` matchers so client/server allowlists stay in sync. A follow-up SP could centralize this in a shared crate; for MVP, duplicate.)

- [ ] **Step 3: Render thumbnail conditionally**

In the FileRow `rsx!` block, find where the generic icon is rendered. Replace:

```rust
rsx! {
    img { class: "file-icon", src: "/static/icons/file.svg" }
}
```

(or whatever the existing form is) with:

```rust
let icon = if entry.kind == FileKind::Directory {
    rsx! { img { class: "file-icon", src: "/static/icons/folder.svg" } }
} else if is_previewable_mime(&entry.mimetype) {
    let preview_url = format!("/api/files/preview/{}?size=64", entry.fileid);
    rsx! {
        img {
            class: "file-thumb",
            src: "{preview_url}",
            // onerror fallback handled by the FallbackImage wrapper below
            "onerror": "this.onerror=null;this.src='/static/icons/file.svg';this.classList.add('file-icon');this.classList.remove('file-thumb');",
        }
    }
} else {
    rsx! { img { class: "file-icon", src: "/static/icons/file.svg" } }
};
```

Adjust the `entry` field names to whatever exists. The `onerror` JS swaps the `src` to the generic icon on 404 / 415 / network error; the class swap restores the icon styling.

If the project uses a `FallbackImage` component or similar abstraction for graceful image fallback, use that instead of inline `onerror`.

- [ ] **Step 4: Build the WASM bundle**

```bash
cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
```

(Or whatever the existing dx build invocation is — check the project's CI.)

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-app/src/
git commit -m "app(files): inline thumbnails for previewable mimes (with onerror fallback)"
```

### Task D2: PublicListing thumbnails

**Files:**
- Modify: `crates/crabcloud-app/src/pages/public_link.rs`

- [ ] **Step 1: Find the PublicListing row component**

In `crates/crabcloud-app/src/pages/public_link.rs`, locate `PublicListing` (and `PublicRow` if separate). It surfaces each file with the link's prefix.

- [ ] **Step 2: Add the same is_previewable_mime helper + img tag**

Copy the helper from `file_row.rs` into `public_link.rs` (or factor to a shared `crates/crabcloud-app/src/preview_mime.rs` module and `use` from both).

In the row rendering, replace the generic file icon with the conditional `<img>` block. The preview URL for public links uses the token + path:

```rust
let preview_url = format!("/s/{}/preview/{}?size=64", token, entry.path);
```

where `entry.path` is the file's user-visible path inside the link. Verify against the existing PublicListing data shape — `path` may be relative or absolute.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-app/
git commit -m "app(public_link): inline thumbnails in PublicListing"
```

### Task D3: SSR snapshot

**Files:**
- Modify: `crates/crabcloud-app/tests/server_fns_files.rs` (or create if missing)

- [ ] **Step 1: Add a snapshot assertion**

Add a test that asserts the rendered HTML for a previewable file contains the expected `<img src=...>` tag:

```rust
#[tokio::test]
async fn file_row_renders_thumbnail_for_image_mime() {
    // Build a minimal FileRow with a JPEG entry and SSR-render it.
    // Assert the resulting HTML contains `src="/api/files/preview/`.
    // (The exact SSR scaffolding depends on how the existing tests in
    // this file work — mirror the pattern.)
}
```

Look at any existing snapshot tests in the file (`rg "render_to_string|ssr_render" crates/crabcloud-app/tests/`) and use the same harness.

If no snapshot harness exists yet, a simpler assertion is to call the server-fn that returns the listing JSON and confirm the fileid is in the response — the UI-side thumbnail wiring is then verified manually in dev. Mark such a placeholder test as `#[ignore]` with a comment explaining the limitation, OR escalate to DONE_WITH_CONCERNS.

- [ ] **Step 2: Run + commit**

```bash
cargo test -p crabcloud-app
```

```bash
git add crates/crabcloud-app/tests/
git commit -m "app(tests): assert FileRow emits preview <img> for previewable mimes"
```

### Task D4: Pre-PR sweep + PR

Standard. PR title: `sp10(d): Files UI inline thumbnails (authed + public-link)`.

---

## Acceptance criteria (spec → coverage map)

| Spec section | Test / artifact |
|---|---|
| §2 Decision 1 (new crate) | Batch A Task A1. |
| §2 Decision 2 (provider trait) | Batch A Task A3 + A4 + A5. |
| §2 Decision 3 (hayro for PDF) | Batch A Task A5 + `pdf_provider_renders_first_page_of_two` unit test. |
| §2 Decision 4 (on-disk cache) | Batch A Task A6 + `cache_miss_renders_and_writes_then_hits` test. |
| §2 Decision 5 (per-key dedup lock) | Batch A Task A6 + `dedup_lock_serializes_concurrent_renders` test. |
| §2 Decision 6 (size ladder) | Batch A Task A2 ladder tests + Batch B `preview_size_too_large_returns_400`. |
| §2 Decision 7 (always JPEG q80) | Batch A image+pdf tests assert JPEG output. |
| §2 Decision 8 (ETag-based staleness) | Batch A `cache_sweeps_stale_etag_siblings` test. |
| §2 Decision 9 (public-link gate) | Batch C `public_preview_create_only_link_returns_403` + `public_preview_password_gated_no_cookie_returns_403`. |
| §2 Decision 10 (HTTP headers / 304) | Batch B `preview_etag_revalidation_returns_304`. |
| §2 Decision 11 (error → status mapping) | Batch B `preview_unsupported_mime_returns_415` + `preview_size_too_large_returns_400` + `preview_unknown_fileid_returns_404`. |
| §2 Decision 12 (UI inline thumbs) | Batch D Tasks D1 + D2 + D3. |
| §3 architecture | Module structure under `crates/crabcloud-preview/` + handler paths. |
| §3.1-3.5 data flows | E2E tests across both surfaces. |
| §4 testing strategy | Mapped above. |
| §5 risks | Mitigations baked into implementation; no separate task. |
