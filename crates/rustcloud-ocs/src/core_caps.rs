//! Built-in `core` namespace capabilities. Matches Nextcloud's shape.
//!
//! Spec §9.3.

use crate::capabilities::{CapabilityContext, CapabilityProvider};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct CoreCapabilities {
    /// In seconds. Nextcloud default is 60.
    pub poll_interval: u32,
    /// Sub-path under `/remote.php` where DAV lives. Default: `"remote.php/dav"`.
    pub webdav_root: String,
    /// Whether mod_rewrite (or equivalent) is configured. True for axum-direct.
    pub mod_rewrite_working: bool,
    /// Reference time bucket size in ms.
    pub reference_time_offset_ms: i64,
}

impl Default for CoreCapabilities {
    fn default() -> Self {
        Self {
            poll_interval: 60,
            webdav_root: "remote.php/dav".into(),
            mod_rewrite_working: true,
            reference_time_offset_ms: 0,
        }
    }
}

#[async_trait]
impl CapabilityProvider for CoreCapabilities {
    fn namespace(&self) -> &'static str {
        "core"
    }

    fn contribute(&self, _ctx: &CapabilityContext<'_>) -> Value {
        json!({
            "pollinterval": self.poll_interval,
            "webdav-root": self.webdav_root,
            "mod-rewrite-working": self.mod_rewrite_working,
            "reference-time": self.reference_time_offset_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{aggregate, CapabilityProvider};
    use rustcloud_cache::{Cache, MemoryCache};
    use std::sync::Arc;

    #[test]
    fn default_values_match_nextcloud_shape() {
        let core = CoreCapabilities::default();
        let v = core.contribute(&CapabilityContext::default());
        assert_eq!(v["pollinterval"], 60);
        assert_eq!(v["webdav-root"], "remote.php/dav");
        assert_eq!(v["mod-rewrite-working"], true);
    }

    #[test]
    fn custom_values_flow_through() {
        let core = CoreCapabilities {
            poll_interval: 30,
            webdav_root: "ocs/v2.php/dav".into(),
            mod_rewrite_working: false,
            reference_time_offset_ms: 1000,
        };
        let v = core.contribute(&CapabilityContext::default());
        assert_eq!(v["pollinterval"], 30);
        assert_eq!(v["webdav-root"], "ocs/v2.php/dav");
        assert_eq!(v["mod-rewrite-working"], false);
        assert_eq!(v["reference-time"], 1000);
    }

    #[tokio::test]
    async fn aggregator_includes_core_namespace() {
        let providers: Vec<Arc<dyn CapabilityProvider>> =
            vec![Arc::new(CoreCapabilities::default())];
        let ctx = CapabilityContext::default();
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        let p = aggregate(&providers, &ctx, cache, "31.0.0", "inst1")
            .await
            .unwrap();
        assert_eq!(p.body["capabilities"]["core"]["pollinterval"], 60);
    }
}
