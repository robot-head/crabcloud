//! Subject template rendering. Filled in Task A5.

pub fn render_subject(subject_id: &str, _params: &serde_json::Value) -> String {
    subject_id.to_string()
}
