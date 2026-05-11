//! 404 fall-through.

use dioxus::prelude::*;

/// Static 404 page rendered for unknown paths in both SSR and client routes.
#[component]
pub fn NotFound() -> Element {
    rsx! {
        main { class: "not-found",
            h1 { "404 — Not Found" }
            p { "The page you requested does not exist." }
            p { a { href: "/", "Return home" } }
        }
    }
}
