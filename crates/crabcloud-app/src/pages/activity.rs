//! Activity feed page — `/activity`.
//!
//! Lists recent activity for the authed user. Cursor pagination via a
//! "Load more" button at the bottom (MVP — true infinite scroll would
//! need an IntersectionObserver shim that isn't worth the wasm-side
//! plumbing for the first cut). Each row shows an event-type icon +
//! pre-rendered subject string + relative timestamp; coalesced rows
//! (`count > 1`) get a `+N more` badge.
//!
//! The page is read-only — no per-row mutations — so the in-flight
//! tracking the trash/versions pages carry isn't needed here. We still
//! mirror their dismissable error banner (`.activity-banner-*` is a
//! sibling of `.trash-banner-*`) so a failed list/load-more call
//! produces visible feedback instead of an empty list.

use crate::context::RequestContext;
use crate::pages::files::chrome;
use crate::server_fns::activity::{list_activity, ActivityRowDto, ListActivityResponse};
use dioxus::prelude::*;

#[component]
pub fn ActivityPage(ctx: RequestContext) -> Element {
    let mut entries = use_signal::<Vec<ActivityRowDto>>(Vec::new);
    let mut next_since = use_signal::<Option<i64>>(|| None);
    let mut loading = use_signal::<bool>(|| true);
    let mut loading_more = use_signal::<bool>(|| false);
    let mut last_error: Signal<Option<String>> = use_signal(|| None);

    // Initial load. Only runs once after first render — `use_effect`'s
    // dependency tracking sees no reactive reads inside the closure.
    use_effect(move || {
        spawn(async move {
            match list_activity(None, Some(30)).await {
                Ok(ListActivityResponse {
                    items,
                    next_since: ns,
                }) => {
                    entries.set(items);
                    next_since.set(ns);
                    last_error.set(None);
                }
                Err(e) => last_error.set(Some(format!("Couldn't load activity: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_load_more = move |_evt: MouseEvent| {
        if loading_more() {
            return;
        }
        let since = next_since();
        loading_more.set(true);
        spawn(async move {
            match list_activity(since, Some(30)).await {
                Ok(ListActivityResponse {
                    items,
                    next_since: ns,
                }) => {
                    entries.with_mut(|v| v.extend(items));
                    next_since.set(ns);
                    last_error.set(None);
                }
                Err(e) => last_error.set(Some(format!("Couldn't load more: {e}"))),
            }
            loading_more.set(false);
        });
    };

    let on_dismiss_error = move |_evt: MouseEvent| last_error.set(None);

    let is_loading = loading();
    let is_loading_more = loading_more();
    let has_more = next_since().is_some();
    let rows_snapshot = entries.read().clone();
    let error_message = last_error.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main activity-page",
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
                        h2 { "Activity" }
                        a { class: "activity-settings-link", href: "/activity/settings", "Settings" }
                    }
                    ActivityList {
                        loading: is_loading,
                        rows: rows_snapshot,
                    }
                    if !is_loading && has_more {
                        button {
                            r#type: "button",
                            class: "activity-load-more",
                            disabled: is_loading_more,
                            onclick: on_load_more,
                            if is_loading_more { "Loading…" } else { "Load more" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ActivityListProps {
    loading: bool,
    rows: Vec<ActivityRowDto>,
}

#[component]
fn ActivityList(props: ActivityListProps) -> Element {
    let ActivityListProps { loading, rows } = props;
    if loading {
        return rsx! { p { class: "activity-loading", "Loading…" } };
    }
    if rows.is_empty() {
        return rsx! { p { class: "activity-empty", "Nothing here yet." } };
    }
    rsx! {
        ul { class: "activity-list",
            for entry in rows.into_iter() {
                ActivityRowView { key: "{entry.id}", entry }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ActivityRowProps {
    entry: ActivityRowDto,
}

#[component]
fn ActivityRowView(props: ActivityRowProps) -> Element {
    let when = format_when(props.entry.last_seen_at);
    let icon = icon_for(&props.entry.event_type);
    let subject = props.entry.subject.clone();
    // Composed accessible label: a screen reader announces the row as a
    // single readable sentence rather than a stream of disjointed cells
    // (icon emoji, subject, count badge, timestamp). Mirrors the
    // trash-row pattern.
    let aria_label = if props.entry.count > 1 {
        format!(
            "{subject} (and {extra} more), {when}",
            extra = props.entry.count - 1
        )
    } else {
        format!("{subject}, {when}")
    };
    let show_count = props.entry.count > 1;
    let extra_count = props.entry.count - 1;

    rsx! {
        li { class: "activity-row", aria_label: "{aria_label}",
            span { class: "activity-row-icon", aria_hidden: "true", "{icon}" }
            span { class: "activity-row-subject", "{subject}" }
            if show_count {
                span { class: "activity-row-count", "+{extra_count} more" }
            }
            span { class: "activity-row-when", "{when}" }
        }
    }
}

/// Map an event-type slug to its row icon. Mirrors the 8 variants of
/// `crabcloud_activity::EventType::as_str()`; any unknown type falls
/// back to a generic bullet so a future event type rolled out behind a
/// flag still renders something.
fn icon_for(event_type: &str) -> &'static str {
    match event_type {
        "file_created" => "📄",
        "file_updated" => "✏",
        "file_deleted" => "🗑",
        "file_renamed" => "🏷",
        "file_restored" => "♻",
        "share_created" => "🔗",
        "share_deleted" => "✂",
        "share_unaccepted" => "🚫",
        "version_restored" => "🕘",
        _ => "•",
    }
}

/// Format a Unix-seconds timestamp as `YYYY-MM-DD HH:MM` (UTC). Same
/// rationale as `trash::format_deleted_at`: the activity view spans
/// hours / days / weeks simultaneously, so a single absolute format
/// composes better than "N hours ago" relative phrasing.
fn format_when(unix_secs: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(unix_secs.max(0), 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_secs.to_string())
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn row(
        id: i64,
        event_type: &str,
        subject: &str,
        count: i32,
        last_seen_at: i64,
    ) -> ActivityRowDto {
        ActivityRowDto {
            id,
            actor: "alice".into(),
            event_type: event_type.into(),
            subject_id: "test".into(),
            subject_params: serde_json::json!({}),
            subject: subject.into(),
            object_type: "file".into(),
            object_id: Some(1),
            occurred_at: last_seen_at,
            last_seen_at,
            count,
        }
    }

    /// SSR snapshot: the activity list rendered with two stubbed rows
    /// emits the subject strings, the per-row icons, and a wrapping
    /// `<ul class="activity-list">`. Locks the rendered shape so a
    /// class-name or copy change is a visible diff.
    ///
    /// EventHandler construction in `rsx!` goes through `Callback::new`,
    /// which needs a live Dioxus runtime; we don't have any handlers in
    /// the list view, but we still wrap in a component body for
    /// consistency with the trash/versions tests.
    #[test]
    fn renders_two_rows_with_icons_and_when() {
        let rows = vec![
            row(
                1,
                "file_created",
                "alice created notes.txt",
                1,
                1_700_000_000,
            ),
            row(
                2,
                "share_created",
                "alice shared photos with you",
                1,
                1_700_000_100,
            ),
        ];

        #[component]
        fn Wrapper(rows: Vec<ActivityRowDto>) -> Element {
            rsx! { ActivityList { loading: false, rows } }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("activity-list"),
            "expected wrapping list element, got: {html}"
        );
        assert!(
            html.contains("alice created notes.txt"),
            "expected first subject in HTML, got: {html}"
        );
        assert!(
            html.contains("alice shared photos with you"),
            "expected second subject in HTML, got: {html}"
        );
        assert!(
            html.contains("📄"),
            "expected file_created icon in HTML, got: {html}"
        );
        assert!(
            html.contains("🔗"),
            "expected share_created icon in HTML, got: {html}"
        );
        assert!(
            html.contains("2023-11-14 22:13"),
            "expected formatted UTC timestamp in HTML, got: {html}"
        );
    }

    /// Empty activity list renders the friendly placeholder copy and no
    /// `<ul>` element. Mirrors trash's empty-state assertion.
    #[test]
    fn empty_state_renders_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! { ActivityList { loading: false, rows: Vec::<ActivityRowDto>::new() } }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("Nothing here yet."),
            "expected empty placeholder copy in HTML, got: {html}"
        );
        assert!(
            !html.contains("<ul"),
            "empty activity should not render a list element, got: {html}"
        );
    }

    /// Loading state renders the spinner copy and no `<ul>` element.
    #[test]
    fn loading_state_renders_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! { ActivityList { loading: true, rows: Vec::<ActivityRowDto>::new() } }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("activity-loading"),
            "expected loading placeholder class in HTML, got: {html}"
        );
        assert!(
            !html.contains("<ul"),
            "loading activity should not render a list element, got: {html}"
        );
        assert!(
            !html.contains("Nothing here yet."),
            "loading state must not also show the empty copy, got: {html}"
        );
    }

    /// Coalesced rows (count > 1) render a `+N more` badge; non-coalesced
    /// rows do not. Locks the count-badge contract.
    #[test]
    fn coalesced_row_shows_count_badge() {
        let rows = vec![
            row(
                1,
                "file_updated",
                "alice updated notes.txt",
                5,
                1_700_000_000,
            ),
            row(2, "file_created", "alice created log.txt", 1, 1_700_000_100),
        ];

        #[component]
        fn Wrapper(rows: Vec<ActivityRowDto>) -> Element {
            rsx! { ActivityList { loading: false, rows } }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("activity-row-count"),
            "expected count-badge class on coalesced row, got: {html}"
        );
        assert!(
            html.contains("+4 more"),
            "expected '+4 more' badge text for count=5, got: {html}"
        );
        // The non-coalesced row's count slot should be absent — assert
        // by counting badge occurrences (exactly one).
        let badges = html.matches("activity-row-count").count();
        assert_eq!(
            badges, 1,
            "expected exactly one count badge across both rows, got {badges}: {html}"
        );
    }

    /// Each row carries a composed aria-label and marks the icon span
    /// aria-hidden so screen readers don't announce the emoji before the
    /// subject. Mirrors trash-row's accessibility assertions.
    #[test]
    fn row_emits_accessibility_attributes() {
        let rows = vec![row(
            1,
            "file_created",
            "alice created notes.txt",
            1,
            1_700_000_000,
        )];

        #[component]
        fn Wrapper(rows: Vec<ActivityRowDto>) -> Element {
            rsx! { ActivityList { loading: false, rows } }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("aria-hidden=\"true\""),
            "expected aria-hidden on decorative icon, got: {html}"
        );
        assert!(
            html.contains("aria-label=\"alice created notes.txt, 2023-11-14 22:13\""),
            "expected composed aria-label on the row, got: {html}"
        );
    }

    /// Coalesced rows fold the `+N more` count into the aria-label so a
    /// screen reader hears the multiplicity, not just the latest subject.
    #[test]
    fn coalesced_row_aria_label_includes_count() {
        let rows = vec![row(
            1,
            "file_updated",
            "alice updated notes.txt",
            3,
            1_700_000_000,
        )];

        #[component]
        fn Wrapper(rows: Vec<ActivityRowDto>) -> Element {
            rsx! { ActivityList { loading: false, rows } }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("aria-label=\"alice updated notes.txt (and 2 more), 2023-11-14 22:13\""),
            "expected aria-label to fold count into the announced sentence, got: {html}"
        );
    }

    /// Icon mapping covers every event-type slug — locks the visual
    /// vocabulary so a future event type doesn't silently fall back to
    /// the generic bullet without a deliberate icon choice.
    #[test]
    fn icon_for_covers_every_event_type() {
        assert_eq!(icon_for("file_created"), "📄");
        assert_eq!(icon_for("file_updated"), "✏");
        assert_eq!(icon_for("file_deleted"), "🗑");
        assert_eq!(icon_for("file_renamed"), "🏷");
        assert_eq!(icon_for("file_restored"), "♻");
        assert_eq!(icon_for("share_created"), "🔗");
        assert_eq!(icon_for("share_deleted"), "✂");
        assert_eq!(icon_for("version_restored"), "🕘");
        // Unknown types fall back to a bullet rather than panicking.
        assert_eq!(icon_for("not_a_real_event"), "•");
    }

    #[test]
    fn format_when_renders_utc_minute() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_when(1_700_000_000), "2023-11-14 22:13");
    }

    #[test]
    fn format_when_clamps_negative_to_zero() {
        // Negative timestamps shouldn't panic in chrono's timestamp_opt;
        // we clamp at zero so the rendered string stays in 1970.
        assert_eq!(format_when(-1), "1970-01-01 00:00");
    }
}
