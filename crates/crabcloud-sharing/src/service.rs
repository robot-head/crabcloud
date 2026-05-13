//! `Shares` — sharing CRUD service. CRUD implementations land in Batch B.
//!
//! Note on type names: the user-facing crate names in this workspace are
//! `crabcloud_users::UsersService` and `crabcloud_filecache::FileCache` (not
//! `Users` / `Filecache` as referenced in some earlier design notes). The
//! `Shares` struct holds `Arc`s to those services so the OCS handler layer
//! can compose them at request time.

use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_users::UsersService;
use std::sync::Arc;

#[derive(Clone)]
#[allow(dead_code)] // Fields populated and read by CRUD impls in Batch B.
pub struct Shares {
    pub(crate) pool: Arc<DbPool>,
    pub(crate) users: Arc<UsersService>,
    pub(crate) filecache: Arc<FileCache>,
}

impl Shares {
    pub fn new(pool: Arc<DbPool>, users: Arc<UsersService>, filecache: Arc<FileCache>) -> Self {
        Self {
            pool,
            users,
            filecache,
        }
    }
}
