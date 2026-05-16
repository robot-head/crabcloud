//! `POST /s/{token}/unlock` handler — verify the link password, mint a
//! `pl_<token>` cookie, redirect back to the viewer. See `super` for the
//! surface-level docs.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use crabcloud_core::AppState;
use crabcloud_publiclinks::{RateLimitDecision, Token, UnlockCookie};
use serde::Deserialize;

use super::UNLOCK_COOKIE_TTL_SECS;

/// Form body for POST /s/{token}/unlock.
#[derive(Debug, Deserialize)]
pub(super) struct UnlockForm {
    password: String,
}

/// `POST /s/{token}/unlock` — verify the password and mint a `pl_<token>`
/// cookie. Intentionally NOT gated on `password_gate_required`; this is the
/// endpoint that LEAVES that state.
pub(super) async fn unlock_handler(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<UnlockForm>,
) -> Response {
    // The middleware already validated the token shape (and 404'd unknown
    // tokens), so the extension exists — but the handler still does a
    // defensive parse so the same body works if it ever gets called from
    // an alternate mount point.
    let Some(_t) = Token::parse(&token) else {
        return (StatusCode::NOT_FOUND, "").into_response();
    };

    let auth = &state.publiclinks_auth;
    if let RateLimitDecision::Throttled { retry_after_secs } =
        auth.rate_limiter.check_password_attempt(&token)
    {
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
        resp.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry_after_secs.to_string())
                .unwrap_or(HeaderValue::from_static("3600")),
        );
        return resp;
    }

    let row = match auth.lookup.lookup(&token).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "unlock: token lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // Expired link → indistinguishable from missing.
    if let Some(exp) = row.expiration {
        if exp < chrono::Utc::now() {
            return (StatusCode::NOT_FOUND, "").into_response();
        }
    }

    let Some(stored_hash) = row.password_hash.as_deref() else {
        // Link doesn't require a password — caller is confused.
        return (StatusCode::BAD_REQUEST, "link has no password").into_response();
    };

    let hashed = crabcloud_publiclinks::HashedPassword::from_stored(stored_hash.to_string());
    if !auth.passwords.verify(&form.password, &hashed) {
        return (StatusCode::UNAUTHORIZED, "wrong password").into_response();
    }

    let exp_unix = chrono::Utc::now().timestamp() + UNLOCK_COOKIE_TTL_SECS;
    let cookie_value = UnlockCookie::sign(&auth.secret, &token, exp_unix);
    let cookie_name = UnlockCookie::cookie_name_for(&token);
    let secure_attr = if state
        .config
        .overwrite_protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
    {
        " Secure;"
    } else {
        ""
    };
    let set_cookie = format!(
        "{cookie_name}={cookie_value}; Path=/; Max-Age={ttl}; HttpOnly;{secure_attr} SameSite=Lax",
        ttl = UNLOCK_COOKIE_TTL_SECS
    );
    let redirect_to = format!("/s/{token}");
    let mut resp = (StatusCode::SEE_OTHER, "").into_response();
    {
        let h = resp.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&set_cookie) {
            h.insert(header::SET_COOKIE, v);
        }
        if let Ok(v) = HeaderValue::from_str(&redirect_to) {
            h.insert(header::LOCATION, v);
        }
    }
    resp
}
