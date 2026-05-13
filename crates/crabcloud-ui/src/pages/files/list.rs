//! Tabular list of files in the current folder. Renders skeleton/empty/error
//! states based on the `Resource` state passed in from the page.

use crate::pages::files::row::FileRow;
use crate::pages::files::states::{EmptyFolder, LoadError, Skeleton};
use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[component]
pub fn FileList(
    entries: Option<Result<Vec<FileEntry>, String>>,
    user_id: String,
    on_open_folder: EventHandler<String>,
    on_retry: EventHandler<()>,
) -> Element {
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
                    }
                }
                tbody {
                    for e in es {
                        FileRow {
                            entry: e.clone(),
                            user_id: user_id.clone(),
                            on_open_folder,
                        }
                    }
                }
            }
        },
    }
}
