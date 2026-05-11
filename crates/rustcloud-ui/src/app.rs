//! Root `App` component + Dioxus `Route` enum. Provides `RequestContext` via
//! context so any descendant component can call `use_context::<RequestContext>()`.

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
    rsx! { Home { ctx: ctx.clone() } }
}

#[component]
pub fn LoginRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { Login { ctx: ctx.clone() } }
}

#[component]
pub fn NotFoundRoute(segments: Vec<String>) -> Element {
    let _ = segments;
    rsx! { NotFound {} }
}

/// Root component. Renders the `Router<Route>` inside a hydration marker div.
/// The `data-hydrated` attribute flips from "false" (SSR) to "true" once the
/// WASM client mounts and runs the effect — Playwright E2E waits on this.
#[component]
pub fn App() -> Element {
    let mut hydrated = use_signal(|| false);
    use_effect(move || {
        hydrated.set(true);
    });
    let value = if hydrated() { "true" } else { "false" };
    rsx! {
        div { id: "app-root", "data-hydrated": "{value}",
            Router::<Route> {}
        }
    }
}
