//! Page chrome: top bar (logo, app name, user chip) + left sidebar
//! ("All files" only for MVP). See spec §2 (decision 7).

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn TopBar(ctx: RequestContext) -> Element {
    let display = ctx
        .display_name
        .clone()
        .unwrap_or_else(|| "User".to_string());
    let initial = display
        .chars()
        .next()
        .unwrap_or('U')
        .to_uppercase()
        .to_string();
    rsx! {
        header { class: "topbar",
            a { class: "topbar-brand", href: "/", "Crabcloud" }
            nav { class: "topbar-nav",
                a { class: "topbar-link active", href: "/apps/files/", "Files" }
            }
            div { class: "topbar-spacer" }
            div { class: "topbar-user", title: "{display}", "{initial}" }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    rsx! {
        aside { class: "sidebar",
            ul { class: "sidebar-list",
                li { class: "sidebar-item active",
                    span { class: "sidebar-icon", "📂" }
                    span { "All files" }
                }
            }
        }
    }
}
