//! Tera template loader (embedded via rust-embed) + render_template.

use crate::envelope::{EventType, MailEnvelope};
use crate::error::MailError;
use rust_embed::RustEmbed;
use serde::Serialize;
use std::sync::OnceLock;
use tera::{Context, Tera};

/// Embedded template files. `crates/crabcloud-mail/src/templates/` is
/// hardcoded; operator overrides at `<datadir>/mail-templates/` is a
/// future feature.
#[derive(RustEmbed)]
#[folder = "src/templates"]
struct EmbeddedTemplates;

/// Process-wide Tera instance, loaded once on first use.
fn tera() -> &'static Tera {
    static T: OnceLock<Tera> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tera::default();
        // Auto-escape is keyed on suffix match; register before adding
        // templates so the parsed templates pick up the policy.
        // Tera 1.x: `autoescape_on` takes a `Vec<&str>` of filename suffixes;
        // a template name ending in any of these strings has HTML escaping
        // applied. Templates without a matching suffix render verbatim.
        t.autoescape_on(vec![".html", ".htm"]);
        for name in EmbeddedTemplates::iter() {
            let bytes = EmbeddedTemplates::get(&name).expect("rust-embed name must resolve");
            let body = std::str::from_utf8(&bytes.data).expect("templates are utf8");
            t.add_raw_template(&name, body).expect("template parse");
        }
        t
    })
}

/// Render context. Event-specific fields are passed in via `event_specific`.
/// `lang` selects the i18n locale; subject lookup is a static `match` for
/// now (i18n integration lands in Batch C).
#[derive(Debug, Clone, Serialize)]
pub struct TemplateContext {
    pub lang: String,
    pub instance_url: String,
    pub recipient_display_name: String,
    pub recipient_email: String,
    pub event_specific: serde_json::Value,
}

/// Tera's `Display` only shows the top-level message; walk `source()` for
/// the parser/runtime details needed to debug template breakage.
fn fmt_tera_error(e: &tera::Error) -> String {
    let mut out = format!("{e}");
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(cur) = src {
        out.push_str(" -> ");
        out.push_str(&cur.to_string());
        src = cur.source();
    }
    out
}

/// Render a fully-populated MailEnvelope for the given event type.
pub fn render_template(event: EventType, ctx: TemplateContext) -> Result<MailEnvelope, MailError> {
    let html_name = format!("{}.html", event.as_str());
    let text_name = format!("{}.txt", event.as_str());

    // Subject lookup: each event has a static subject for now (i18n TBD).
    let subject = match event {
        EventType::ShareCreated => "You've received a new share",
        EventType::LinkEmailed => "A file has been shared with you",
        EventType::ExpirationWarning => "Your share link is expiring soon",
    }
    .to_string();

    let mut tera_ctx = Context::new();
    tera_ctx.insert("instance_url", &ctx.instance_url);
    tera_ctx.insert("recipient_display_name", &ctx.recipient_display_name);
    tera_ctx.insert("recipient_email", &ctx.recipient_email);
    tera_ctx.insert("lang", &ctx.lang);
    tera_ctx.insert("subject", &subject);
    if let serde_json::Value::Object(map) = &ctx.event_specific {
        for (k, v) in map.iter() {
            tera_ctx.insert(k.clone(), v);
        }
    }

    let html_body = tera()
        .render(&html_name, &tera_ctx)
        .map_err(|e| MailError::Render(format!("{html_name}: {}", fmt_tera_error(&e))))?;
    let text_body = tera()
        .render(&text_name, &tera_ctx)
        .map_err(|e| MailError::Render(format!("{text_name}: {}", fmt_tera_error(&e))))?;

    Ok(MailEnvelope {
        recipient: ctx.recipient_email,
        subject,
        html_body,
        text_body,
        event_type: event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(event_specific: serde_json::Value) -> TemplateContext {
        TemplateContext {
            lang: "en".to_string(),
            instance_url: "https://crabcloud.example".to_string(),
            recipient_display_name: "Bob".to_string(),
            recipient_email: "bob@example.com".to_string(),
            event_specific,
        }
    }

    #[test]
    fn share_created_renders_both_formats() {
        let env = render_template(
            EventType::ShareCreated,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "Vacation",
            })),
        )
        .unwrap();
        assert_eq!(env.recipient, "bob@example.com");
        assert!(env.subject.contains("share") || env.subject.contains("Share"));
        assert!(env.html_body.contains("Alice"));
        assert!(env.html_body.contains("Vacation"));
        assert!(env.text_body.contains("Alice"));
        assert!(env.text_body.contains("Vacation"));
    }

    #[test]
    fn link_emailed_includes_link_and_omits_password() {
        let env = render_template(
            EventType::LinkEmailed,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "Photos",
                "link_url": "https://crabcloud.example/s/AbCd123Xyz0789Q",
                "password_protected": true,
                // Deliberately stuff a sentinel password into the context. The
                // template MUST NOT emit it under any circumstance — the
                // recipient gets the password out-of-band from the sender.
                "password": "hunter2",
            })),
        )
        .unwrap();
        assert!(env
            .html_body
            .contains("https://crabcloud.example/s/AbCd123Xyz0789Q"));
        // The invariant: no template path ever reaches the password field.
        assert!(
            !env.html_body.contains("hunter2"),
            "password leaked into html_body: {}",
            env.html_body
        );
        assert!(
            !env.text_body.contains("hunter2"),
            "password leaked into text_body: {}",
            env.text_body
        );
        // The body should still mention "password" to explain the
        // password-protected status to the recipient.
        assert!(env.html_body.contains("password") || env.text_body.contains("password"));
    }

    #[test]
    fn expiration_warning_includes_link_and_date() {
        let env = render_template(
            EventType::ExpirationWarning,
            ctx(json!({
                "link_basename": "Photos",
                "link_url": "https://crabcloud.example/s/AbCd123Xyz0789Q",
                "expiration_dt": "2026-06-15",
            })),
        )
        .unwrap();
        assert!(env.html_body.contains("2026-06-15"));
        assert!(env.html_body.contains("Photos"));
    }

    #[test]
    fn html_escapes_user_supplied_path() {
        let env = render_template(
            EventType::ShareCreated,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "<script>alert(1)</script>",
            })),
        )
        .unwrap();
        // HTML body must escape; plaintext is verbatim.
        assert!(!env.html_body.contains("<script>"));
        assert!(env.html_body.contains("&lt;script&gt;"));
        assert!(env.text_body.contains("<script>"));
    }
}
