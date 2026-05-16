//! Axum middleware that gates anonymous public-link traffic.
//!
//! Flow:
//! 1. Parse the token from the request path (`/s/{token}` for the browser
//!    surface, `/public.php/dav/files/{token}/...` for DAV).
//! 2. Look the token up via `TokenLookup`. Unknown → 404.
//! 3. If `expiration` is set and past, return 404 (indistinguishable from
//!    a missing token — we don't leak the existence of expired shares).
//! 4. If the row carries a password, enforce the gate:
//!    - **Browser:** look for a valid `pl_<token>` unlock cookie. If absent
//!      or invalid, insert a `PasswordGateRequired` extension and continue
//!      to the downstream handler (which renders the password form). We
//!      deliberately do *not* 401 the browser surface because the viewer
//!      page *is* the gate; a 401 would surface the browser's native auth
//!      dialog, which is the wrong UX.
//!    - **DAV:** rate-limit per token first (to avoid revealing whether a
//!      given password attempt was correct on a throttled token), then
//!      verify HTTP Basic. Missing / wrong → 401 with
//!      `WWW-Authenticate: Basic realm="public-link"`.
//! 5. Attach a `PublicLinkAuthContext` extension and call the next layer.
//!
//! The middleware is registered via `axum::middleware::from_fn_with_state`,
//! so it receives `Arc<PublicLinkAuthState>` cloned per request — all
//! interior fields are themselves `Arc`-backed or `Vec<u8>` so this is
//! cheap.

use crate::{
    cookie::UnlockCookie,
    passwords::{HashedPassword, Passwords},
    ratelimit::{RateLimitDecision, RateLimiter},
    tokens::{Token, TokenLookup},
    PublicLinkAuthContext,
};
use axum::{
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use std::sync::Arc;

/// Bits SP7's `SharePermissions::from_wire` keeps after masking. We replicate
/// the normalisation locally so this crate doesn't have to depend on
/// `crabcloud-sharing` (which depends back on us). Downstream consumers can
/// still wrap the stored `u32` via `SharePermissions::from_wire` for a typed
/// view; the value is byte-for-byte identical either way.
const PERMISSION_MASK: u32 = 0x1F & !0x10; // 0x0F: read/update/create/delete, no share bit.

fn normalise_permissions(b: u32) -> u32 {
    b & PERMISSION_MASK
}

/// Composition handle for the public-link auth layer. Constructed once at
/// startup and shared by every request via `from_fn_with_state`.
pub struct PublicLinkAuthState {
    /// Resolves opaque tokens to `LinkRow`s.
    pub lookup: Arc<dyn TokenLookup>,
    /// Bcrypt verifier reused across requests (stateless, but factored out
    /// so the verifier policy can change in one place).
    pub passwords: Arc<Passwords>,
    /// Per-token windowed counter; throttles DAV password attempts.
    pub rate_limiter: Arc<RateLimiter>,
    /// HMAC key for unlock-cookie verification. Reuses `FileConfig::secret`
    /// — cleanly domain-separated by cookie name (`pl_<token>`) and by
    /// inclusion of the token in the MAC input.
    pub secret: Vec<u8>,
}

/// Which surface the middleware is running on. Two surfaces share the same
/// auth state but differ in password-gate enforcement (cookie vs. Basic).
/// Token extraction is prefix-agnostic — the surface prefix is stripped by
/// `Router::nest` before this middleware runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSurface {
    /// Browser-facing `/s/{token}`; password gate is a cookie set by the
    /// viewer's password form.
    Browser,
    /// `/public.php/dav/...`; password gate is HTTP Basic.
    Dav,
}

/// Marker extension inserted on the browser surface when a password gate
/// is required but no valid unlock cookie was presented. The viewer handler
/// branches on this to render the password form instead of the file list.
#[derive(Debug, Clone, Copy)]
pub struct PasswordGateRequired;

/// Middleware entry point. Mounted via
/// `axum::middleware::from_fn_with_state(state.clone(), |req, next|
/// public_link_auth(state.clone(), AuthSurface::Browser, req, next))`.
pub async fn public_link_auth(
    state: Arc<PublicLinkAuthState>,
    surface: AuthSurface,
    mut req: Request,
    next: Next,
) -> Response {
    // The middleware is mounted via `Router::nest(prefix, …)`, so by the time
    // we run, `req.uri().path()` has already been re-rooted to the path
    // *after* the surface prefix (`/{token}/…`). `extract_token` just takes
    // the first segment of whatever we hand it — no prefix-stripping needed.
    let token = match extract_token(req.uri().path()) {
        Some(t) => t,
        None => return not_found(),
    };

    let row = match state.lookup.lookup(token.as_str()).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => {
            // The lookup error type is opaque to this crate (the adapter in
            // `crabcloud-core` boxes it), so we can only print via `Display`.
            // Token is logged so operators can correlate to the failing
            // share row; tokens are not secrets — possession of one is the
            // capability, but the act of revealing it in a server-side log
            // doesn't grant access to anyone reading the log.
            tracing::warn!(
                error = %e,
                token = %token.as_str(),
                "public-link token lookup failed"
            );
            return server_error();
        }
    };

    // Past-expiration is indistinguishable from "no such token". We rely
    // on `chrono::Utc::now()` rather than an injected clock; the auth
    // layer does not have a clock dependency yet and shaving microseconds
    // off this comparison doesn't justify the plumbing.
    if let Some(exp) = row.expiration {
        if exp < chrono::Utc::now() {
            return not_found();
        }
    }

    // Tracked separately from the `PasswordGateRequired` marker extension so
    // we can also stamp it onto `PublicLinkAuthContext`. The marker remains
    // the "render the gate page" signal for the viewer; the field is the
    // "is this context safe to act on?" signal that every downstream handler
    // is forced to acknowledge (the only legitimate consumer when it's `true`
    // is the unlock POST handler, which needs the resolved share to mint a
    // cookie). Dav never sets this — Dav 401s on missing/wrong credentials
    // before reaching the context-build step.
    let mut gate_required = false;

    if let Some(hashed) = row.password_hash.as_deref() {
        match surface {
            AuthSurface::Browser => {
                if !browser_unlocked(&state, &token, &req) {
                    // Note: no 401. The viewer renders the gate variant
                    // based on this marker. Returning 401 here would pop a
                    // browser auth dialog, which is the wrong UX.
                    req.extensions_mut().insert(PasswordGateRequired);
                    gate_required = true;
                }
            }
            AuthSurface::Dav => {
                // Rate-limit *before* verification so a throttled token
                // doesn't reveal correctness via timing.
                if let RateLimitDecision::Throttled { retry_after_secs } =
                    state.rate_limiter.check_password_attempt(token.as_str())
                {
                    return throttled(retry_after_secs);
                }
                if !dav_unlocked(&state, hashed, &req) {
                    return basic_challenge();
                }
            }
        }
    }

    let owner_uid = match UserId::new(row.owner_uid) {
        Ok(u) => u,
        Err(_) => return server_error(),
    };
    let owner_path = match StoragePath::new(row.owner_path.trim_start_matches('/').to_string()) {
        Ok(p) => p,
        Err(_) => return server_error(),
    };

    req.extensions_mut().insert(PublicLinkAuthContext {
        link_share_id: row.share_id,
        owner_uid,
        owner_path,
        permissions: normalise_permissions(row.permissions),
        password_gate_required: gate_required,
    });

    next.run(req).await
}

/// Extract the token from the post-prefix path. The caller mounts this
/// middleware via `Router::nest(prefix, …)`, so axum already stripped the
/// surface prefix before we ran; we just take the first non-empty path
/// segment. Returns `None` for paths that don't parse as a well-formed
/// 15-char base62 token, which short-circuits DB lookups for typos and bot
/// probes.
fn extract_token(path_after_prefix: &str) -> Option<Token> {
    let first = path_after_prefix
        .trim_start_matches('/')
        .split('/')
        .next()?;
    Token::parse(first)
}

/// True iff the request carries a valid `pl_<token>` unlock cookie. We
/// iterate every `Cookie` header (axum may report multiples) and every
/// `name=value` pair within each header, so the verifier sees the cookie
/// regardless of how the client packs them.
fn browser_unlocked(state: &PublicLinkAuthState, token: &Token, req: &Request) -> bool {
    let name = UnlockCookie::cookie_name_for(token.as_str());
    let prefix = format!("{name}=");
    let now = chrono::Utc::now().timestamp();
    for header_value in req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
    {
        for pair in header_value.split(';') {
            let pair = pair.trim();
            if let Some(rest) = pair.strip_prefix(&prefix) {
                if UnlockCookie::verify(&state.secret, token.as_str(), rest, now).is_ok() {
                    return true;
                }
            }
        }
    }
    false
}

/// True iff the request carries a valid HTTP Basic credential. We tolerate
/// both `anonymous:pw` (the desktop client default) and `:pw` (some
/// scripts). The username is ignored; only the password is checked.
fn dav_unlocked(state: &PublicLinkAuthState, hashed: &str, req: &Request) -> bool {
    let raw = match req.headers().get(header::AUTHORIZATION) {
        Some(h) => h,
        None => return false,
    };
    let s = match raw.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let token_part = match s.strip_prefix("Basic ") {
        Some(t) => t,
        None => return false,
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(token_part) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let decoded = match std::str::from_utf8(&decoded) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Strict ASCII colon split; tolerates empty username. Returns "" when
    // the input contains no colon — which fails verification cleanly.
    let password = decoded.split_once(':').map(|(_, p)| p).unwrap_or("");
    let hp = HashedPassword::from_stored(hashed.to_string());
    state.passwords.verify(password, &hp)
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "").into_response()
}

fn server_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
}

fn basic_challenge() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"public-link\""),
    );
    resp
}

fn throttled(retry_after_secs: u64) -> Response {
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
    resp.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&retry_after_secs.to_string())
            .unwrap_or(HeaderValue::from_static("3600")),
    );
    resp
}
