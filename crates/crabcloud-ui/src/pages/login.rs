//! `/login` — Login form that invokes the `login` server function. The form
//! still degrades gracefully without JS: the same `#[server]` function is
//! registered at `/index.php/login`, so a non-enhanced submission hits the
//! same handler.

use crate::context::RequestContext;
use crate::server_fns::login;
use dioxus::prelude::*;

#[component]
pub fn Login(ctx: RequestContext) -> Element {
    let _ = ctx;
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let nav = use_navigator();

    let on_submit = move |evt: FormEvent| {
        evt.prevent_default();
        let u = username();
        let p = password();
        spawn(async move {
            match login(u, p).await {
                Ok(()) => {
                    nav.replace("/");
                }
                Err(e) => error.set(Some(format!("{e}"))),
            }
        });
    };

    rsx! {
        main { class: "login",
            h1 { "Log in" }
            form {
                onsubmit: on_submit,
                method: "post",
                action: "/index.php/login",
                "accept-charset": "utf-8",

                label { r#for: "username", "Username" }
                input {
                    id: "username", name: "username", r#type: "text",
                    autocomplete: "username", required: true,
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }

                label { r#for: "password", "Password" }
                input {
                    id: "password", name: "password", r#type: "password",
                    autocomplete: "current-password", required: true,
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }

                button { r#type: "submit", "Log in" }

                if let Some(msg) = error() {
                    p { class: "login-error", "{msg}" }
                }
            }
        }
    }
}
