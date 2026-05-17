//! Trash bin page — `/trash`.
//!
//! Reads trash entries via [`crate::server_fns::trash::list_trash`]. Each
//! row exposes Restore + Delete permanently buttons. The page header has
//! an Empty-trash button, which routes through an in-page confirm modal
//! (matches the sibling files-page delete-confirm pattern at
//! `pages::files::delete_modal`) so a single misclick doesn't wipe the
//! whole bin. On any successful mutation the resource is refetched (via
//! a monotonic refresh counter) so the affected rows disappear.
//!
//! Mutation handlers (restore / purge / empty) gate on a page-scoped
//! `busy` signal so a fast double-click can't dispatch the server fn
//! twice. Buttons disable while a mutation is in flight. Page-level
//! locking is intentionally coarser than per-row tracking — restoring
//! one entry while purging another is undefined-but-not-corrupting, and
//! the simpler shape keeps the snapshot tests tractable for MVP.
//!
//! No upload zone, no inline rename, no breadcrumb: the trash is a flat
//! per-user namespace by design (see SP12 spec §2).

use crate::context::RequestContext;
use crate::pages::files::chrome;
use crate::server_fns::trash::{
    empty_trash, list_trash, purge_trash, restore_trash, TrashEntryDto,
};
use dioxus::prelude::*;

#[component]
pub fn TrashPage(ctx: RequestContext) -> Element {
    let mut refresh = use_signal(|| 0u64);
    // Page-scoped in-flight guard. All three mutation handlers gate on
    // this so a fast double-click can't fire the same server fn twice
    // (and so the user gets visual feedback via disabled buttons).
    let mut busy = use_signal(|| false);
    // Toggles the empty-trash confirm modal. Mirrors the
    // `delete_target` pattern in `pages::files::mod` (an `Option`
    // there because the modal needs the list of paths; a bool here
    // because Empty has no payload).
    let mut confirm_empty = use_signal(|| false);

    // The list_trash server fn is only invoked from the client (wasm)
    // side. SSR renders the chrome + a pending skeleton; the resource
    // starts in the `Pending` state and is populated after hydration.
    let entries = use_resource(move || async move {
        let _ = refresh();
        list_trash().await.map_err(|e| format!("{e}"))
    });

    // Mutation handlers: fire-and-refetch. Errors are swallowed (matching
    // the `pages::files` convention — `let _ = …`) because the refetch is
    // the source of truth: if the server rejected the call, the row stays
    // in the list and the user sees that the action didn't take effect.
    // Future work could surface a toast / error banner; for now the
    // refetched list is the only feedback channel.
    let on_restore = move |id: i64| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let _ = restore_trash(id).await;
            refresh.set(refresh() + 1);
            busy.set(false);
        });
    };

    let on_purge = move |id: i64| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let _ = purge_trash(id).await;
            refresh.set(refresh() + 1);
            busy.set(false);
        });
    };

    let on_empty_request = move |_evt: MouseEvent| confirm_empty.set(true);
    let on_empty_cancel = move |_evt: MouseEvent| confirm_empty.set(false);
    let on_empty_confirmed = move |_evt: MouseEvent| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let _ = empty_trash().await;
            confirm_empty.set(false);
            refresh.set(refresh() + 1);
            busy.set(false);
        });
    };

    let entries_view: Option<Result<Vec<TrashEntryDto>, String>> = entries.read().clone();
    let is_busy = busy();
    let show_confirm = confirm_empty();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main trash-page",
                    div { class: "trash-header",
                        h2 { "Deleted files" }
                        button {
                            class: "trash-empty-btn",
                            disabled: is_busy,
                            onclick: on_empty_request,
                            "Empty trash"
                        }
                    }
                    TrashList {
                        entries: entries_view,
                        busy: is_busy,
                        on_restore,
                        on_purge,
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
                                class: "files-modal-cancel",
                                disabled: is_busy,
                                onclick: on_empty_cancel,
                                "Cancel"
                            }
                            button {
                                class: "files-modal-confirm",
                                disabled: is_busy,
                                onclick: on_empty_confirmed,
                                "Yes, empty trash"
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
    busy: bool,
    on_restore: EventHandler<i64>,
    on_purge: EventHandler<i64>,
}

#[component]
fn TrashList(props: TrashListProps) -> Element {
    let TrashListProps {
        entries,
        busy,
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
                    TrashRow {
                        key: "{entry.id}",
                        entry,
                        busy,
                        on_restore,
                        on_purge,
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

    rsx! {
        li { class: "trash-row",
            span { class: "trash-row-icon", "{icon}" }
            span { class: "trash-row-name", "{basename}" }
            span { class: "trash-row-location", "from {location}" }
            span { class: "trash-row-when", "{when}" }
            div { class: "trash-row-actions",
                button {
                    class: "trash-restore-btn",
                    disabled: busy,
                    onclick: move |_| on_restore.call(id),
                    "Restore"
                }
                button {
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
                    busy: false,
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
                    busy: false,
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
                    busy: false,
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
                    busy: false,
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

    /// When `busy` is true the per-row Restore / Delete buttons render
    /// the `disabled` attribute so the user sees the in-flight state
    /// and a fast double-click can't dispatch the server fn twice.
    #[test]
    fn busy_disables_row_action_buttons() {
        let rows = vec![TrashEntryDto {
            id: 1,
            basename: "notes.txt".into(),
            suffix: ".d1700000000".into(),
            location: "/".into(),
            deleted_at: 1_700_000_000,
            r#type: "file".into(),
        }];

        #[component]
        fn Wrapper(rows: Vec<TrashEntryDto>) -> Element {
            rsx! {
                TrashList {
                    entries: Some(Ok(rows)),
                    busy: true,
                    on_restore: move |_: i64| {},
                    on_purge: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("disabled"),
            "expected disabled attribute on row buttons when busy, got: {html}"
        );
    }

    #[test]
    fn format_deleted_at_renders_utc_minute() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_deleted_at(1_700_000_000), "2023-11-14 22:13");
    }
}
