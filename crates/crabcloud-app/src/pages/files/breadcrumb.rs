//! Breadcrumb showing the path from Home to the current folder. Each
//! segment is clickable and navigates back up the tree.

use crate::pages::files::path::path_to_segments;
use dioxus::prelude::*;

#[component]
pub fn Breadcrumb(path: String, on_navigate: EventHandler<String>) -> Element {
    let segments = path_to_segments(&path);
    let mut cumulative = String::from("/");
    let mut crumbs: Vec<(String, String)> = vec![("Home".to_string(), "/".to_string())];
    for seg in segments {
        if cumulative != "/" {
            cumulative.push('/');
        } else {
            cumulative.clear();
            cumulative.push('/');
        }
        if cumulative == "/" {
            cumulative = format!("/{seg}");
        } else {
            cumulative.push_str(&seg);
        }
        crumbs.push((seg, cumulative.clone()));
    }
    let last_index = crumbs.len().saturating_sub(1);
    rsx! {
        nav { class: "files-breadcrumb",
            for (i, (label, target)) in crumbs.iter().enumerate() {
                if i > 0 {
                    span { class: "files-breadcrumb-sep", "›" }
                }
                if i == last_index {
                    span { class: "files-breadcrumb-here", "{label}" }
                } else {
                    button {
                        class: "files-breadcrumb-link",
                        onclick: {
                            let t = target.clone();
                            move |_| on_navigate.call(t.clone())
                        },
                        "{label}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_yields_no_extra_segments() {
        let segs = path_to_segments("/");
        assert!(segs.is_empty());
    }

    #[test]
    fn one_level_path() {
        let segs = path_to_segments("/photos");
        assert_eq!(segs, vec!["photos".to_string()]);
    }

    #[test]
    fn two_level_path() {
        let segs = path_to_segments("/photos/vacation");
        assert_eq!(segs, vec!["photos".to_string(), "vacation".to_string()]);
    }
}
