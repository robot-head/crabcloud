//! Image source provider. Body lands in Task A4; this commit only
//! materializes the struct so `provider_for_mime` can compile.

use crate::provider::{PreviewProvider, ProviderResult};
use async_trait::async_trait;

pub struct ImageProvider;

#[async_trait]
impl PreviewProvider for ImageProvider {
    async fn render(
        &self,
        _source_bytes: Vec<u8>,
        _size_px: u32,
        _max_pixels: u32,
    ) -> ProviderResult<Vec<u8>> {
        unimplemented!("filled in Task A4")
    }
}
