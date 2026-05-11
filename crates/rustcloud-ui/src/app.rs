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
    #[route("/")]
    HomeRoute {},

    #[route("/login")]
    LoginRoute {},

    #[route("/:..segments")]
    NotFoundRoute { segments: Vec<String> },
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

/// Root component. Renders the `Router<Route>`. Callers must install
/// `RequestContext` into the context before rendering (see `ssr.rs`).
#[component]
pub fn App() -> Element {
    rsx! { Router::<Route> {} }
}
