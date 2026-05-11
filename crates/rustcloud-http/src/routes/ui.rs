//! Axum SSR handler. Renders the Dioxus `App` for the requested URL, wraps it
//! in an HTML shell, and injects the hydration payload.
//!
//! The pure-render helpers (head, body, escape) live in `rustcloud-ui` so
//! the UI crate has no dependency on `rustcloud-http`. This module wires
//! those helpers to axum extractors and the session/auth state.

use axum::extract::{OriginalUri, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rustcloud_core::AppState;
use rustcloud_i18n::{resolve, Locale};
use rustcloud_ui::{render_app_html, render_head_html, RequestContext, HTML_DOCTYPE};

use crate::extractors::auth::OptionalUser;
use crate::session::SessionHandle;

pub async fn handler(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    axum::Extension(session): axum::Extension<SessionHandle>,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
) -> Response {
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let available = state.i18n.available_locales().to_vec();
    let fallback = Locale::new(state.config.default_language.as_str());
    let locale = resolve(accept_lang, &available, &fallback);

    let session_snapshot = session.read().await;
    let request_token = session_snapshot.csrf_token.clone();
    drop(session_snapshot);

    let ctx = match user {
        Some(u) => RequestContext::authenticated(u.user_id, locale.as_str(), request_token),
        None => RequestContext::anonymous(locale.as_str(), request_token),
    };

    let body_html = render_app_html(ctx.clone(), uri.path());
    let head_html = render_head_html(&ctx);
    let document = format!(
        "{doctype}<html lang=\"{lang}\"><head>{head}</head><body><div id=\"main\">{body}</div></body></html>",
        doctype = HTML_DOCTYPE,
        lang = ctx.locale,
        head = head_html,
        body = body_html,
    );

    let mut resp = (StatusCode::OK, document).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}
