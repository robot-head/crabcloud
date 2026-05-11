//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

// On non-wasm targets this binary collapses to a single eprintln! so the
// `[dependencies]` block looks fully unused — they're consumed only by the
// `#[cfg(target_arch = "wasm32")]` branch above.
#![allow(unused_crate_dependencies)]

#[cfg(target_arch = "wasm32")]
mod web {
    use crabcloud_ui::{RequestContext, Route};
    use dioxus::prelude::*;

    /// Top-level component. Reads the hydration payload from the DOM on first
    /// render, installs it as the request context, then mounts the router.
    #[component]
    pub fn AppRoot() -> Element {
        let ctx = use_hook(|| {
            read_hydration_context().unwrap_or_else(|| RequestContext::anonymous("en", ""))
        });
        use_context_provider(|| ctx.clone());
        rsx! { Router::<Route> {} }
    }

    pub fn launch() {
        dioxus_web::launch::launch(
            AppRoot,
            Vec::new(),
            vec![Box::new(dioxus_web::Config::new().hydrate(true))],
        );
    }

    fn read_hydration_context() -> Option<RequestContext> {
        let window = web_sys::window()?;
        let document = window.document()?;
        let el = document.get_element_by_id("__dx_ctx")?;
        let json = el.text_content()?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    web::launch();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("crabcloud-ui-web is a WASM-only entry point");
}
