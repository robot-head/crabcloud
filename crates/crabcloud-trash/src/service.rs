//! Placeholder until Task A4 lands the real `Trash` implementation.

use crabcloud_db::DbPool;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct Trash {
    pub(crate) pool: Arc<DbPool>,
    pub(crate) datadir: PathBuf,
}

impl Trash {
    pub fn new(pool: Arc<DbPool>, datadir: PathBuf) -> Self {
        Self { pool, datadir }
    }
}
