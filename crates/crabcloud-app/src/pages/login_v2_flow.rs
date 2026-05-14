//! `/index.php/login/v2/flow/<flow-id>` — the page the Nextcloud client
//! opens in the user's browser to authorize a fresh app password.
//!
//! Anonymous visitors get a "please log in first" stub; authenticated
//! sessions get an Authorize button that invokes the `login_v2_authorize`
//! server fn, which mints an AppPassword-kind token and hands it to the
//! client's polling channel.

use crate::context::RequestContext;
use crate::server_fns::login_v2_authorize;
use dioxus::prelude::*;

#[component]
pub fn LoginV2Flow(ctx: RequestContext, flow_id: String) -> Element {
    let mut authorized = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let flow_id_for_submit = flow_id.clone();

    if ctx.user_id.is_none() {
        return rsx! {
            main { class: "login-v2-flow",
                h1 { "Sign in required" }
                p { "Please log in first, then return to this URL to authorize the app." }
                p { a { href: "/login", "Log in" } }
            }
        };
    }

    if authorized() {
        return rsx! {
            main { class: "login-v2-flow",
                h1 { "Authorized" }
                p { "You can close this tab now." }
            }
        };
    }

    rsx! {
        main { class: "login-v2-flow",
            h1 { "Authorize app" }
            p { "This will grant the calling application access to your account using an app password. You can revoke it from Settings → Security at any time." }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
            button {
                onclick: move |_| {
                    let fid = flow_id_for_submit.clone();
                    spawn(async move {
                        match login_v2_authorize(fid).await {
                            Ok(()) => authorized.set(true),
                            Err(e) => error.set(Some(format!("{e}"))),
                        }
                    });
                },
                "Authorize"
            }
        }
    }
}
