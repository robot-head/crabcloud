//! PDF source provider via hayro 0.7 (pure-Rust PDF renderer). Renders
//! page 0 at a scale that maps the longest edge to the target size, then
//! funnels the result through JPEG encode (q80) so the output matches
//! `ImageProvider`'s shape byte-for-byte.
//!
//! Hayro 0.7 API used here:
//!   - `hayro::hayro_syntax::Pdf::new(bytes)` returns
//!     `Result<Pdf, LoadPdfError>` (rejects garbage input).
//!   - `pdf.pages().iter().next()` exposes the first page.
//!   - `page.render_dimensions() -> (f32, f32)` gives the rendered size in
//!     pixels at scale=1 (i.e. unscaled).
//!   - `hayro::render(&page, &RenderCache, &InterpreterSettings,
//!     &RenderSettings) -> Pixmap` is the entry point (NOT a method on
//!     `Page` as the original plan sketch assumed).
//!   - `Pixmap::data_as_u8_slice()` is the premultiplied RGBA8 buffer; we
//!     unpremultiply and feed it into the `image` crate for resize+encode.

use crate::error::PreviewError;
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::{render, RenderCache, RenderSettings};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbaImage};

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
        tokio::task::spawn_blocking(move || render_blocking(source_bytes, size_px, max_pixels))
            .await
            .map_err(|e| PreviewError::PdfRender(format!("provider task join: {e}")))?
    }
}

fn render_blocking(
    source_bytes: Vec<u8>,
    size_px: u32,
    max_pixels: u32,
) -> ProviderResult<Vec<u8>> {
    // 1. Load the PDF. `Pdf::new` rejects garbage with `LoadPdfError`.
    //    `LoadPdfError` doesn't implement `Display` cleanly in 0.7, so we
    //    swallow the inner detail.
    let pdf = Pdf::new(source_bytes).map_err(|_| {
        PreviewError::PdfRender("failed to load PDF (malformed or unsupported)".to_string())
    })?;
    // 2. First page (deferring later-page selection to a future SP).
    let pages = pdf.pages();
    let page = pages
        .iter()
        .next()
        .ok_or_else(|| PreviewError::PdfRender("PDF has no pages".to_string()))?;
    // 3. Pick a scale so the longest edge maps to size_px. hayro's
    //    `render_dimensions()` returns the unscaled rendering size in
    //    "pixels" at scale=1.
    let (w_f, h_f) = page.render_dimensions();
    let max_edge = w_f.max(h_f);
    if max_edge <= 0.0 || !max_edge.is_finite() {
        return Err(PreviewError::PdfRender("page has zero size".to_string()));
    }
    let scale = (size_px as f32) / max_edge;
    let scaled_w = ((w_f * scale).round() as u32).max(1);
    let scaled_h = ((h_f * scale).round() as u32).max(1);
    if scaled_w.saturating_mul(scaled_h) > max_pixels {
        return Err(PreviewError::SourceTooLarge {
            width: scaled_w,
            height: scaled_h,
            max: max_pixels,
        });
    }
    // 4. Render.
    let cache = RenderCache::new();
    let interp = InterpreterSettings::default();
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: None,
        height: None,
        ..Default::default()
    };
    let pixmap = render(page, &cache, &interp, &settings);
    let pm_w = pixmap.width() as u32;
    let pm_h = pixmap.height() as u32;
    // 5. Pixmap stores premultiplied RGBA8. Take the unpremultiplied form
    //    so the `image` crate sees standard alpha, then build an
    //    `image::RgbaImage`.
    let unpremul = pixmap.take_unpremultiplied();
    let mut raw = Vec::with_capacity(unpremul.len() * 4);
    for px in &unpremul {
        raw.push(px.r);
        raw.push(px.g);
        raw.push(px.b);
        raw.push(px.a);
    }
    let rgba = RgbaImage::from_raw(pm_w, pm_h, raw).ok_or_else(|| {
        PreviewError::PdfRender(format!(
            "pixmap buffer/dim mismatch ({pm_w}x{pm_h}, {} pixels)",
            unpremul.len()
        ))
    })?;
    let dynimg = DynamicImage::ImageRgba8(rgba);
    // 6. Exact long-edge fit (hayro's scale step rounds; this corrects).
    let resized = dynimg.thumbnail(size_px, size_px);
    // 7. JPEG q80 — flatten alpha to RGB.
    let rgb = resized.to_rgb8();
    let mut out = Vec::with_capacity(64 * 1024);
    let mut encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    encoder
        .encode_image(&rgb)
        .map_err(|e| PreviewError::Encode(format!("jpeg: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid 1-page PDF with a 100x100 MediaBox and an empty
    /// content stream. Verified to load through `hayro 0.7` in the
    /// implementation probe.
    fn synthesize_one_page_pdf() -> Vec<u8> {
        const PDF: &[u8] = b"%PDF-1.4\n\
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
        // For a square 100x100 page → exactly 256x256 at size_px=256.
        assert!(img.width() == 256 || img.height() == 256);
    }

    #[tokio::test]
    async fn rejects_garbage_pdf() {
        let r = PdfProvider
            .render(b"not a pdf at all".to_vec(), 64, 64 * 1024 * 1024)
            .await;
        assert!(matches!(r, Err(PreviewError::PdfRender(_))), "got {r:?}");
    }

    #[tokio::test]
    async fn rejects_scaled_above_max_pixels() {
        let bytes = synthesize_one_page_pdf();
        // Tiny cap forces SourceTooLarge before the render even starts.
        let r = PdfProvider.render(bytes, 1024, 100).await;
        match r {
            Err(PreviewError::SourceTooLarge { .. }) => {}
            other => panic!("expected SourceTooLarge, got {other:?}"),
        }
    }
}
