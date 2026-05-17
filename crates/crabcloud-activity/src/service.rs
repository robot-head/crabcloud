//! Activity service. Filled in Task A4.

use crate::emitter::ActivityEmitter;
use crate::error::{ActivityEmitError, ActivityError};
use crate::settings::ActivitySettings;
use crate::types::{ActivityEvent, ActivityRow};
use crabcloud_db::DbPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct Activity {
    #[allow(dead_code)]
    pool: Arc<DbPool>,
    #[allow(dead_code)]
    settings: ActivitySettings,
    #[allow(dead_code)]
    coalesce_window_secs: i64,
}

impl Activity {
    pub fn new(pool: Arc<DbPool>, settings: ActivitySettings, coalesce_window_secs: i64) -> Self {
        Self {
            pool,
            settings,
            coalesce_window_secs,
        }
    }

    pub async fn list(
        &self,
        _affected_user: &str,
        _since: Option<i64>,
        _limit: i64,
    ) -> Result<Vec<ActivityRow>, ActivityError> {
        Ok(Vec::new())
    }

    pub async fn sweep_expired(&self, _cutoff: i64) -> Result<u64, ActivityError> {
        Ok(0)
    }
}

#[async_trait::async_trait]
impl ActivityEmitter for Activity {
    async fn emit(&self, _event: ActivityEvent) -> Result<(), ActivityEmitError> {
        Ok(())
    }
}
