//! `SessionStore` — typed wrapper over `Arc<dyn Cache>` for session payloads.

use crate::session::data::{Session, SessionId};
use rustcloud_cache::{Cache, CacheError};
use std::sync::Arc;
use std::time::Duration;

/// Idle TTL for sessions. Spec §7.3 says 30 min idle, 24 h absolute. Phase 3
/// ships the idle-TTL only; absolute-TTL enforcement is a Phase 4 concern.
pub const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct SessionStore {
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl SessionStore {
    pub fn new(cache: Arc<dyn Cache>, instance_id: impl Into<String>) -> Self {
        Self {
            cache,
            instance_id: instance_id.into(),
        }
    }

    fn key(&self, id: &SessionId) -> String {
        format!("{}:session:{}", self.instance_id, id.as_str())
    }

    pub async fn load(&self, id: &SessionId) -> Result<Option<Session>, CacheError> {
        let raw = self.cache.get(&self.key(id)).await?;
        match raw {
            Some(bytes) => {
                let s: Session = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::Io(format!("session decode: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub async fn save(&self, id: &SessionId, session: &Session) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| CacheError::Io(format!("session encode: {e}")))?;
        self.cache
            .set(&self.key(id), &bytes, Some(SESSION_IDLE_TTL))
            .await
    }

    pub async fn destroy(&self, id: &SessionId) -> Result<(), CacheError> {
        self.cache.del(&self.key(id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        let mut s = Session::new();
        s.user_id = Some("alice".into());
        store.save(&id, &s).await.unwrap();
        let loaded = store.load(&id).await.unwrap().unwrap();
        assert_eq!(loaded.user_id.as_deref(), Some("alice"));
        assert_eq!(loaded.csrf_token, s.csrf_token);
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_removes_session() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        store.save(&id, &Session::new()).await.unwrap();
        store.destroy(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }
}
