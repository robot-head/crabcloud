//! Placeholder — replaced in Task A6.

use crate::error::ZipError;
use crate::types::{ZipCaps, ZipSummary};
use crabcloud_fs::path::UserPath;
use crabcloud_fs::View;
use tokio::io::AsyncWrite;

pub async fn stream_folder<W>(
    _view: &View,
    _root: &UserPath,
    _caps: ZipCaps,
    _sink: W,
) -> Result<ZipSummary, ZipError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    Ok(ZipSummary {
        entries: 0,
        bytes: 0,
    })
}
