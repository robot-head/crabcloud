//! `MailEnvelope` — a fully-rendered mail ready for transport. Carries
//! the recipient address, subject, and both HTML + plaintext bodies for
//! multipart MIME assembly.

use serde::{Deserialize, Serialize};

/// Discriminates which notification event a queued mail represents.
/// Stored as a `&'static str` on the wire (`oc_mail_queue.event_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    ShareCreated,
    LinkEmailed,
    ExpirationWarning,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::ShareCreated => "share_created",
            EventType::LinkEmailed => "link_emailed",
            EventType::ExpirationWarning => "expiration_warning",
        }
    }

    /// Parse from the on-wire string used in `oc_mail_queue.event_type`.
    /// Returns `None` for unknown strings (forward-compat with rows
    /// written by a newer server).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "share_created" => Some(Self::ShareCreated),
            "link_emailed" => Some(Self::LinkEmailed),
            "expiration_warning" => Some(Self::ExpirationWarning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MailEnvelope {
    pub recipient: String,
    pub subject: String,
    pub html_body: String,
    pub text_body: String,
    pub event_type: EventType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_round_trips_via_str() {
        for v in &[
            EventType::ShareCreated,
            EventType::LinkEmailed,
            EventType::ExpirationWarning,
        ] {
            let s = v.as_str();
            assert_eq!(EventType::from_str(s), Some(*v));
        }
    }

    #[test]
    fn event_type_parse_rejects_unknown() {
        assert_eq!(EventType::from_str("password_reset"), None);
        assert_eq!(EventType::from_str(""), None);
    }
}
