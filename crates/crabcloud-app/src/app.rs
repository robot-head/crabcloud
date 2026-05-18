//! Root `App` component + Dioxus `Route` enum. The App pulls per-request
//! data from the server via `use_server_cached` (the closure runs only on the
//! server; the result is replayed into the hydration payload), exposes it to
//! descendants through `use_context`, and emits `<meta name="requesttoken">`
//! so legacy client code that reads the CSRF token from the DOM keeps working.

use crate::context::RequestContext;
use crate::pages::{
    activity::ActivityPage, home::Home, login::Login, login_v2_flow::LoginV2Flow,
    not_found::NotFound, public_link::PublicLink as PublicLinkPage,
    settings_notifications::SettingsNotifications, settings_security::SettingsSecurity,
    trash::TrashPage,
};
use dioxus::prelude::*;

/// Routes the SSR side honors. The browser router has the same shape so
/// hydration matches.
#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    /// Authenticated landing page (or login redirect for anonymous users).
    #[route("/")]
    HomeRoute {},

    /// Login form.
    #[route("/login")]
    LoginRoute {},

    /// Nextcloud-client `login/v2` flow page — the URL the desktop/mobile
    /// client opens in the user's browser after `POST /index.php/login/v2`.
    #[route("/index.php/login/v2/flow/:flow_id")]
    LoginV2FlowRoute { flow_id: String },

    /// Per-user security settings: list/create/revoke app passwords and
    /// log out everywhere else.
    #[route("/settings/security")]
    SettingsSecurityRoute {},

    /// Per-user email-notification settings: toggle each event type.
    #[route("/settings/notifications")]
    SettingsNotificationsRoute {},

    /// Files browser. Catch-all so paths like `/apps/files/photos/vacation`
    /// route here and the page renders the folder identified by `segments`.
    #[route("/apps/files/:..segments")]
    FilesRoute { segments: Vec<String> },

    /// Trash bin — flat per-user view of soft-deleted files and folders.
    /// See SP12 spec (trash bin) §2.
    #[route("/trash")]
    TrashRoute {},

    /// Activity feed — descending list of recent events affecting the
    /// authed user. See SP14 spec §5.1.
    #[route("/activity")]
    ActivityRoute {},

    /// Anonymous public-link viewer root. `/s/<token>` renders the linked
    /// folder (or the password gate, depending on the auth context the
    /// upstream `public_link_auth` middleware attached).
    #[route("/s/:token")]
    PublicLinkRoute { token: String },

    /// Nested public-link path. `/s/<token>/sub/folder` renders the same
    /// page with `path == ["sub", "folder"]`.
    #[route("/s/:token/:..path")]
    PublicLinkPathRoute { token: String, path: Vec<String> },

    /// Catch-all 404 page. SSR uses this to detect unknown paths and emit
    /// an HTTP 404 status.
    #[route("/:..segments")]
    NotFoundRoute {
        /// Captured path segments; ignored by the page itself.
        segments: Vec<String>,
    },
}

#[component]
pub fn HomeRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { Home { ctx } }
}

#[component]
pub fn LoginRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { Login { ctx } }
}

#[component]
pub fn LoginV2FlowRoute(flow_id: String) -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { LoginV2Flow { ctx: ctx.clone(), flow_id: flow_id.clone() } }
}

#[component]
pub fn SettingsSecurityRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { SettingsSecurity { ctx: ctx.clone() } }
}

#[component]
pub fn SettingsNotificationsRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { SettingsNotifications { ctx: ctx.clone() } }
}

#[component]
pub fn FilesRoute(segments: Vec<String>) -> Element {
    use crate::pages::files::{path::segments_to_path, Files};
    let ctx = use_context::<RequestContext>();
    let path = segments_to_path(&segments);
    rsx! { Files { ctx, path } }
}

#[component]
pub fn TrashRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { TrashPage { ctx } }
}

#[component]
pub fn ActivityRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { ActivityPage { ctx } }
}

#[component]
pub fn PublicLinkRoute(token: String) -> Element {
    rsx! { PublicLinkPage { token, path: Vec::<String>::new() } }
}

#[component]
pub fn PublicLinkPathRoute(token: String, path: Vec<String>) -> Element {
    rsx! { PublicLinkPage { token, path } }
}

#[component]
pub fn NotFoundRoute(segments: Vec<String>) -> Element {
    let _ = segments;
    // Tell the fullstack runtime to set the HTTP status to 404 on the SSR
    // response. Without this the SSR path always returns 200 for the
    // catch-all route, breaking Nextcloud-compatible 404 detection.
    #[cfg(feature = "server")]
    {
        use dioxus::fullstack::FullstackContext;
        FullstackContext::commit_http_status(axum::http::StatusCode::NOT_FOUND, None);
    }
    rsx! { NotFound {} }
}

/// App stylesheet. Bundled via `asset!()` so dx hashes the path and copies
/// the file into the wasm bundle (otherwise the Dioxus.toml `style` reference
/// would point at a path the bundler doesn't ship).
const APP_CSS: Asset = asset!("/assets/app.css");

/// On wasm32 only: monkey-patch `window.fetch` so every same-origin request
/// to a server-fn endpoint (`/api/...`) carries the `requesttoken` header
/// the CSRF middleware (`crabcloud-http/src/csrf.rs`) requires. The token
/// itself comes from the `<meta name="requesttoken">` tag this App emits.
///
/// Why this exists: the Dioxus 0.7 server-fn client (built on `gloo_net` →
/// `window.fetch`) has no per-call header API and no knowledge of our CSRF
/// scheme. Without this shim every authenticated WASM-side server-fn call
/// (list_dir, mkdir, rename, delete, etc.) is rejected with 403.
///
/// The direct-fetch upload code in `pages/files/upload.rs` sidesteps the
/// issue by sending `ocs-apirequest: true`, but that bypasses CSRF rather
/// than satisfying it — we want the real token on the standard server-fn
/// path so protection stays in place.
///
/// Called from `main.rs` before `dioxus::launch` so the patch is in place
/// before any component code runs (and well before list_dir fires its
/// first request). The patched function reads the meta tag at call time,
/// not at install time, so it doesn't matter that the App component hasn't
/// rendered yet — the SSR'd HTML already has the tag in the document.
///
/// The JS body is compiled into the wasm-bindgen glue via `inline_js`
/// (served from `'self'`) rather than `js_sys::eval`: the server's CSP
/// (`script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'`) blocks runtime
/// `eval` but permits scripts from same-origin and inline content.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function install_csrf_fetch_interceptor() {
    if (window.__crabcloud_fetch_patched) return;
    window.__crabcloud_fetch_patched = true;
    var orig = window.fetch.bind(window);
    window.fetch = function(input, init) {
        init = init || {};
        var url = typeof input === "string"
            ? input
            : ((input && input.url) || "");
        var apiPrefix = "/api/";
        var sameOriginApi = url.indexOf(apiPrefix) === 0
            || url.indexOf(location.origin + apiPrefix) === 0;
        if (sameOriginApi) {
            var meta = document.querySelector('meta[name="requesttoken"]');
            var token = meta ? meta.getAttribute("content") : "";
            if (token) {
                var headers = new Headers(
                    init.headers || (input && input.headers) || undefined
                );
                if (!headers.has("requesttoken")) {
                    headers.set("requesttoken", token);
                }
                init.headers = headers;
            }
        }
        return orig(input, init);
    };
    try { console.log("[crabcloud] CSRF fetch interceptor installed"); } catch (e) {}
}
"#)]
extern "C" {
    pub fn install_csrf_fetch_interceptor();
}

/// Root component. Captures the per-request context on the server, replays it
/// on the client via the hydration payload, and provides it to descendants.
/// Emits `<meta name="requesttoken">` and the `data-hydrated` marker the
/// Playwright suite waits on.
#[component]
pub fn App() -> Element {
    let ctx = use_server_cached(|| {
        #[cfg(feature = "server")]
        {
            crate::server::current_request_context()
        }
        #[cfg(not(feature = "server"))]
        {
            RequestContext::anonymous("en", "")
        }
    });
    use_context_provider(|| ctx.clone());

    let mut hydrated = use_signal(|| false);
    use_effect(move || {
        hydrated.set(true);
    });
    let value = if hydrated() { "true" } else { "false" };
    let request_token = ctx.request_token.clone();

    rsx! {
        document::Stylesheet { href: APP_CSS }
        document::Meta { name: "requesttoken", content: request_token }
        div { id: "app-root", "data-hydrated": "{value}",
            Router::<Route> {}
        }
    }
}
