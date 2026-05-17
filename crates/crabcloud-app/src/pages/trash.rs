//! Trash bin page — `/trash`.
//!
//! Reads trash entries via [`crate::server_fns::trash::list_trash`]. Each
//! row exposes Restore + Delete permanently buttons. The page header has
//! an Empty-trash button, which routes through an in-page confirm modal
//! (matches the sibling files-page delete-confirm pattern at
//! `pages::files::delete_modal`) so a single misclick doesn't wipe the
//! whole bin. The per-row Delete permanently button also routes through
//! a confirm modal — for consistency with the files-page per-row delete
//! and because trash is the last stop before permanent deletion. On any
//! successful mutation the resource is refetched (via a monotonic refresh
//! counter) so the affected rows disappear.
//!
//! Mutation handlers use per-row in-flight tracking: a page-scoped
//! `HashSet<i64>` of ids that have a pending Restore or Purge. Each row's
//! Restore / Delete buttons disable only when their own id is in flight,
//! so a user can fire restores on multiple rows in parallel. The
//! Empty-trash button keeps a separate page-level `emptying` lock since
//! it affects every row anyway.
//!
//! Failed mutations surface in a dismissable error banner at the top of
//! the page rather than being silently swallowed. The previous behaviour
//! (silent `let _ = …`) made a failed Restore look like the user hadn't
//! clicked at all — the row just stayed put with no feedback. The banner
//! is the simplest cross-platform (wasm + SSR) surface; no timer plumbing
//! needed, the user dismisses it with the ✕ button.
//!
//! No upload zone, no inline rename, no breadcrumb: the trash is a flat
//! per-user namespace by design (see SP12 spec §2).

use crate::context::RequestContext;
use crate::pages::files::chrome;
use crate::server_fns::trash::{
    empty_trash, list_trash, purge_trash, restore_trash, TrashEntryDto,
};
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn TrashPage(ctx: RequestContext) -> Element {
    let mut refresh = use_signal(|| 0u64);
    // Per-row in-flight tracking. A row's id sits in this set from the
    // moment its Restore or Purge handler fires until the server-fn
    // future resolves and the refetch is requested. Each row reads
    // `in_flight.contains(&self.id)` to decide whether to disable its
    // buttons, so restoring one entry doesn't lock out the others.
    let mut in_flight: Signal<HashSet<i64>> = use_signal(HashSet::new);
    // Page-level lock for the Empty-trash button — that mutation affects
    // every row, so per-row tracking doesn't help. Kept separate from
    // `in_flight` so a per-row mutation doesn't accidentally re-enable
    // (or disable) the Empty button.
    let mut emptying = use_signal(|| false);
    // Toggles the empty-trash confirm modal. Mirrors the
    // `delete_target` pattern in `pages::files::mod` (an `Option`
    // there because the modal needs the list of paths; a bool here
    // because Empty has no payload).
    let mut confirm_empty = use_signal(|| false);
    // Pending per-row purge confirm. Some(id) means the confirm modal is
    // open for that row; None means no purge pending. Mirrors the
    // `pages::files::delete_modal` flow so the trash's last-stop delete
    // also gets a confirm step (matches the files-page asymmetry the
    // earlier MVP punted on).
    let mut pending_purge: Signal<Option<i64>> = use_signal(|| None);
    // Last error from a failed mutation. Surfaced in a dismissable banner
    // at the top of the page; cleared when the user clicks ✕ or when the
    // next mutation succeeds. None = no banner shown.
    let mut last_error: Signal<Option<String>> = use_signal(|| None);

    // The list_trash server fn is only invoked from the client (wasm)
    // side. SSR renders the chrome + a pending skeleton; the resource
    // starts in the `Pending` state and is populated after hydration.
    let entries = use_resource(move || async move {
        let _ = refresh();
        list_trash().await.map_err(|e| format!("{e}"))
    });

    // Mutation handlers: fire-and-refetch. On success the refetched list
    // is the source of truth (the affected row disappears). On error we
    // populate `last_error` so the banner appears at the top of the page
    // — the row stays put because the server-side state didn't change,
    // and the banner explains why.
    let on_restore = move |id: i64| {
        if in_flight.read().contains(&id) {
            return;
        }
        in_flight.write().insert(id);
        spawn(async move {
            match restore_trash(id).await {
                Ok(_) => last_error.set(None),
                Err(e) => last_error.set(Some(format!("Failed to restore: {e}"))),
            }
            in_flight.write().remove(&id);
            refresh.set(refresh() + 1);
        });
    };

    let on_purge_request = move |id: i64| pending_purge.set(Some(id));
    let on_purge_cancel = move |_evt: MouseEvent| pending_purge.set(None);
    let on_purge_confirmed = move |_evt: MouseEvent| {
        let Some(id) = pending_purge() else {
            return;
        };
        if in_flight.read().contains(&id) {
            return;
        }
        in_flight.write().insert(id);
        pending_purge.set(None);
        spawn(async move {
            match purge_trash(id).await {
                Ok(_) => last_error.set(None),
                Err(e) => last_error.set(Some(format!("Failed to delete: {e}"))),
            }
            in_flight.write().remove(&id);
            refresh.set(refresh() + 1);
        });
    };

    let on_empty_request = move |_evt: MouseEvent| confirm_empty.set(true);
    let on_empty_cancel = move |_evt: MouseEvent| confirm_empty.set(false);
    let on_empty_confirmed = move |_evt: MouseEvent| {
        if emptying() {
            return;
        }
        emptying.set(true);
        spawn(async move {
            match empty_trash().await {
                Ok(_) => last_error.set(None),
                Err(e) => last_error.set(Some(format!("Failed to empty trash: {e}"))),
            }
            confirm_empty.set(false);
            refresh.set(refresh() + 1);
            emptying.set(false);
        });
    };

    let on_dismiss_error = move |_evt: MouseEvent| last_error.set(None);

    let entries_view: Option<Result<Vec<TrashEntryDto>, String>> = entries.read().clone();
    let in_flight_snapshot: HashSet<i64> = in_flight.read().clone();
    let is_emptying = emptying();
    let show_confirm = confirm_empty();
    let purge_target = pending_purge();
    let error_message = last_error.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main trash-page",
                    if let Some(msg) = error_message {
                        div {
                            class: "trash-banner trash-banner-error",
                            role: "alert",
                            span { class: "trash-banner-icon", aria_hidden: "true", "⚠" }
                            span { class: "trash-banner-text", "{msg}" }
                            button {
                                r#type: "button",
                                class: "trash-banner-close",
                                aria_label: "Dismiss error",
                                onclick: on_dismiss_error,
                                "✕"
                            }
                        }
                    }
                    div { class: "trash-header",
                        h2 { "Deleted files" }
                        button {
                            r#type: "button",
                            class: "trash-empty-btn",
                            disabled: is_emptying,
                            onclick: on_empty_request,
                            "Empty trash"
                        }
                    }
                    TrashList {
                        entries: entries_view,
                        in_flight: in_flight_snapshot,
                        on_restore,
                        on_purge: on_purge_request,
                    }
                }
            }
            if show_confirm {
                div { class: "files-modal-backdrop", onclick: on_empty_cancel,
                    div {
                        class: "files-modal",
                        onclick: move |e: MouseEvent| e.stop_propagation(),
                        div { class: "files-modal-title", "Empty trash?" }
                        div { class: "files-modal-body",
                            "This permanently deletes every item in the trash. This cannot be undone."
                        }
                        div { class: "files-modal-actions",
                            button {
                                r#type: "button",
                                class: "files-modal-cancel",
                                disabled: is_emptying,
                                onclick: on_empty_cancel,
                                "Cancel"
                            }
                            button {
                                r#type: "button",
                                class: "files-modal-confirm",
                                disabled: is_emptying,
                                onclick: on_empty_confirmed,
                                "Yes, empty trash"
                            }
                        }
                    }
                }
            }
            if purge_target.is_some() {
                div { class: "files-modal-backdrop", onclick: on_purge_cancel,
                    div {
                        class: "files-modal",
                        onclick: move |e: MouseEvent| e.stop_propagation(),
                        div { class: "files-modal-title", "Delete permanently?" }
                        div { class: "files-modal-body",
                            "This item will be permanently deleted. This cannot be undone."
                        }
                        div { class: "files-modal-actions",
                            button {
                                r#type: "button",
                                class: "files-modal-cancel",
                                onclick: on_purge_cancel,
                                "Cancel"
                            }
                            button {
                                r#type: "button",
                                class: "files-modal-confirm",
                                onclick: on_purge_confirmed,
                                "Delete"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct TrashListProps {
    entries: Option<Result<Vec<TrashEntryDto>, String>>,
    in_flight: HashSet<i64>,
    on_restore: EventHandler<i64>,
    on_purge: EventHandler<i64>,
}

#[component]
fn TrashList(props: TrashListProps) -> Element {
    let TrashListProps {
        entries,
        in_flight,
        on_restore,
        on_purge,
    } = props;
    match entries {
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            p { class: "trash-empty", "Nothing in trash." }
        },
        Some(Ok(rows)) => rsx! {
            ul { class: "trash-list",
                for entry in rows.into_iter() {
                    {
                        let row_busy = in_flight.contains(&entry.id);
                        rsx! {
                            TrashRow {
                                key: "{entry.id}",
                                entry,
                                busy: row_busy,
                                on_restore,
                                on_purge,
                            }
                        }
                    }
                }
            }
        },
        // Raw `ServerFnError` text isn't useful to a user (it's usually
        // a transport/serde diagnostic). Show a friendly placeholder.
        Some(Err(_)) => rsx! {
            p { class: "trash-error", "Couldn't load trash. Try again." }
        },
        None => rsx! { p { class: "trash-loading", "Loading…" } },
    }
}

#[derive(Props, Clone, PartialEq)]
struct TrashRowProps {
    entry: TrashEntryDto,
    busy: bool,
    on_restore: EventHandler<i64>,
    on_purge: EventHandler<i64>,
}

#[component]
fn TrashRow(props: TrashRowProps) -> Element {
    let TrashRowProps {
        entry,
        busy,
        on_restore,
        on_purge,
    } = props;
    let id = entry.id;
    let basename = entry.basename.clone();
    let location = entry.location.clone();
    let when = format_deleted_at(entry.deleted_at);
    let icon = if entry.r#type == "dir" {
        "📁"
    } else {
        "📄"
    };
    // Composed accessible label: a screen reader announces the row as a
    // single readable sentence instead of a stream of disjointed cells
    // (icon emoji, name, "from /albums", timestamp). Mirrors the sibling
    // files-page convention of letting the visual columns each carry
    // their own text while folding the row-level meaning into aria-label.
    let aria_label = format!("{basename}, originally in {location}, deleted {when}");

    rsx! {
        li { class: "trash-row", aria_label: "{aria_label}",
            // The emoji is decorative — the basename and aria-label already
            // tell the user what kind of entry this is. Hide from AT so the
            // row isn't announced as "page sheet, notes.txt, …".
            span { class: "trash-row-icon", aria_hidden: "true", "{icon}" }
            span { class: "trash-row-name", "{basename}" }
            span { class: "trash-row-location", "from {location}" }
            span { class: "trash-row-when", "{when}" }
            div { class: "trash-row-actions",
                button {
                    r#type: "button",
                    class: "trash-restore-btn",
                    disabled: busy,
                    onclick: move |_| on_restore.call(id),
                    "Restore"
                }
                button {
                    r#type: "button",
                    class: "trash-purge-btn",
                    disabled: busy,
                    onclick: move |_| on_purge.call(id),
                    "Delete permanently"
                }
            }
        }
    }
}

/// Format a Unix-seconds timestamp as `YYYY-MM-DD HH:MM` (UTC). The
/// trash view is intentionally low-density — the deleted-at column is
/// for orientation, not exact bookkeeping — so we render the same
/// format on every row instead of "N hours ago" relative phrasing,
/// which (unlike the files list) wouldn't compose well with rows that
/// can be hours, days, or weeks old simultaneously.
fn format_deleted_at(unix_secs: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_secs.to_string())
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// SSR snapshot: the trash list rendered with two stubbed entries
    /// emits the basenames, the "from <location>" hint, and the
    /// per-row Restore / Delete permanently button text. Locks the
    /// rendered shape so a copy or class-name change is a visible diff.
    ///
    /// EventHandler construction in `rsx!` goes through `Callback::new`,
    /// which needs a live Dioxus runtime. Wrapping the rsx call in a
    /// component body (`Wrapper`) defers the conversion to render time,
    /// when the VirtualDom has its runtime up.
    #[test]
    fn renders_two_entries_with_action_buttons() {
        let rows = vec![
            TrashEntryDto {
                id: 1,
                basename: "notes.txt".into(),
                suffix: ".d1700000000".into(),
                location: "/".into(),
                deleted_at: 1_700_000_000,
                r#type: "file".into(),
            },
            TrashEntryDto {
                id: 2,
                basename: "photos".into(),
                suffix: ".d1700000100".into(),
                location: "/albums".into(),
                deleted_at: 1_700_000_100,
                r#type: "dir".into(),
            },
        ];

        #[component]
        fn Wrapper(rows: Vec<TrashEntryDto>) -> Element {
            rsx! {
                TrashList {
                    entries: Some(Ok(rows)),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }

        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("notes.txt"),
            "expected notes.txt basename in HTML, got: {html}"
        );
        assert!(
            html.contains("photos"),
            "expected photos basename in HTML, got: {html}"
        );
        assert!(
            html.contains("from /albums"),
            "expected 'from /albums' location hint in HTML, got: {html}"
        );
        assert!(
            html.contains("Restore"),
            "expected Restore button text in HTML, got: {html}"
        );
        assert!(
            html.contains("Delete permanently"),
            "expected 'Delete permanently' button text in HTML, got: {html}"
        );
    }

    /// SSR snapshot: an empty trash renders the "Nothing in trash."
    /// placeholder (and no list element).
    #[test]
    fn empty_state_renders_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                TrashList {
                    entries: Some(Ok(Vec::<TrashEntryDto>::new())),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("Nothing in trash."),
            "expected empty placeholder in HTML, got: {html}"
        );
        assert!(
            !html.contains("<ul"),
            "empty trash should not render a list element, got: {html}"
        );
    }

    /// Directory rows render the folder emoji; file rows render the
    /// file emoji. Locks the per-type icon mapping.
    #[test]
    fn dir_row_uses_folder_icon() {
        let rows = vec![TrashEntryDto {
            id: 7,
            basename: "albums".into(),
            suffix: ".d1700000000".into(),
            location: "/".into(),
            deleted_at: 1_700_000_000,
            r#type: "dir".into(),
        }];

        #[component]
        fn Wrapper(rows: Vec<TrashEntryDto>) -> Element {
            rsx! {
                TrashList {
                    entries: Some(Ok(rows)),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("📁"),
            "expected folder emoji for dir row, got: {html}"
        );
        assert!(
            !html.contains("📄"),
            "dir-only row should not render the file emoji, got: {html}"
        );
    }

    /// Error state renders the friendly placeholder copy, not the raw
    /// `ServerFnError` text. The raw error (usually a transport / serde
    /// string like "fetch failed: ...") isn't useful to the user, so the
    /// view collapses it to a single retryable line. Diagnostics are the
    /// server log's job.
    #[test]
    fn error_state_renders_friendly_copy() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                TrashList {
                    entries: Some(Err::<Vec<TrashEntryDto>, String>(
                        "internal: deadline exceeded".into(),
                    )),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        // SSR escapes the apostrophe to &#39; in the rendered HTML, so
        // match on the unambiguous tail of the copy instead of the full
        // sentence (and on the class name to anchor the placeholder).
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("class=\"trash-error\""),
            "expected trash-error placeholder in HTML, got: {html}"
        );
        assert!(
            html.contains("load trash. Try again."),
            "expected friendly error copy tail in HTML, got: {html}"
        );
        assert!(
            !html.contains("deadline exceeded"),
            "error placeholder must not leak the raw error string, got: {html}"
        );
    }

    /// Per-row in-flight tracking: only the row whose id is in the
    /// `in_flight` set renders its buttons disabled. The other row stays
    /// interactive so the user can fire mutations in parallel.
    #[test]
    fn per_row_in_flight_disables_only_matching_row() {
        let rows = vec![
            TrashEntryDto {
                id: 1,
                basename: "row-one.txt".into(),
                suffix: ".d1700000000".into(),
                location: "/".into(),
                deleted_at: 1_700_000_000,
                r#type: "file".into(),
            },
            TrashEntryDto {
                id: 2,
                basename: "row-two.txt".into(),
                suffix: ".d1700000100".into(),
                location: "/".into(),
                deleted_at: 1_700_000_100,
                r#type: "file".into(),
            },
        ];

        #[component]
        fn Wrapper(rows: Vec<TrashEntryDto>) -> Element {
            let mut set = HashSet::<i64>::new();
            set.insert(1);
            rsx! {
                TrashList {
                    entries: Some(Ok(rows)),
                    in_flight: set,
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        // Both rows should render. The disabled attribute count is the
        // load-bearing assertion: with only id=1 in flight, exactly the
        // two buttons in that row should be disabled.
        assert!(
            html.contains("row-one.txt") && html.contains("row-two.txt"),
            "expected both row basenames in HTML, got: {html}"
        );
        let disabled_count = html.matches("disabled").count();
        assert_eq!(
            disabled_count, 2,
            "expected exactly 2 disabled buttons (one row, two buttons), got {disabled_count}: {html}"
        );
    }

    /// Accessibility: the row carries a composed aria-label and the icon
    /// span is marked aria-hidden so screen readers don't announce the
    /// emoji before the basename. Action buttons render with explicit
    /// `type="button"` so they can't be hijacked by a future surrounding
    /// form context.
    #[test]
    fn row_emits_accessibility_attributes() {
        let rows = vec![TrashEntryDto {
            id: 1,
            basename: "notes.txt".into(),
            suffix: ".d1700000000".into(),
            location: "/inbox".into(),
            deleted_at: 1_700_000_000,
            r#type: "file".into(),
        }];

        #[component]
        fn Wrapper(rows: Vec<TrashEntryDto>) -> Element {
            rsx! {
                TrashList {
                    entries: Some(Ok(rows)),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("aria-hidden=\"true\""),
            "expected aria-hidden on decorative icon, got: {html}"
        );
        assert!(
            html.contains("aria-label=\"notes.txt, originally in /inbox, deleted"),
            "expected composed aria-label on the row, got: {html}"
        );
        assert!(
            html.contains("type=\"button\""),
            "expected explicit type=\"button\" on action buttons, got: {html}"
        );
    }

    #[test]
    fn format_deleted_at_renders_utc_minute() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_deleted_at(1_700_000_000), "2023-11-14 22:13");
    }
}
