//! `SessionLayer` — thin middleware that owns cookie sign/verify and the
//! per-session ephemeral state (CSRF token, two_factor_passed). Auth itself
//! is handled upstream by `AuthLayer`; this layer reads the AuthContext to
//! decide whether to load / write ephemeral state.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::session::cookie::encode_cookie;
use crate::session::data::Session;
use crate::session::store::SessionStore;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use futures::future::BoxFuture;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::{Layer, Service};

/// Name of the session cookie (Nextcloud-compatible).
pub const COOKIE_NAME: &str = "oc_sessionPassphrase";

/// Pending-cookie action: a handler can stash a [`PendingCookie::Set`] to mint
/// a fresh cookie post-response, or [`PendingCookie::Destroy`] to clear it.
#[derive(Debug, Clone)]
pub enum PendingCookie {
    /// Mint a fresh cookie carrying `raw_token` as the opaque payload.
    Set {
        /// Plaintext token to embed (HMAC-signed by the layer).
        raw_token: String,
        /// Authoritative `oc_authtoken` row id this cookie binds to. The
        /// SessionLayer saves the ephemeral session blob under this id so it
        /// survives the cookie swap (login mint, post-password-change rotation).
        token_id: i64,
        /// `Max-Age` cookie attribute, in seconds.
        max_age_secs: u64,
    },
    /// Emit a Set-Cookie that clears the cookie on the client.
    Destroy,
}

/// Wrapper inserted into request extensions so handlers can mutate the
/// ephemeral session blob and request cookie writes.
#[derive(Clone)]
pub struct SessionHandle {
    /// The authoritative `oc_authtoken` row id, when the request was
    /// authenticated via session cookie. `None` for anonymous / header-auth
    /// requests.
    pub token_id: Option<i64>,
    /// Mutable session blob guarded by an async mutex.
    pub inner: Arc<Mutex<Session>>,
    /// Pending cookie mutation to apply on response.
    pub pending_cookie: Arc<Mutex<Option<PendingCookie>>>,
}

impl SessionHandle {
    /// Read a snapshot of the current session state.
    pub async fn read(&self) -> Session {
        self.inner.lock().await.clone()
    }
    /// Best-effort sync snapshot used by SSR rendering (which can't `.await`).
    /// Returns `None` if the lock is currently held; callers should treat
    /// that as "session not available this frame" rather than retrying.
    pub fn try_read_snapshot(&self) -> Option<Session> {
        self.inner.try_lock().ok().map(|g| g.clone())
    }
    /// Mutate the session under the lock; changes are persisted by the layer
    /// when the response is flushed.
    pub async fn mutate<F: FnOnce(&mut Session)>(&self, f: F) {
        let mut s = self.inner.lock().await;
        f(&mut s);
    }
    /// Stash a pending cookie mutation. The layer will apply it on response.
    pub async fn set_pending_cookie(&self, p: PendingCookie) {
        *self.pending_cookie.lock().await = Some(p);
    }
}

/// `tower::Layer` that loads the ephemeral session blob keyed by the
/// AuthContext's `token_id`, makes it available via [`SessionHandle`]
/// extension, and writes back any changes (and pending cookie mutations)
/// on response.
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

fn make_set_cookie(value: &str, secure: bool, max_age: u64) -> HeaderValue {
    let mut s = format!("{COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie attrs are ascii")
}

fn make_destroy_cookie(secure: bool) -> HeaderValue {
    let mut s = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
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
            // Token id from AuthLayer's context. Cookie-auth has one; non-
            // Session auth (Bearer/Basic) also has one but we don't load
            // ephemeral state for those (they're stateless headers).
            let token_id_opt = req
                .extensions()
                .get::<AuthContext>()
                .filter(|c| c.method == AuthMethod::Session)
                .map(|c| c.token_id);

            // Load the session blob for this token (or start fresh).
            let session = match token_id_opt {
                Some(id) => store
                    .load_for_token(id)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                None => Session::new(),
            };

            let handle = SessionHandle {
                token_id: token_id_opt,
                inner: Arc::new(Mutex::new(session)),
                pending_cookie: Arc::new(Mutex::new(None)),
            };
            req.extensions_mut().insert(handle.clone());

            let mut resp = inner.call(req).await?;

            // Determine where to save the blob: a freshly-set pending cookie
            // changes the canonical token_id (login mint, post-password-change
            // rotation). Without this, the blob would be saved under the OLD
            // (or None) token id and lost across the cookie swap — which would
            // produce an empty csrf_token on the next request and silently
            // bypass CSRF.
            let pending = handle.pending_cookie.lock().await.clone();
            let save_id = match &pending {
                Some(PendingCookie::Set { token_id, .. }) => Some(*token_id),
                Some(PendingCookie::Destroy) => None,
                None => token_id_opt,
            };

            if let Some(id) = save_id {
                let final_session = handle.inner.lock().await.clone();
                let _ = store.save_for_token(id, &final_session).await;
            }

            // Apply pending cookie mutation.
            if let Some(pending) = pending {
                match pending {
                    PendingCookie::Set {
                        raw_token,
                        max_age_secs,
                        ..
                    } => {
                        let value = encode_cookie(&raw_token, secret.expose_secret().as_bytes());
                        resp.headers_mut()
                            .append(SET_COOKIE, make_set_cookie(&value, secure, max_age_secs));
                    }
                    PendingCookie::Destroy => {
                        if let Some(id) = token_id_opt {
                            let _ = store.destroy_for_token(id).await;
                        }
                        resp.headers_mut()
                            .append(SET_COOKIE, make_destroy_cookie(secure));
                    }
                }
            }

            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::SessionStore;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use crabcloud_cache::MemoryCache;
    use tower::ServiceExt;

    async fn no_op_handler(
        axum::Extension(_handle): axum::Extension<SessionHandle>,
    ) -> &'static str {
        "ok"
    }

    async fn set_cookie_handler(
        axum::Extension(handle): axum::Extension<SessionHandle>,
    ) -> &'static str {
        handle
            .set_pending_cookie(PendingCookie::Set {
                raw_token: "the-raw-token".into(),
                token_id: 7,
                max_age_secs: 1800,
            })
            .await;
        "ok"
    }

    fn app() -> Router {
        let cache: Arc<dyn crabcloud_cache::Cache> = Arc::new(MemoryCache::new());
        let store = SessionStore::new(cache, "inst1");
        let layer = SessionLayer::new(store, SecretString::new("secret".into()), false);
        Router::new()
            .route("/noop", get(no_op_handler))
            .route("/login", get(set_cookie_handler))
            .layer(layer)
    }

    #[tokio::test]
    async fn no_pending_cookie_means_no_set_cookie_header() {
        let req = Request::builder().uri("/noop").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn pending_cookie_set_emits_set_cookie() {
        let req = Request::builder()
            .uri("/login")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let setc = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
        assert!(setc.starts_with(&format!("{COOKIE_NAME}=")));
        assert!(setc.contains("HttpOnly"));
        assert!(setc.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn pending_cookie_set_saves_blob_under_new_token_id() {
        let cache: Arc<dyn crabcloud_cache::Cache> = Arc::new(MemoryCache::new());
        let store = SessionStore::new(cache.clone(), "inst1");
        let layer = SessionLayer::new(store, SecretString::new("secret".into()), false);

        async fn mint_session(
            axum::Extension(handle): axum::Extension<SessionHandle>,
        ) -> &'static str {
            handle
                .mutate(|s| {
                    s.csrf_token = "abc".into();
                })
                .await;
            handle
                .set_pending_cookie(PendingCookie::Set {
                    raw_token: "raw".into(),
                    token_id: 42,
                    max_age_secs: 1800,
                })
                .await;
            "ok"
        }

        let app = Router::new().route("/mint", get(mint_session)).layer(layer);
        let req = Request::builder().uri("/mint").body(Body::empty()).unwrap();
        let _resp = app.oneshot(req).await.unwrap();

        // Blob should have been saved under token_id=42, even though the
        // request had no AuthContext (token_id_opt was None).
        let key = "inst1:session_blob:42";
        let bytes = cache.get(key).await.unwrap().expect("blob missing");
        let s: Session = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s.csrf_token, "abc");
    }
}
