//! `View` — per-user filesystem façade. Implementation lands in Batches B + C.

use crate::error::FsResult;
use crate::mount::Mount;
use crabcloud_filecache::FileCache;
use crabcloud_storage::ChannelEventSink;
use crabcloud_users::UserId;
use std::sync::Arc;

#[allow(dead_code)] // fields wired up in Batches B/C
pub struct View {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) filecache: Arc<FileCache>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
    ) -> Self {
        Self {
            uid,
            mounts,
            filecache,
            storage_sink,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }
}

// Operations land in Batch B (reads) and Batch C (rename/copy). Marker
// import to keep `FsResult` in scope without warnings.
#[allow(dead_code)]
fn _typecheck() -> FsResult<()> {
    Ok(())
}
