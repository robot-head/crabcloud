//! Tabular list of files in the current folder. Renders skeleton/empty/error
//! states based on the `Resource` state passed in from the page.

use crate::pages::files::row::FileRow;
use crate::pages::files::states::{EmptyFolder, LoadError, Skeleton};
use crate::server_fns::FileEntry;
use dioxus::prelude::*;
use std::collections::HashSet;

#[derive(Props, Clone, PartialEq)]
pub struct FileListProps {
    pub entries: Option<Result<Vec<FileEntry>, String>>,
    pub user_id: String,
    pub selection: HashSet<String>,
    pub rename_target: Option<String>,
    pub on_open_folder: EventHandler<String>,
    pub on_toggle_select: EventHandler<String>,
    pub on_rename_start: EventHandler<String>,
    pub on_rename_commit: EventHandler<(String, String)>,
    pub on_rename_cancel: EventHandler<()>,
    pub on_delete: EventHandler<String>,
    pub on_share: EventHandler<String>,
    pub on_retry: EventHandler<()>,
}

#[component]
pub fn FileList(props: FileListProps) -> Element {
    let FileListProps {
        entries,
        user_id,
        selection,
        rename_target,
        on_open_folder,
        on_toggle_select,
        on_rename_start,
        on_rename_commit,
        on_rename_cancel,
        on_delete,
        on_share,
        on_retry,
    } = props;

    match entries {
        None => rsx! { Skeleton {} },
        Some(Err(msg)) => rsx! { LoadError { reason: msg, on_retry } },
        Some(Ok(es)) if es.is_empty() => rsx! { EmptyFolder {} },
        Some(Ok(es)) => rsx! {
            table { class: "files-table",
                thead {
                    tr {
                        th { class: "files-th files-check" }
                        th { class: "files-th", "Name" }
                        th { class: "files-th files-size", "Size" }
                        th { class: "files-th files-mtime", "Modified" }
                        th { class: "files-th files-actions" }
                    }
                }
                tbody {
                    for e in es {
                        FileRow {
                            entry: e.clone(),
                            user_id: user_id.clone(),
                            rename_active: rename_target.as_deref() == Some(&e.path),
                            selected: selection.contains(&e.path),
                            on_open_folder,
                            on_toggle_select,
                            on_rename_start,
                            on_rename_commit,
                            on_rename_cancel,
                            on_delete,
                            on_share,
                        }
                    }
                }
            }
        },
    }
}
