//! Capabilities aggregator. Iterates registered providers, merges JSON, caches.
//!
//! See spec §9.3.

use async_trait::async_trait;
use rustcloud_cache::{Cache, CacheError};
use serde_json::{json, Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

/// Per-request context passed to `CapabilityProvider::contribute`.
/// Lightweight (no `AppState` reference) so providers don't accidentally couple
/// to the wider state machinery.
#[derive(Debug, Default, Clone)]
pub struct CapabilityContext<'a> {
    pub locale: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    /// The top-level key under `ocs.data.capabilities` this provider contributes to.
    fn namespace(&self) -> &'static str;

    /// Return a JSON value (usually an object) to merge under `namespace()`.
    fn contribute(&self, ctx: &CapabilityContext<'_>) -> Value;
}

/// The aggregated payload returned to clients.
#[derive(Debug, Clone)]
pub struct CapabilitiesPayload {
    pub etag: String,
    pub body: Value,
}

/// Run the aggregator. Cache key includes locale + user_id so personalized
/// responses don't bleed across users.
pub async fn aggregate(
    providers: &[Arc<dyn CapabilityProvider>],
    ctx: &CapabilityContext<'_>,
    cache: Arc<dyn Cache>,
    version: &str,
    instance_id: &str,
) -> Result<CapabilitiesPayload, CapabilityError> {
    let cache_key = format!(
        "{instance_id}:caps:{}:{}",
        ctx.locale.unwrap_or(""),
        ctx.user_id.unwrap_or("")
    );

    if let Some(raw) = cache.get(&cache_key).await? {
        if let Ok(payload) = serde_json::from_slice::<CachedPayload>(&raw) {
            return Ok(CapabilitiesPayload {
                etag: payload.etag,
                body: payload.body,
            });
        }
    }

    let mut caps = Map::new();
    for p in providers {
        caps.insert(p.namespace().to_string(), p.contribute(ctx));
    }

    let body = json!({
        "version": {
            "major": 31,
            "minor": 0,
            "micro": 0,
            "string": version,
            "edition": ""
        },
        "capabilities": Value::Object(caps),
    });

    let etag = compute_etag(version, instance_id, providers, ctx);
    let cached = CachedPayload {
        etag: etag.clone(),
        body: body.clone(),
    };
    let serialized = serde_json::to_vec(&cached)?;
    let _ = cache
        .set(&cache_key, &serialized, Some(Duration::from_secs(60)))
        .await;

    Ok(CapabilitiesPayload { etag, body })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedPayload {
    etag: String,
    body: Value,
}

fn compute_etag(
    version: &str,
    instance_id: &str,
    providers: &[Arc<dyn CapabilityProvider>],
    ctx: &CapabilityContext<'_>,
) -> String {
    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    instance_id.hash(&mut hasher);
    for p in providers {
        p.namespace().hash(&mut hasher);
    }
    ctx.locale.unwrap_or("").hash(&mut hasher);
    ctx.user_id.unwrap_or("").hash(&mut hasher);
    format!("W/\"{:x}\"", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;
    use serde_json::json;

    struct FakeProvider {
        ns: &'static str,
        body: Value,
    }

    #[async_trait]
    impl CapabilityProvider for FakeProvider {
        fn namespace(&self) -> &'static str {
            self.ns
        }
        fn contribute(&self, _ctx: &CapabilityContext<'_>) -> Value {
            self.body.clone()
        }
    }

    fn cache() -> Arc<dyn Cache> {
        Arc::new(MemoryCache::new())
    }

    #[tokio::test]
    async fn merges_providers_under_their_namespaces() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![
            Arc::new(FakeProvider {
                ns: "core",
                body: json!({"pollinterval": 60}),
            }),
            Arc::new(FakeProvider {
                ns: "files",
                body: json!({"versioning": true}),
            }),
        ];
        let ctx = CapabilityContext::default();
        let payload = aggregate(&providers, &ctx, cache(), "31.0.0", "inst1")
            .await
            .unwrap();
        assert_eq!(payload.body["capabilities"]["core"]["pollinterval"], 60);
        assert_eq!(payload.body["capabilities"]["files"]["versioning"], true);
        assert_eq!(payload.body["version"]["string"], "31.0.0");
    }

    #[tokio::test]
    async fn etag_changes_with_version() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let ctx = CapabilityContext::default();
        let c = cache();
        let a = aggregate(&providers, &ctx, c.clone(), "31.0.0", "inst1")
            .await
            .unwrap();
        // Clear cache so we compute fresh.
        c.del("inst1:caps::").await.unwrap();
        let b = aggregate(&providers, &ctx, c.clone(), "31.0.1", "inst1")
            .await
            .unwrap();
        assert_ne!(a.etag, b.etag);
    }

    #[tokio::test]
    async fn etag_separates_users() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let c = cache();
        let alice_ctx = CapabilityContext {
            locale: Some("en"),
            user_id: Some("alice"),
        };
        let bob_ctx = CapabilityContext {
            locale: Some("en"),
            user_id: Some("bob"),
        };
        let a = aggregate(&providers, &alice_ctx, c.clone(), "31", "inst1")
            .await
            .unwrap();
        let b = aggregate(&providers, &bob_ctx, c.clone(), "31", "inst1")
            .await
            .unwrap();
        assert_ne!(a.etag, b.etag);
    }

    #[tokio::test]
    async fn second_call_hits_cache() {
        // Verify by checking that a cache key was written after the first call.
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let ctx = CapabilityContext::default();
        let c = cache();
        aggregate(&providers, &ctx, c.clone(), "31", "inst1")
            .await
            .unwrap();
        let key = "inst1:caps::";
        assert!(
            c.get(key).await.unwrap().is_some(),
            "cache should contain aggregated payload"
        );

        // Second call should produce identical etag (cache hit).
        let p2 = aggregate(&providers, &ctx, c.clone(), "31", "inst1")
            .await
            .unwrap();
        let first_etag = {
            let raw = c.get(key).await.unwrap().unwrap();
            let cached: CachedPayload = serde_json::from_slice(&raw).unwrap();
            cached.etag
        };
        assert_eq!(p2.etag, first_etag);
    }
}
