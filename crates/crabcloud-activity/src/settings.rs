//! ActivitySettings — per-user-per-event stream toggles. Filled in Task A4.

use crabcloud_db::DbPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct ActivitySettings {
    #[allow(dead_code)]
    pool: Arc<DbPool>,
}

impl ActivitySettings {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }
}
