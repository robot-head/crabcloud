//! Trash bin page — `/trash`.
//!
//! Reads trash entries via [`crate::server_fns::trash::list_trash`]. Each
//! row exposes Restore + Delete permanently buttons. The page header has
//! an Empty-trash button. On any successful mutation the resource is
//! refetched (via a monotonic refresh counter) so the affected rows
//! disappear.
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
        spawn(async move {
            let _ = restore_trash(id).await;
            refresh.set(refresh() + 1);
        });
    };

    let on_purge = move |id: i64| {
        spawn(async move {
            let _ = purge_trash(id).await;
            refresh.set(refresh() + 1);
        });
    };

    let on_empty = move |_evt: MouseEvent| {
        spawn(async move {
            let _ = empty_trash().await;
            refresh.set(refresh() + 1);
        });
    };

    let entries_view: Option<Result<Vec<TrashEntryDto>, String>> = entries.read().clone();

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
                            onclick: on_empty,
                            "Empty trash"
                        }
                    }
                    TrashList {
                        entries: entries_view,
                        on_restore,
                        on_purge,
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct TrashListProps {
    entries: Option<Result<Vec<TrashEntryDto>, String>>,
    on_restore: EventHandler<i64>,
    on_purge: EventHandler<i64>,
}

#[component]
fn TrashList(props: TrashListProps) -> Element {
    let TrashListProps {
        entries,
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
                        on_restore,
                        on_purge,
                    }
                }
            }
        },
        Some(Err(e)) => rsx! { p { class: "trash-error", "Error: {e}" } },
        None => rsx! { p { class: "trash-loading", "Loading…" } },
    }
}

#[derive(Props, Clone, PartialEq)]
struct TrashRowProps {
    entry: TrashEntryDto,
    on_restore: EventHandler<i64>,
    on_purge: EventHandler<i64>,
}

#[component]
fn TrashRow(props: TrashRowProps) -> Element {
    let TrashRowProps {
        entry,
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
                    onclick: move |_| on_restore.call(id),
                    "Restore"
                }
                button {
                    class: "trash-purge-btn",
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

    #[test]
    fn format_deleted_at_renders_utc_minute() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_deleted_at(1_700_000_000), "2023-11-14 22:13");
    }
}
