// Implemented in Task 10.

use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CapabilityContext<'a> {
    pub locale: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("placeholder")]
    Placeholder,
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    fn namespace(&self) -> &'static str;
    fn contribute(&self, ctx: &CapabilityContext<'_>) -> serde_json::Value;
}

#[derive(Debug)]
pub struct CapabilitiesPayload {
    pub etag: String,
    pub body: serde_json::Value,
}

pub async fn aggregate(
    _providers: &[Arc<dyn CapabilityProvider>],
    _ctx: &CapabilityContext<'_>,
    _cache: Arc<dyn rustcloud_cache::Cache>,
    _version: &str,
    _instance_id: &str,
) -> Result<CapabilitiesPayload, CapabilityError> {
    todo!("implemented in Task 10")
}
