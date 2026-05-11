//! Root `App` component + Dioxus `Route` enum. The App pulls per-request
//! data from the server via `use_server_cached` (the closure runs only on the
//! server; the result is replayed into the hydration payload), exposes it to
//! descendants through `use_context`, and emits `<meta name="requesttoken">`
//! so legacy client code that reads the CSRF token from the DOM keeps working.

use crate::context::RequestContext;
use crate::pages::{home::Home, login::Login, not_found::NotFound};
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
