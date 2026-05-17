//! Subject template rendering.
//!
//! Each `subject_id` maps to an English template with `{key}` placeholders.
//! Unknown subject_ids fall back to the id verbatim with a `tracing::warn`.

use serde_json::Value;

/// Render a subject_id + params into an English string.
pub fn render_subject(subject_id: &str, params: &Value) -> String {
    let template = match template_for(subject_id) {
        Some(t) => t,
        None => {
            tracing::warn!(
                subject_id,
                "activity: subject_id has no template; returning verbatim"
            );
            return subject_id.to_string();
        }
    };
    interpolate(template, params)
}

fn template_for(subject_id: &str) -> Option<&'static str> {
    Some(match subject_id {
        "file_created_by" => "{actor} created {file}",
        "file_created_you" => "You created {file}",
        "file_updated_by" => "{actor} updated {file}",
        "file_updated_you" => "You updated {file}",
        "file_deleted_by" => "{actor} deleted {file}",
        "file_deleted_you" => "You deleted {file}",
        "file_renamed_by" => "{actor} renamed {old} to {file}",
        "file_renamed_you" => "You renamed {old} to {file}",
        "file_restored_by" => "{actor} restored {file} from the trash",
        "file_restored_you" => "You restored {file} from the trash",
        "share_created_by" => "{actor} shared {file} with you",
        "share_created_you" => "You shared {file} with {recipient}",
        "share_deleted_by" => "{actor} unshared {file} from you",
        "share_deleted_you" => "You unshared {file} from {recipient}",
        "version_restored_by" => "{actor} restored a previous version of {file}",
        "version_restored_you" => "You restored a previous version of {file}",
        // Public-link variants (actor = "")
        "file_created_link" => "Someone created {file} via a shared link",
        "file_updated_link" => "Someone updated {file} via a shared link",
        "file_deleted_link" => "Someone deleted {file} via a shared link",
        _ => return None,
    })
}

/// Replace every `{key}` placeholder with the corresponding string from
/// `params` (JSON object). Missing keys → empty string.
fn interpolate(template: &str, params: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '{' {
            out.push(ch);
            continue;
        }
        // Collect the key until '}'.
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '}' {
                chars.next();
                break;
            }
            key.push(c);
            chars.next();
        }
        let value = params.get(&key).and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interpolate_simple() {
        assert_eq!(
            interpolate("hello {name}", &json!({"name": "alice"})),
            "hello alice"
        );
    }

    #[test]
    fn interpolate_missing_key_is_empty() {
        assert_eq!(interpolate("{a} {b}", &json!({"a": "x"})), "x ");
    }

    #[test]
    fn render_known_subject() {
        assert_eq!(
            render_subject(
                "file_updated_by",
                &json!({"actor": "alice", "file": "x.txt"})
            ),
            "alice updated x.txt"
        );
    }

    #[test]
    fn render_unknown_returns_verbatim() {
        let r = render_subject("unknown_template", &json!({}));
        assert_eq!(r, "unknown_template");
    }
}
