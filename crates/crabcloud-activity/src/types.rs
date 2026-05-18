//! Public-facing value types for the activity service.

use crabcloud_users::UserId;
use serde::{Deserialize, Serialize};

/// Discriminates the event categories MVP supports. Wire form is the
/// `as_str()` value stored in `oc_activity.event_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    FileCreated,
    FileUpdated,
    FileDeleted,
    FileRenamed,
    FileRestored,
    ShareCreated,
    ShareDeleted,
    VersionRestored,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::FileCreated => "file_created",
            EventType::FileUpdated => "file_updated",
            EventType::FileDeleted => "file_deleted",
            EventType::FileRenamed => "file_renamed",
            EventType::FileRestored => "file_restored",
            EventType::ShareCreated => "share_created",
            EventType::ShareDeleted => "share_deleted",
            EventType::VersionRestored => "version_restored",
        }
    }

    /// Inverse of `as_str`. Not a `FromStr` impl because callers want
    /// an `Option<Self>` rather than going through a string-error type
    /// (this enum's wire form has no invalid-input nuance to preserve).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file_created" => Some(Self::FileCreated),
            "file_updated" => Some(Self::FileUpdated),
            "file_deleted" => Some(Self::FileDeleted),
            "file_renamed" => Some(Self::FileRenamed),
            "file_restored" => Some(Self::FileRestored),
            "share_created" => Some(Self::ShareCreated),
            "share_deleted" => Some(Self::ShareDeleted),
            "version_restored" => Some(Self::VersionRestored),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectType {
    File,
    Share,
    Version,
}

impl ObjectType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectType::File => "file",
            ObjectType::Share => "share",
            ObjectType::Version => "version",
        }
    }

    /// Inverse of `as_str`; see [`EventType::from_str`] for the
    /// rationale.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "share" => Some(Self::Share),
            "version" => Some(Self::Version),
            _ => None,
        }
    }
}

/// Input to [`crate::ActivityEmitter::emit`]. Emitter sites construct
/// this with recipient list already resolved (actor + share recipients
/// + group members where applicable).
#[derive(Debug, Clone)]
pub struct ActivityEvent {
    /// Empty string for public-link / system events.
    pub actor: String,
    pub event_type: EventType,
    /// i18n key for the actor row (e.g. `"file_updated_you"`).
    pub subject_id_actor: String,
    /// i18n key for non-actor recipient rows (e.g. `"file_updated_by"`).
    pub subject_id_recipient: String,
    /// JSON object with `{actor, file, ...}` keys consumed by the template.
    pub subject_params: serde_json::Value,
    pub object_type: ObjectType,
    pub object_id: Option<i64>,
    /// Includes the actor if they should see the event in their own feed.
    pub recipients: Vec<UserId>,
    /// Unix seconds; emit site passes `chrono::Utc::now().timestamp()`.
    pub occurred_at: i64,
}

/// Row returned from [`crate::Activity::list`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityRow {
    pub id: i64,
    pub affected_user: String,
    pub actor: String,
    pub event_type: String,
    pub subject_id: String,
    pub subject_params: serde_json::Value,
    pub object_type: String,
    pub object_id: Option<i64>,
    pub occurred_at: i64,
    pub last_seen_at: i64,
    pub count: i32,
}

/// Row returned from [`crate::ActivitySettings::get_all_for_user`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivitySetting {
    pub event_type: String,
    pub stream: bool,
}
