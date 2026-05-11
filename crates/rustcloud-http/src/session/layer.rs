//! `SessionLayer` — Tower middleware that loads the session from the cookie
//! into a request extension, then writes it back on response.
//!
//! Cookie name: `oc_sessionPassphrase` (Nextcloud-compatible).

use crate::session::cookie::{decode_cookie, encode_cookie};
use crate::session::data::{Session, SessionId};
use crate::session::store::SessionStore;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use futures::future::BoxFuture;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::{Layer, Service};

/// Name of the session cookie (Nextcloud-compatible).
pub const COOKIE_NAME: &str = "oc_sessionPassphrase";

/// Wrapper inserted into request extensions so handlers can mutate the session.
#[derive(Clone)]
pub struct SessionHandle {
    /// Session id (matches the cookie value).
    pub id: SessionId,
    /// Mutable session payload guarded by an async mutex.
    pub inner: Arc<Mutex<Session>>,
    /// Set to true when the handler wants the session destroyed on response.
    pub destroy: Arc<Mutex<bool>>,
}

impl SessionHandle {
    /// Read a snapshot of the current session state.
    pub async fn read(&self) -> Session {
        self.inner.lock().await.clone()
    }
    /// Mutate the session under the lock; changes are persisted by the layer
    /// when the response is flushed.
    pub async fn mutate<F: FnOnce(&mut Session)>(&self, f: F) {
        let mut s = self.inner.lock().await;
        f(&mut s);
    }
    /// Mark the session for destruction. The layer will purge the cache entry
    /// and emit a clearing `Set-Cookie` on response.
    pub async fn destroy(&self) {
        *self.destroy.lock().await = true;
    }
}

/// `tower::Layer` that loads the session referenced by the request cookie,
/// makes it available via [`SessionHandle`] extension, and writes back any
/// changes on response.
#[derive(Clone)]
pub struct SessionLayer {
    store: SessionStore,
    secret: Arc<SecretString>,
    secure: bool,
}

impl SessionLayer {
    /// Build the layer from a store, signing `secret`, and whether to set the
    /// `Secure` cookie flag (true in production / behind HTTPS).
    pub fn new(store: SessionStore, secret: SecretString, secure: bool) -> Self {
        Self {
            store,
            secret: Arc::new(secret),
            secure,
        }
    }
}

impl<S> Layer<S> for SessionLayer {
    type Service = SessionMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        SessionMiddleware {
            inner,
            store: self.store.clone(),
            secret: self.secret.clone(),
            secure: self.secure,
        }
    }
}

/// Middleware service produced by [`SessionLayer`].
#[derive(Clone)]
pub struct SessionMiddleware<S> {
    inner: S,
    store: SessionStore,
    secret: Arc<SecretString>,
    secure: bool,
}

fn extract_cookie(req: &Request<impl Send>, name: &str) -> Option<String> {
    let raw = req.headers().get(COOKIE)?.to_str().ok()?;
    for piece in raw.split(';').map(str::trim) {
        if let Some((k, v)) = piece.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn make_set_cookie(value: &str, secure: bool, max_age: u64) -> HeaderValue {
    let mut s = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        COOKIE_NAME, value, max_age
    );
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

fn make_destroy_cookie(secure: bool) -> HeaderValue {
    let mut s = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        COOKIE_NAME
    );
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

impl<S, B> Service<Request<B>> for SessionMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let store = self.store.clone();
        let secret = self.secret.clone();
        let secure = self.secure;
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // 1. Resolve session ID from cookie, or mint a new one.
            let (id, mut session) = match extract_cookie(&req, COOKIE_NAME) {
                Some(raw) => match decode_cookie(&raw, secret.expose_secret().as_bytes()) {
                    Ok(id_hex) => {
                        let id = SessionId(id_hex);
                        let loaded = store.load(&id).await.ok().flatten();
                        match loaded {
                            Some(s) => (id, s),
                            None => (SessionId::new_random(), Session::new()),
                        }
                    }
                    Err(_) => (SessionId::new_random(), Session::new()),
                },
                None => (SessionId::new_random(), Session::new()),
            };

            // 2. Slide TTL (touch last_activity).
            session.last_activity = now_secs();

            // 3. Insert handle into request extensions.
            let handle = SessionHandle {
                id: id.clone(),
                inner: Arc::new(Mutex::new(session)),
                destroy: Arc::new(Mutex::new(false)),
            };
            req.extensions_mut().insert(handle.clone());

            // 4. Run inner service.
            let mut resp = inner.call(req).await?;

            // 5. Save or destroy session as the handler indicated.
            let destroy = *handle.destroy.lock().await;
            if destroy {
                let _ = store.destroy(&handle.id).await;
                resp.headers_mut()
                    .append(SET_COOKIE, make_destroy_cookie(secure));
            } else {
                let final_session = handle.inner.lock().await.clone();
                let _ = store.save(&handle.id, &final_session).await;
                let cookie_value =
                    encode_cookie(handle.id.as_str(), secret.expose_secret().as_bytes());
                resp.headers_mut().append(
                    SET_COOKIE,
                    make_set_cookie(
                        &cookie_value,
                        secure,
                        super::store::SESSION_IDLE_TTL.as_secs(),
                    ),
                );
            }

            Ok(resp)
        })
    }
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::SessionStore;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use rustcloud_cache::MemoryCache;
    use tower::ServiceExt;

    async fn login_handler(
        axum::Extension(handle): axum::Extension<SessionHandle>,
    ) -> &'static str {
        handle.mutate(|s| s.user_id = Some("alice".into())).await;
        "ok"
    }

    async fn whoami(axum::Extension(handle): axum::Extension<SessionHandle>) -> String {
        handle.read().await.user_id.unwrap_or_default()
    }

    fn app() -> (Router, Arc<dyn rustcloud_cache::Cache>) {
        let cache: Arc<dyn rustcloud_cache::Cache> = Arc::new(MemoryCache::new());
        let store = SessionStore::new(cache.clone(), "inst1");
        let layer = SessionLayer::new(store, SecretString::new("secret".into()), false);
        let app = Router::new()
            .route("/login", get(login_handler))
            .route("/whoami", get(whoami))
            .layer(layer);
        (app, cache)
    }

    #[tokio::test]
    async fn login_sets_session_cookie() {
        let (app, _) = app();
        let req = Request::builder()
            .uri("/login")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let setc = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
        assert!(setc.starts_with("oc_sessionPassphrase="));
        assert!(setc.contains("HttpOnly"));
        assert!(setc.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn round_trip_session_via_cookie() {
        let (app, _) = app();
        // 1st request: login.
        let req1 = Request::builder()
            .uri("/login")
            .body(Body::empty())
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        let setc = resp1
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let cookie = setc.split(';').next().unwrap().to_string();
        // 2nd request: whoami with the cookie.
        let req2 = Request::builder()
            .uri("/whoami")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let body = axum::body::to_bytes(resp2.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "alice");
    }
}
