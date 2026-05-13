//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod breadcrumb;
pub mod chrome;
pub mod delete_modal;
pub mod list;
pub mod mkdir_row;
pub mod path;
pub mod row;
pub mod states;
pub mod toolbar;

#[cfg(feature = "server")]
pub mod ssr;

use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::delete_modal::DeleteModal;
use crate::pages::files::list::FileList;
use crate::pages::files::mkdir_row::MkdirRow;
use crate::server_fns::{delete, list_dir, mkdir, rename, FileEntry};
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    // SSR-only: redirect anonymous visitors to login with redirect_url.
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" {
                String::new()
            } else {
                path.clone()
            }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let user_id = ctx.user_id.clone().unwrap_or_default();
    let mut path_sig = use_signal(|| path.clone());
    let mut refresh = use_signal(|| 0u64);

    // Keep the route's `path` prop in sync with the signal. Back/forward
    // navigation re-renders this component with a new `path` prop; the
    // `use_reactive` adapter (Dioxus 0.7.9) re-fires the effect when the
    // captured non-reactive prop value changes.
    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
    }));

    // The list_dir server fn is only invoked from the client (wasm) side.
    // SSR renders the chrome + a pending skeleton; the resource starts in
    // the `Pending` state and is populated after hydration.
    let entries = use_resource(move || {
        let p = path_sig();
        let _ = refresh();
        async move { list_dir(p).await.map_err(|e| format!("{e}")) }
    });

    // Both Breadcrumb and FileList want to push a Files route for a target
    // path. `Navigator` is `Copy`, so we hand a fresh closure to each prop
    // instead of trying to alias one closure between two `EventHandler`
    // call-sites (which would move-once).
    let nav = use_navigator();
    let on_open_folder = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_navigate_breadcrumb = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_retry = move |_| refresh.set(refresh() + 1);

    // New mutation state — C5.
    let mut rename_target: Signal<Option<String>> = use_signal(|| None);
    let mut delete_target: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut mkdir_active = use_signal(|| false);
    let mut selection: Signal<HashSet<String>> = use_signal(HashSet::new);

    let on_toggle_select = move |p: String| {
        let mut s = selection();
        if !s.insert(p.clone()) {
            s.remove(&p);
        }
        selection.set(s);
    };
    let on_rename_start = move |p: String| rename_target.set(Some(p));
    let on_rename_cancel = move |_| rename_target.set(None);
    let on_rename_commit = move |(from, new_name): (String, String)| {
        spawn(async move {
            let to = match from.rsplit_once('/') {
                Some(("", _)) => format!("/{new_name}"),
                Some((parent, _)) => format!("{parent}/{new_name}"),
                None => format!("/{new_name}"),
            };
            let _ = rename(from, to).await;
            rename_target.set(None);
            refresh.set(refresh() + 1);
        });
    };
    let on_delete = move |p: String| delete_target.set(Some(vec![p]));
    let on_delete_confirm = move |_| {
        if let Some(paths) = delete_target() {
            spawn(async move {
                let _ = delete(paths).await;
                delete_target.set(None);
                refresh.set(refresh() + 1);
            });
        }
    };
    let on_delete_cancel = move |_| delete_target.set(None);

    let on_mkdir_start = move |_| mkdir_active.set(true);
    let on_mkdir_cancel = move |_| mkdir_active.set(false);
    let on_mkdir_commit = move |name: String| {
        let parent = path_sig();
        let new_path = if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        };
        spawn(async move {
            let _ = mkdir(new_path).await;
            mkdir_active.set(false);
            refresh.set(refresh() + 1);
        });
    };

    let entries_view: Option<Result<Vec<FileEntry>, String>> = entries.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    div { class: "files-toolbar",
                        button {
                            class: "files-tb-btn files-tb-primary",
                            onclick: on_mkdir_start,
                            "+ New folder"
                        }
                    }
                    Breadcrumb {
                        path: path_sig(),
                        on_navigate: on_navigate_breadcrumb,
                    }
                    if mkdir_active() {
                        table { class: "files-table",
                            tbody {
                                MkdirRow { on_commit: on_mkdir_commit, on_cancel: on_mkdir_cancel }
                            }
                        }
                    }
                    FileList {
                        entries: entries_view,
                        user_id: user_id.clone(),
                        selection: selection(),
                        rename_target: rename_target(),
                        on_open_folder,
                        on_toggle_select,
                        on_rename_start,
                        on_rename_commit,
                        on_rename_cancel,
                        on_delete,
                        on_retry,
                    }
                }
            }
            if let Some(paths) = delete_target() {
                DeleteModal {
                    paths,
                    on_cancel: on_delete_cancel,
                    on_confirm: on_delete_confirm,
                }
            }
        }
    }
}
