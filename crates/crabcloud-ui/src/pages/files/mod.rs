//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod breadcrumb;
pub mod chrome;
pub mod delete_modal;
pub mod list;
pub mod mkdir_row;
pub mod path;
pub mod progress_strip;
pub mod row;
pub mod states;
pub mod toolbar;
pub mod upload;

#[cfg(feature = "server")]
pub mod ssr;

use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::delete_modal::DeleteModal;
use crate::pages::files::list::FileList;
use crate::pages::files::mkdir_row::MkdirRow;
use crate::pages::files::toolbar::Toolbar;
use crate::server_fns::{delete, list_dir, mkdir, move_paths, rename, FileEntry};
use dioxus::prelude::*;
use std::collections::HashSet;

#[derive(Clone, PartialEq)]
pub struct Clipboard {
    pub source_dir: String,
    pub paths: Vec<String>,
}

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
    let mut selection: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Keep the route's `path` prop in sync with the signal. Back/forward
    // navigation re-renders this component with a new `path` prop; the
    // `use_reactive` adapter (Dioxus 0.7.9) re-fires the effect when the
    // captured non-reactive prop value changes. Selection is also cleared
    // because it's path-scoped; clipboard intentionally persists across
    // navigation so a user can cut here and paste in a different folder.
    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
        selection.set(HashSet::new());
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

    // Cut/paste state — D3. The clipboard intentionally outlives a single
    // folder view so the user can navigate from source to destination
    // between cut and paste.
    let mut clipboard: Signal<Option<Clipboard>> = use_signal(|| None);

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

    // Cut/paste + bulk selection handlers — D3.
    let on_cut = move |_| {
        let s = selection();
        if !s.is_empty() {
            clipboard.set(Some(Clipboard {
                source_dir: path_sig(),
                paths: s.into_iter().collect(),
            }));
            selection.set(HashSet::new());
        }
    };
    let on_clear_selection = move |_| selection.set(HashSet::new());
    let on_clear_clipboard = move |_| clipboard.set(None);
    let on_delete_selection = move |_| {
        let s = selection();
        if !s.is_empty() {
            delete_target.set(Some(s.into_iter().collect()));
        }
    };
    let on_paste = move |_| {
        if let Some(cb) = clipboard() {
            let dest = path_sig();
            if dest == cb.source_dir {
                return;
            }
            spawn(async move {
                let _ = move_paths(cb.paths, dest).await;
                clipboard.set(None);
                refresh.set(refresh() + 1);
            });
        }
    };

    let entries_view: Option<Result<Vec<FileEntry>, String>> = entries.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    Toolbar {
                        selection_count: selection().len(),
                        clipboard_count: clipboard().as_ref().map(|c| c.paths.len()).unwrap_or(0),
                        clipboard_source: clipboard().as_ref().map(|c| c.source_dir.clone()),
                        can_paste: clipboard()
                            .as_ref()
                            .map(|c| c.source_dir != path_sig())
                            .unwrap_or(false),
                        on_new_folder: on_mkdir_start,
                        on_upload: move |_| {}, // wired in Batch E
                        on_cut,
                        on_delete_selection,
                        on_clear_selection,
                        on_paste,
                        on_clear_clipboard,
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
