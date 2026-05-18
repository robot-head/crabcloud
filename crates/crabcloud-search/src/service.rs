//! Search service. Filled in Task A5.

use crate::error::SearchError;
use crate::types::{SearchHit, SearchQuery};
use async_trait::async_trait;
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_users::UserId;
use std::sync::Arc;

#[derive(Clone)]
pub struct Search {
    #[allow(dead_code)]
    pool: Arc<DbPool>,
}

impl Search {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    pub async fn query(
        &self,
        _viewer_uid: &str,
        _q: &SearchQuery,
        _limit: i64,
        _cursor: Option<(f64, i64)>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        Ok(Vec::new())
    }
}

/// Trait that `crabcloud-sharing` depends on so it can drive bulk
/// fan-out at share lifecycle events without taking a hard dep on
/// `crabcloud-search`. `Search` itself impls this; tests can use
/// [`NoopSearchFanout`].
///
/// The `filecache` reference is passed in so the trait can avoid
/// taking an `Arc<FileCache>` field on every implementor — the share
/// service already holds one and threads it through.
#[async_trait]
pub trait SearchFanout: Send + Sync {
    /// Walk every fileid under `owner_subroot_path` in `owner_uid`'s
    /// home storage and UPSERT one row per recipient with the
    /// share-mount-translated path.
    async fn fan_out_for_share(
        &self,
        filecache: &FileCache,
        recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
        recipient_path_prefix: &str,
    ) -> Result<(), SearchError>;

    /// Inverse: walk the same subroot and DELETE per-(recipient, fileid).
    async fn fan_out_for_unshare(
        &self,
        filecache: &FileCache,
        former_recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
    ) -> Result<(), SearchError>;
}

#[async_trait]
impl SearchFanout for Search {
    async fn fan_out_for_share(
        &self,
        _filecache: &FileCache,
        _recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
        _recipient_path_prefix: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }

    async fn fan_out_for_unshare(
        &self,
        _filecache: &FileCache,
        _former_recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }
}

/// No-op implementation. Tests / fixtures that don't need search
/// fan-out can pass `Arc::new(NoopSearchFanout)` into `SharesConfig`.
pub struct NoopSearchFanout;

#[async_trait]
impl SearchFanout for NoopSearchFanout {
    async fn fan_out_for_share(
        &self,
        _filecache: &FileCache,
        _recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
        _recipient_path_prefix: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }

    async fn fan_out_for_unshare(
        &self,
        _filecache: &FileCache,
        _former_recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }
}
