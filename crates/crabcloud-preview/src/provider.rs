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
/// 415. Mime matching is case-insensitive and ignores parameters (e.g.
/// `image/jpeg; charset=binary` matches `image/jpeg`).
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
            assert!(
                provider_for_mime(mime).is_none(),
                "{mime} should not dispatch"
            );
        }
    }
}
