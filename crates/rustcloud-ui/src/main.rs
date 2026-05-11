//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

#[cfg(target_arch = "wasm32")]
mod web {
    use dioxus::prelude::*;
    use dioxus_router::prelude::*;
    use rustcloud_ui::{RequestContext, Route};

    #[component]
    fn AppRoot(ctx: RequestContext) -> Element {
        use_context_provider(|| ctx.clone());
        rsx! { Router::<Route> {} }
    }

    pub fn launch() {
        let ctx = read_hydration_context().unwrap_or_else(|| RequestContext::anonymous("en", ""));
        dioxus::launch(move || {
            rsx! { AppRoot { ctx: ctx.clone() } }
        });
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
    eprintln!("rustcloud-ui-web is a WASM-only entry point");
}
