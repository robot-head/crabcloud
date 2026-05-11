// Implemented in Task 11.

use crate::capabilities::{CapabilityContext, CapabilityProvider};
use async_trait::async_trait;

pub struct CoreCapabilities {
    pub webdav_root: String,
    pub poll_interval: u32,
}

#[async_trait]
impl CapabilityProvider for CoreCapabilities {
    fn namespace(&self) -> &'static str {
        "core"
    }
    fn contribute(&self, _ctx: &CapabilityContext<'_>) -> serde_json::Value {
        serde_json::Value::Null
    }
}
