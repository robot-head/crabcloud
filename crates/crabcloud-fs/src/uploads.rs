//! `Uploads` — chunked upload facade. Implementation lands in Batch D.

use crate::error::FsResult;
use crate::mount::Mount;
use crabcloud_filecache::FileCache;
use crabcloud_storage::ChannelEventSink;
use crabcloud_users::UserId;
use std::sync::Arc;

#[allow(dead_code)] // fields wired up in Batch D
pub struct Uploads {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
    pub(crate) filecache: Arc<FileCache>,
}

impl Uploads {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        storage_sink: Arc<ChannelEventSink>,
        filecache: Arc<FileCache>,
    ) -> Self {
        Self {
            uid,
            mounts,
            storage_sink,
            filecache,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }
}

#[allow(dead_code)]
fn _typecheck() -> FsResult<()> {
    Ok(())
}
