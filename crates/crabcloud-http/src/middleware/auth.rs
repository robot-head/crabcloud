//! `AuthLayer` — Tower middleware that resolves authentication from one of
//! three arms (Bearer header / Basic header / session cookie) and attaches
//! an [`AuthContext`] to the request's extensions.
//!
//! Precedence (top-down, first hit wins). Header arms fail loud (401 when
//! their token is present but invalid). The cookie arm fails quiet (a
//! malformed / unknown cookie is treated as if no cookie was present, so
//! anonymous routes like `/login` still work after a secret rotation).
//!
//! See `docs/superpowers/specs/2026-05-12-app-passwords-bearer-basic-auth-design.md` §5.1.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::session::{decode_cookie, COOKIE_NAME};
use axum::http::{header::AUTHORIZATION, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as B64STD;
use base64::Engine;
use crabcloud_core::AppState;
use crabcloud_users::AuthTokenType;
use futures::future::BoxFuture;
use secrecy::ExposeSecret;
use std::task::{Context, Poll};
use tower::{Layer, Service};

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

#[derive(Clone)]
pub struct AuthLayer {
    state: AppState,
}

impl AuthLayer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            state: self.state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    state: AppState,
}

#[derive(Debug)]
enum ArmOutcome {
    Authenticated(AuthContext),
    /// The arm's input was present but invalid; respond 401.
    HeaderRejected,
    /// The arm had nothing to offer; continue to the next arm.
    NoInput,
}

impl<S, B> Service<Request<B>> for AuthMiddleware<S>
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
        let state = self.state.clone();
        let mut inner = self.inner.clone();

        // Extract owned candidate inputs from the request synchronously so the
        // async block never borrows `&Request<B>` (which would force
        // `B: Sync` on callers — too restrictive for axum's Body types).
        let auth_header = extract_authorization_header_owned(&req);
        let cookie_value = extract_cookie_value_owned(&req, COOKIE_NAME);

        Box::pin(async move {
            // 1. Bearer
            match try_bearer(&state, auth_header.as_deref()).await {
                ArmOutcome::Authenticated(ctx) => {
                    req.extensions_mut().insert(ctx);
                    return inner.call(req).await;
                }
                ArmOutcome::HeaderRejected => return Ok(unauthorized()),
                ArmOutcome::NoInput => {}
            }
            // 2. Basic
            match try_basic(&state, auth_header.as_deref()).await {
                ArmOutcome::Authenticated(ctx) => {
                    req.extensions_mut().insert(ctx);
                    return inner.call(req).await;
                }
                ArmOutcome::HeaderRejected => return Ok(unauthorized()),
                ArmOutcome::NoInput => {}
            }
            // 3. Cookie (fail quiet)
            if let ArmOutcome::Authenticated(ctx) =
                try_cookie(&state, cookie_value.as_deref()).await
            {
                req.extensions_mut().insert(ctx);
            }
            inner.call(req).await
        })
    }
}

fn extract_authorization_header_owned<B>(req: &Request<B>) -> Option<String> {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

async fn try_bearer(state: &AppState, header: Option<&str>) -> ArmOutcome {
    let header = match header {
        Some(h) if h.starts_with("Bearer ") || h.starts_with("bearer ") => h,
        _ => return ArmOutcome::NoInput,
    };
    let raw = header[7..].trim();
    if raw.is_empty() {
        return ArmOutcome::HeaderRejected;
    }
    match verify_and_build(state, raw, AuthMethod::Bearer, None).await {
        Some(ctx) => ArmOutcome::Authenticated(ctx),
        None => ArmOutcome::HeaderRejected,
    }
}

async fn try_basic(state: &AppState, header: Option<&str>) -> ArmOutcome {
    let header = match header {
        Some(h) if h.starts_with("Basic ") || h.starts_with("basic ") => h,
        _ => return ArmOutcome::NoInput,
    };
    let b64 = header[6..].trim();
    let decoded = match B64STD.decode(b64) {
        Ok(d) => d,
        Err(_) => return ArmOutcome::HeaderRejected,
    };
    let s = match std::str::from_utf8(&decoded) {
        Ok(s) => s,
        Err(_) => return ArmOutcome::HeaderRejected,
    };
    let (uid_str, token) = match s.split_once(':') {
        Some(p) => p,
        None => return ArmOutcome::HeaderRejected,
    };
    if token.is_empty() {
        return ArmOutcome::HeaderRejected;
    }
    match verify_and_build(state, token, AuthMethod::Basic, Some(uid_str)).await {
        Some(ctx) => ArmOutcome::Authenticated(ctx),
        None => ArmOutcome::HeaderRejected,
    }
}

async fn try_cookie(state: &AppState, raw: Option<&str>) -> ArmOutcome {
    let raw = match raw {
        Some(v) => v,
        None => return ArmOutcome::NoInput,
    };
    let secret = state.config.secret.expose_secret().as_bytes().to_vec();
    let token_value = match decode_cookie(raw, &secret) {
        Ok(v) => v,
        Err(e) => {
            ::tracing::warn!(error = ?e, "cookie_hmac_invalid; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
    };
    let ap = match state.users.app_passwords() {
        Some(ap) => ap.clone(),
        None => return ArmOutcome::NoInput,
    };
    let row = match ap.verify(&token_value).await {
        Ok(r) if r.kind == AuthTokenType::Session => r,
        Ok(_) => {
            ::tracing::warn!("cookie_wrong_kind; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
        Err(_) => {
            ::tracing::warn!("cookie_unknown_or_unusable; falling through to anonymous");
            return ArmOutcome::NoInput;
        }
    };
    ArmOutcome::Authenticated(AuthContext {
        user_id: row.uid,
        method: AuthMethod::Session,
        token_id: row.id,
        login_name: row.login_name,
        remember: row.remember,
    })
}

fn extract_cookie_value_owned<B>(req: &Request<B>, name: &str) -> Option<String> {
    let raw = req
        .headers()
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?;
    for piece in raw.split(';').map(str::trim) {
        if let Some((k, v)) = piece.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

async fn verify_and_build(
    state: &AppState,
    raw: &str,
    method: AuthMethod,
    expected_uid: Option<&str>,
) -> Option<AuthContext> {
    let ap = state.users.app_passwords()?.clone();
    let row = match ap.verify(raw).await {
        Ok(r) => r,
        Err(e) => {
            ::tracing::warn!(error = %e, ?method, "auth_token_not_found");
            return None;
        }
    };
    if let Some(expected) = expected_uid {
        use subtle::ConstantTimeEq;
        if row
            .uid
            .as_str()
            .as_bytes()
            .ct_eq(expected.as_bytes())
            .unwrap_u8()
            != 1
        {
            ::tracing::warn!("auth_basic_uid_mismatch");
            return None;
        }
    }
    Some(AuthContext {
        user_id: row.uid,
        method,
        token_id: row.id,
        login_name: row.login_name,
        remember: row.remember,
    })
}
