//! Server-only glue: pulls per-request data (user id, locale, CSRF token)
//! out of the live axum request via `FullstackContext` and packages it as a
//! `RequestContext` for the App root to provide as component context.
//!
//! Compiled only when the `server` feature is enabled. We read `AppState` and
//! `SessionHandle` (installed by `crabcloud-http`'s middleware stack) from
//! the request's extensions; the locale resolves against `AppState.i18n` and
//! the `accept-language` header on the request.

use crate::context::RequestContext;
use dioxus::fullstack::FullstackContext;

/// Read the per-request context from the current axum request. Falls back to
/// an anonymous "en"/empty-token context when the request lacks the
/// extensions we expect (e.g. during static prerenders or hydration replay
/// on the client, where `FullstackContext::current()` returns `None`).
pub fn current_request_context() -> RequestContext {
    let Some(fs) = FullstackContext::current() else {
        return RequestContext::anonymous("en", "");
    };

    let session = fs.extension::<crabcloud_http::SessionHandle>();
    let state = fs.extension::<crabcloud_core::AppState>();

    let snapshot = session.as_ref().and_then(|s| s.try_read_snapshot());
    let user_id = snapshot.as_ref().and_then(|s| s.user_id.clone());
    let request_token = snapshot
        .as_ref()
        .map(|s| s.csrf_token.clone())
        .unwrap_or_default();

    let accept_lang = {
        let parts = fs.parts_mut();
        parts
            .headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let locale = state
        .as_ref()
        .map(|state| {
            let available = state.i18n.available_locales().to_vec();
            let fallback = crabcloud_i18n::Locale::new(state.config.default_language.as_str());
            crabcloud_i18n::resolve(&accept_lang, &available, &fallback)
                .as_str()
                .to_string()
        })
        .unwrap_or_else(|| "en".to_string());

    match user_id {
        Some(uid) => RequestContext::authenticated(uid, locale, request_token),
        None => RequestContext::anonymous(locale, request_token),
    }
}
