//! Image source provider. Decodes JPEG/PNG/GIF/WebP, resizes to fit the
//! target longest-edge, encodes the result as JPEG (quality 80).

use crate::error::PreviewError;
use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;
use image::codecs::jpeg::JpegEncoder;
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
        tokio::task::spawn_blocking(move || render_blocking(&source_bytes, size_px, max_pixels))
            .await
            .map_err(|e| PreviewError::Decode(format!("image provider task join: {e}")))?
    }
}

fn render_blocking(source_bytes: &[u8], size_px: u32, max_pixels: u32) -> ProviderResult<Vec<u8>> {
    // 1. Peek dimensions via a throwaway reader before the full decode so
    //    we can reject sources that would blow up memory. `ImageReader`
    //    in `image = "0.25"` is not `Clone`, so we build two readers.
    let dim_reader = ImageReader::new(Cursor::new(source_bytes))
        .with_guessed_format()
        .map_err(|e| PreviewError::Decode(format!("format guess: {e}")))?;
    let (w, h) = dim_reader
        .into_dimensions()
        .map_err(|e| PreviewError::Decode(format!("dimensions: {e}")))?;
    if w.saturating_mul(h) > max_pixels {
        return Err(PreviewError::SourceTooLarge {
            width: w,
            height: h,
            max: max_pixels,
        });
    }
    // 2. Full decode and resize so the longest edge equals size_px.
    //    `thumbnail` uses Lanczos3; switch to `Triangle` (resize_exact) if
    //    profiling shows it's too slow.
    let reader = ImageReader::new(Cursor::new(source_bytes))
        .with_guessed_format()
        .map_err(|e| PreviewError::Decode(format!("format guess: {e}")))?;
    let img = reader
        .decode()
        .map_err(|e| PreviewError::Decode(format!("decode: {e}")))?;
    let thumb = img.thumbnail(size_px, size_px);
    // 4. JPEG q80. Drop alpha by flattening to RGB8 since JPEG can't store
    //    transparency anyway; this also sidesteps `write_with_encoder`
    //    requiring a `Color::Rgb8` path.
    let rgb = thumb.to_rgb8();
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
    use image::codecs::gif::GifEncoder;
    use image::ImageFormat;
    use std::io::Cursor;

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

    fn synthesize_animated_gif() -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = GifEncoder::new(&mut buf);
            for tint in [0u8, 200u8] {
                let frame =
                    image::RgbaImage::from_pixel(64, 32, image::Rgba([tint, tint, tint, 255]));
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
        assert!(img.height() <= 64);
    }

    #[tokio::test]
    async fn strips_animation_to_single_frame() {
        let bytes = synthesize_animated_gif();
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
            Err(PreviewError::SourceTooLarge {
                width: 500,
                height: 500,
                ..
            }) => {}
            other => panic!("expected SourceTooLarge, got {other:?}"),
        }
    }
}
