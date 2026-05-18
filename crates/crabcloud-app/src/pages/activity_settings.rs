//! Activity stream settings page — `/activity/settings`.
//!
//! Renders one labeled checkbox per `EventType` variant. Loads the
//! authed user's current overrides via `get_activity_settings()` and
//! writes individual updates back via `set_activity_setting()`. Toggling
//! a row is optimistic — we flip the local snapshot immediately, then
//! fire the server call in the background; on error we revert the row
//! and surface the failure in a dismissable banner.
//!
//! Default-true semantics: any event type not present in the response
//! renders as checked. This mirrors the read-side default in
//! `crabcloud-activity::ActivitySettings::is_streamed` — missing row =
//! opted in.

use crate::context::RequestContext;
use crate::pages::files::chrome;
use crate::server_fns::activity::{get_activity_settings, set_activity_setting};
use dioxus::prelude::*;
use std::collections::HashMap;

/// `(slug, label)` for each `EventType::as_str()` value. Order is the
/// rendered order on the page; groups file-* together then share-* then
/// version-*.
const EVENT_TYPES: &[(&str, &str)] = &[
    ("file_created", "File created"),
    ("file_updated", "File updated"),
    ("file_deleted", "File deleted"),
    ("file_renamed", "File renamed"),
    ("file_restored", "File restored from trash"),
    ("share_created", "Share created"),
    ("share_deleted", "Share removed"),
    ("version_restored", "Version restored"),
];

#[component]
pub fn ActivitySettingsPage(ctx: RequestContext) -> Element {
    let mut settings = use_signal::<HashMap<String, bool>>(HashMap::new);
    let mut loaded = use_signal::<bool>(|| false);
    let mut last_error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        spawn(async move {
            match get_activity_settings().await {
                Ok(rows) => {
                    let mut m = HashMap::new();
                    for r in rows {
                        m.insert(r.event_type, r.stream);
                    }
                    settings.set(m);
                }
                Err(e) => last_error.set(Some(format!("Couldn't load settings: {e}"))),
            }
            loaded.set(true);
        });
    });

    if ctx.user_id.is_none() {
        return rsx! {
            div { class: "files-page",
                chrome::TopBar { ctx: ctx.clone() }
                div { class: "files-body",
                    chrome::Sidebar {}
                    main { class: "files-main activity-settings-page",
                        h2 { "Please log in" }
                        p { a { href: "/login", "Log in" } }
                    }
                }
            }
        };
    }

    let on_toggle = move |(event_type, new_value): (String, bool)| {
        // Optimistic flip. On error we revert and surface the message.
        settings.with_mut(|m| {
            m.insert(event_type.clone(), new_value);
        });
        spawn(async move {
            match set_activity_setting(event_type.clone(), new_value).await {
                Ok(_) => last_error.set(None),
                Err(e) => {
                    settings.with_mut(|m| {
                        m.insert(event_type.clone(), !new_value);
                    });
                    last_error.set(Some(format!("Couldn't update {event_type}: {e}")));
                }
            }
        });
    };

    let on_dismiss_error = move |_evt: MouseEvent| last_error.set(None);

    let is_loaded = loaded();
    let snapshot = settings.read().clone();
    let error_message = last_error.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main activity-settings-page",
                    if let Some(msg) = error_message {
                        div {
                            class: "activity-banner activity-banner-error",
                            role: "alert",
                            span { class: "activity-banner-icon", aria_hidden: "true", "⚠" }
                            span { class: "activity-banner-msg", "{msg}" }
                            button {
                                r#type: "button",
                                class: "activity-banner-close",
                                aria_label: "Dismiss error",
                                onclick: on_dismiss_error,
                                "✕"
                            }
                        }
                    }
                    div { class: "activity-header",
                        h2 { "Activity settings" }
                        a { class: "activity-settings-link", href: "/activity", "Back to feed" }
                    }
                    p { class: "activity-settings-blurb",
                        "Pick which event types appear in your activity feed. Disabled events still happen — they're just hidden from your stream."
                    }
                    ActivitySettingsList {
                        loaded: is_loaded,
                        snapshot,
                        on_toggle,
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ActivitySettingsListProps {
    loaded: bool,
    snapshot: HashMap<String, bool>,
    on_toggle: EventHandler<(String, bool)>,
}

#[component]
fn ActivitySettingsList(props: ActivitySettingsListProps) -> Element {
    let ActivitySettingsListProps {
        loaded,
        snapshot,
        on_toggle,
    } = props;
    if !loaded {
        return rsx! { p { class: "activity-loading", "Loading…" } };
    }
    rsx! {
        ul { class: "activity-settings-list",
            for (slug, label) in EVENT_TYPES.iter() {
                {
                    let slug_str: String = (*slug).to_string();
                    let label_str: String = (*label).to_string();
                    // Default-true semantics: missing rows render as
                    // checked. Mirrors `ActivitySettings::is_streamed`.
                    let checked = *snapshot.get(&slug_str).unwrap_or(&true);
                    rsx! {
                        ActivitySettingRow {
                            key: "{slug_str}",
                            slug: slug_str,
                            label: label_str,
                            checked,
                            on_toggle,
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ActivitySettingRowProps {
    slug: String,
    label: String,
    checked: bool,
    on_toggle: EventHandler<(String, bool)>,
}

#[component]
fn ActivitySettingRow(props: ActivitySettingRowProps) -> Element {
    let ActivitySettingRowProps {
        slug,
        label,
        checked,
        on_toggle,
    } = props;
    let slug_for_handler = slug.clone();
    rsx! {
        li { class: "activity-settings-row",
            label { class: "activity-settings-label",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |evt| {
                        let new_val = evt.checked();
                        on_toggle.call((slug_for_handler.clone(), new_val));
                    },
                }
                span { class: "activity-settings-row-text", "{label}" }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// SSR snapshot: every event-type slug renders a row, and the
    /// human-readable label appears verbatim. Locks the
    /// label vocabulary so a copy change is a visible diff.
    #[test]
    fn all_event_type_rows_render() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                ActivitySettingsList {
                    loaded: true,
                    snapshot: HashMap::<String, bool>::new(),
                    on_toggle: move |_: (String, bool)| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        for (_, label) in EVENT_TYPES.iter() {
            assert!(
                html.contains(label),
                "expected label {label:?} in HTML, got: {html}"
            );
        }
        // All 8 rows. The class `activity-settings-row` also appears as
        // a substring of `activity-settings-row-text`, so match on the
        // `<li class="activity-settings-row"` opening to count rows
        // uniquely.
        let rows = html.matches("<li class=\"activity-settings-row\"").count();
        assert_eq!(
            rows,
            EVENT_TYPES.len(),
            "expected {} rows, got {}: {html}",
            EVENT_TYPES.len(),
            rows,
        );
    }

    /// Default-true semantics: when `get_activity_settings()` returns
    /// nothing, every checkbox renders checked. This is the load-bearing
    /// invariant for new users — the feed defaults to on across the board.
    /// Dioxus 0.7's SSR renders a true boolean attribute as
    /// `checked=true` and omits the attribute entirely when false, so
    /// we count both literal `checked=true` occurrences and the bare
    /// `<input type="checkbox"/>` pattern to be unambiguous.
    #[test]
    fn empty_settings_renders_all_checked() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                ActivitySettingsList {
                    loaded: true,
                    snapshot: HashMap::<String, bool>::new(),
                    on_toggle: move |_: (String, bool)| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        let on = html.matches("checked=true").count();
        let off = html.matches("<input type=\"checkbox\"/>").count();
        assert_eq!(
            on,
            EVENT_TYPES.len(),
            "expected every checkbox to render checked=true, got {on}: {html}",
        );
        assert_eq!(off, 0, "expected zero unchecked rows, got {off}: {html}");
    }

    /// Returned overrides render unchecked. Spot-check `file_updated`
    /// being explicitly off, with the rest still on.
    #[test]
    fn override_renders_unchecked() {
        let mut snapshot = HashMap::new();
        snapshot.insert("file_updated".to_string(), false);
        #[component]
        fn Wrapper(snapshot: HashMap<String, bool>) -> Element {
            rsx! {
                ActivitySettingsList {
                    loaded: true,
                    snapshot,
                    on_toggle: move |_: (String, bool)| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { snapshot } });
        let on = html.matches("checked=true").count();
        let off = html.matches("<input type=\"checkbox\"/>").count();
        assert_eq!(
            on,
            EVENT_TYPES.len() - 1,
            "expected 7 checked rows when one is overridden off, got {on}: {html}",
        );
        assert_eq!(
            off, 1,
            "expected exactly one bare unchecked input, got {off}: {html}",
        );
    }

    /// Pre-load (loaded=false) renders the spinner placeholder and not
    /// the list. Mirrors the activity-page loading-state assertion.
    #[test]
    fn pre_load_renders_loading_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                ActivitySettingsList {
                    loaded: false,
                    snapshot: HashMap::<String, bool>::new(),
                    on_toggle: move |_: (String, bool)| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("activity-loading"),
            "expected loading placeholder in HTML, got: {html}"
        );
        assert!(
            !html.contains("activity-settings-row"),
            "loading state must not render any rows, got: {html}"
        );
    }
}
