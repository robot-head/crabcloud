//! `/` — Welcome page. Shows the authenticated user's display name or a
//! "guest" greeting; links to `/login` when anonymous.

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn Home(ctx: RequestContext) -> Element {
    let greeting = match &ctx.display_name {
        Some(name) => format!("Welcome, {name}"),
        None => "Welcome, guest".to_string(),
    };
    let show_login_link = ctx.user_id.is_none();
    rsx! {
        main { class: "home",
            h1 { "{greeting}" }
            p { "Rustcloud — a Rust port of Nextcloud server." }
            if show_login_link {
                p { a { href: "/login", "Log in" } }
            }
        }
    }
}
