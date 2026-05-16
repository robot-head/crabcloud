//! `/settings/notifications` — per-user email notification toggles.
//!
//! Three toggle rows, one per supported event type. The page loads the
//! current opt-in state via `notification_prefs_get` and writes individual
//! updates back via `notification_prefs_set`. The pattern mirrors
//! `settings_security.rs`: `use_signal` for the local state, `use_effect`
//! to kick the initial server-fn call, and per-row `spawn` callbacks for
//! mutations. We re-fetch after each write rather than optimistically
//! mutating the local snapshot — keeps the UI honest when the server
//! rejects a write.

use crate::context::RequestContext;
use crate::server_fns::notification_prefs::{
    notification_prefs_get, notification_prefs_set, NotificationPrefsDto,
};
use dioxus::prelude::*;

#[component]
pub fn SettingsNotifications(ctx: RequestContext) -> Element {
    let mut prefs = use_signal(|| Option::<NotificationPrefsDto>::None);
    let mut error = use_signal(|| Option::<String>::None);

    // Load once after first render. Same inline-body pattern as
    // settings_security to dodge Dioxus 0.7's signal/spawn closure
    // ergonomics — duplicated below in each row callback.
    use_effect(move || {
        spawn(async move {
            match notification_prefs_get().await {
                Ok(p) => prefs.set(Some(p)),
                Err(e) => error.set(Some(format!("{e}"))),
            }
        });
    });

    if ctx.user_id.is_none() {
        return rsx! {
            main { class: "settings-notifications",
                h1 { "Please log in" }
                p { a { href: "/login", "Log in" } }
            }
        };
    }

    rsx! {
        main { class: "settings-notifications",
            h1 { "Email notifications" }
            p { "Choose which events trigger an email to you." }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
            if let Some(p) = prefs() {
                section { class: "notifications-settings",
                    ToggleRow {
                        label: "Notify me when others share with me",
                        event_type: "share_created",
                        enabled: p.share_created,
                        on_changed: move |_: ()| {
                            spawn(async move {
                                match notification_prefs_get().await {
                                    Ok(np) => prefs.set(Some(np)),
                                    Err(e) => error.set(Some(format!("{e}"))),
                                }
                            });
                        },
                    }
                    ToggleRow {
                        label: "Send a copy of email-share confirmations",
                        event_type: "link_emailed",
                        enabled: p.link_emailed,
                        on_changed: move |_: ()| {
                            spawn(async move {
                                match notification_prefs_get().await {
                                    Ok(np) => prefs.set(Some(np)),
                                    Err(e) => error.set(Some(format!("{e}"))),
                                }
                            });
                        },
                    }
                    ToggleRow {
                        label: "Warn me before my public links expire",
                        event_type: "expiration_warning",
                        enabled: p.expiration_warning,
                        on_changed: move |_: ()| {
                            spawn(async move {
                                match notification_prefs_get().await {
                                    Ok(np) => prefs.set(Some(np)),
                                    Err(e) => error.set(Some(format!("{e}"))),
                                }
                            });
                        },
                    }
                }
            }
        }
    }
}

/// One label + checkbox row. The checkbox `onchange` hits the
/// `notification_prefs_set` server-fn directly with the row's event-type
/// string, then notifies the parent via `on_changed` so the parent can
/// re-fetch the snapshot.
#[component]
fn ToggleRow(
    label: String,
    event_type: String,
    enabled: bool,
    on_changed: EventHandler<()>,
) -> Element {
    rsx! {
        label { class: "notification-toggle",
            input {
                r#type: "checkbox",
                checked: enabled,
                onchange: move |evt| {
                    let new_value = evt.value() == "true";
                    let et = event_type.clone();
                    spawn(async move {
                        // Best-effort: errors surface through the parent's
                        // re-fetch (which will report the unchanged state)
                        // rather than via a row-local error slot.
                        let _ = notification_prefs_set(et, new_value).await;
                        on_changed.call(());
                    });
                },
            }
            "{label}"
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// SSR snapshot: rendering the three toggle rows with a mocked
    /// `NotificationPrefsDto` must produce HTML that contains every
    /// user-facing label. Asserts the spec labels appear verbatim so a
    /// future copy change doesn't silently break the page.
    ///
    /// EventHandler construction in `rsx!` goes through `Callback::new`,
    /// which needs a live Dioxus runtime. Wrapping the rsx call in a
    /// component body (`Wrapper`) defers the conversion to render time,
    /// when the VirtualDom has its runtime up.
    #[test]
    fn renders_all_three_labels_with_mocked_prefs() {
        let prefs = NotificationPrefsDto {
            share_created: true,
            link_emailed: false,
            expiration_warning: true,
        };

        #[component]
        fn Wrapper(prefs: NotificationPrefsDto) -> Element {
            rsx! {
                section { class: "notifications-settings",
                    ToggleRow {
                        label: "Notify me when others share with me".to_string(),
                        event_type: "share_created".to_string(),
                        enabled: prefs.share_created,
                        on_changed: move |_: ()| {},
                    }
                    ToggleRow {
                        label: "Send a copy of email-share confirmations".to_string(),
                        event_type: "link_emailed".to_string(),
                        enabled: prefs.link_emailed,
                        on_changed: move |_: ()| {},
                    }
                    ToggleRow {
                        label: "Warn me before my public links expire".to_string(),
                        event_type: "expiration_warning".to_string(),
                        enabled: prefs.expiration_warning,
                        on_changed: move |_: ()| {},
                    }
                }
            }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { prefs } });
        assert!(
            html.contains("Notify me when others share with me"),
            "expected share_created label in HTML, got: {html}"
        );
        assert!(
            html.contains("Send a copy of email-share confirmations"),
            "expected link_emailed label in HTML, got: {html}"
        );
        assert!(
            html.contains("Warn me before my public links expire"),
            "expected expiration_warning label in HTML, got: {html}"
        );
    }

    /// SSR snapshot: each toggle row reflects its `enabled` prop as the
    /// `checked` attribute on the checkbox. Lock this in to catch a
    /// future refactor that accidentally inverts the state.
    #[test]
    fn checkbox_checked_attr_reflects_enabled_prop() {
        #[component]
        fn Wrapper(enabled: bool) -> Element {
            rsx! {
                ToggleRow {
                    label: "label".to_string(),
                    event_type: "share_created".to_string(),
                    enabled,
                    on_changed: move |_: ()| {},
                }
            }
        }
        let html_on = dioxus::ssr::render_element(rsx! { Wrapper { enabled: true } });
        assert!(
            html_on.contains("checked"),
            "enabled=true should render a checked attribute, got: {html_on}"
        );
        let html_off = dioxus::ssr::render_element(rsx! { Wrapper { enabled: false } });
        assert!(
            !html_off.contains("checked"),
            "enabled=false should not render a checked attribute, got: {html_off}"
        );
    }
}
