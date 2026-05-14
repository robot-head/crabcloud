//! `/settings/security` — list/create/revoke app passwords + log out
//! everywhere else.
//!
//! Anonymous visitors get a sign-in stub; authenticated users see a list of
//! their `oc_authtoken` rows (Session and AppPassword kinds), with controls
//! to revoke individual rows, revoke every row except the current one, and
//! mint a fresh app password. The freshly-minted plaintext token is shown
//! exactly once.

use crate::context::RequestContext;
use crate::server_fns::{
    create_app_password, destroy_other_sessions, list_app_passwords, revoke_app_password,
    AuthTokenSummary, CreatedAppPassword,
};
use dioxus::prelude::*;

#[component]
pub fn SettingsSecurity(ctx: RequestContext) -> Element {
    let mut tokens = use_signal(Vec::<AuthTokenSummary>::new);
    let mut just_created = use_signal(|| Option::<CreatedAppPassword>::None);
    let mut new_name = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);

    // Load once after first render. The inline body keeps us out of the
    // closure-cloning corner of Dioxus 0.7's signal/spawn ergonomics — we
    // duplicate this `list -> set tokens` snippet at every callsite below.
    use_effect(move || {
        spawn(async move {
            match list_app_passwords().await {
                Ok(rows) => tokens.set(rows),
                Err(e) => error.set(Some(format!("{e}"))),
            }
        });
    });

    if ctx.user_id.is_none() {
        return rsx! {
            main { class: "settings-security",
                h1 { "Please log in" }
                p { a { href: "/login", "Log in" } }
            }
        };
    }

    rsx! {
        main { class: "settings-security",
            h1 { "Security" }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
            section {
                h2 { "Active devices" }
                table {
                    thead { tr { th { "Name" } th { "Type" } th { "Last activity" } th {} } }
                    tbody {
                        for row in tokens().into_iter() {
                            tr {
                                td { "{row.name}" }
                                td { if row.kind == 0 { "Browser session" } else { "App password" } }
                                td { "{row.last_activity}" }
                                td {
                                    if row.current {
                                        em { "current" }
                                    } else {
                                        button {
                                            onclick: move |_| {
                                                let id = row.id;
                                                spawn(async move {
                                                    let _ = revoke_app_password(id).await;
                                                    match list_app_passwords().await {
                                                        Ok(rows) => tokens.set(rows),
                                                        Err(e) => error.set(Some(format!("{e}"))),
                                                    }
                                                });
                                            },
                                            "Revoke"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                button {
                    onclick: move |_| {
                        spawn(async move {
                            let _ = destroy_other_sessions().await;
                            match list_app_passwords().await {
                                Ok(rows) => tokens.set(rows),
                                Err(e) => error.set(Some(format!("{e}"))),
                            }
                        });
                    },
                    "Log out everywhere else"
                }
            }
            section {
                h2 { "Create app password" }
                if let Some(created) = just_created() {
                    div { class: "created",
                        p { "Copy this password now — it will not be shown again:" }
                        code { "{created.raw_token}" }
                        button {
                            onclick: move |_| just_created.set(None),
                            "Dismiss"
                        }
                    }
                } else {
                    form {
                        onsubmit: move |evt: FormEvent| {
                            evt.prevent_default();
                            let name = new_name();
                            if name.is_empty() { return; }
                            spawn(async move {
                                match create_app_password(name.clone()).await {
                                    Ok(c) => {
                                        just_created.set(Some(c));
                                        new_name.set(String::new());
                                        match list_app_passwords().await {
                                            Ok(rows) => tokens.set(rows),
                                            Err(e) => error.set(Some(format!("{e}"))),
                                        }
                                    }
                                    Err(e) => error.set(Some(format!("{e}"))),
                                }
                            });
                        },
                        input {
                            r#type: "text",
                            placeholder: "Device name (e.g. \"iPhone\")",
                            value: "{new_name}",
                            oninput: move |evt| new_name.set(evt.value()),
                        }
                        button { r#type: "submit", "Create" }
                    }
                }
            }
        }
    }
}
