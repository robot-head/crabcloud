//! `/login` — Login form that POSTs to `/index.php/login` (Phase 3 handler).
//! Works without JavaScript; the WASM client may enhance it later.

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn Login(ctx: RequestContext) -> Element {
    let _ = ctx; // unused for now; Phase 5+ may pre-fill username from cookie.
    rsx! {
        main { class: "login",
            h1 { "Log in" }
            form {
                method: "post",
                action: "/index.php/login",
                "accept-charset": "utf-8",

                label { r#for: "username", "Username" }
                input { id: "username", name: "username", r#type: "text", autocomplete: "username", required: true }

                label { r#for: "password", "Password" }
                input { id: "password", name: "password", r#type: "password", autocomplete: "current-password", required: true }

                button { r#type: "submit", "Log in" }
            }
        }
    }
}
