//! Notification hooks for the `Shares` service. These are *post-commit*,
//! best-effort: a render or enqueue failure never propagates back to the
//! caller — the share row has already landed. Every failure path logs
//! and drops so the outer `Shares::create` keeps its `Ok` contract.
//!
//! Extracted from `service.rs` to keep that file focused on CRUD +
//! token resolve. SP11/C3 + SP11/C4.

use chrono::NaiveDateTime;
use crabcloud_mail::{render_template, EventType, TemplateContext};
use crabcloud_storage::StoragePath;
use crabcloud_users::{UserId, UsersService};

use super::Shares;

impl Shares {
    /// Best-effort: look up the recipient's email + display name,
    /// the owner's display name, gate on `share_created` opt-out,
    /// render the `share_created` template, and enqueue.
    ///
    /// Never returns an error: every failure path logs + drops so the
    /// caller's `create()` success is preserved.
    pub(super) async fn try_enqueue_share_created_mail(
        &self,
        recipient_uid: &str,
        owner_uid: &str,
        storage_path: &StoragePath,
    ) {
        let recipient = match UserId::new(recipient_uid) {
            Ok(uid) => uid,
            Err(_) => return,
        };
        // 1. Resolve recipient user (need their email + display name).
        let user = match self.users.user_store().lookup(&recipient).await {
            Ok(Some(u)) => u,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "share_created: recipient lookup failed");
                return;
            }
        };
        let email = match &user.email {
            Some(e) => e.as_str().to_string(),
            None => return, // no email, nothing to send
        };
        // 2. Gate on opt-out (default true).
        match self.prefs.get(recipient.as_str(), "share_created").await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::warn!(error = %e, "share_created: prefs.get failed");
                return;
            }
        }
        // 3. Owner display name (fall back to uid).
        let owner_display = display_name_of(&self.users, owner_uid).await;
        // 4. Render + enqueue.
        let ctx = TemplateContext {
            lang: "en".to_string(),
            instance_url: self.instance_url.clone(),
            recipient_display_name: user.display_name.clone(),
            recipient_email: email,
            event_specific: serde_json::json!({
                "owner_display_name": owner_display,
                "path_basename": storage_path.basename(),
            }),
        };
        let env = match render_template(EventType::ShareCreated, ctx) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "share_created: render_template failed");
                return;
            }
        };
        if let Err(e) = self.mail.enqueue(&env).await {
            tracing::warn!(error = %e, "share_created: mail.enqueue failed");
        }
    }

    /// Best-effort: render + enqueue a `link_emailed` mail to the
    /// recipient address. Mirrors `try_enqueue_share_created_mail`
    /// shape; no opt-out gate since the recipient isn't a local uid.
    pub(super) async fn try_enqueue_link_emailed_mail(
        &self,
        recipient_email: &str,
        owner_uid: &str,
        storage_path: &StoragePath,
        token: &str,
        password_protected: bool,
        expiration: Option<NaiveDateTime>,
    ) {
        let owner_display = display_name_of(&self.users, owner_uid).await;
        let link_url = build_link_url(&self.instance_url, token);
        let expiration_str = expiration
            .map(|n| n.date().format("%Y-%m-%d").to_string())
            .unwrap_or_default();

        let ctx = TemplateContext {
            lang: "en".to_string(),
            instance_url: self.instance_url.clone(),
            // Templates use `recipient_display_name`; for an email-only
            // recipient we don't have one, so reuse the address.
            recipient_display_name: recipient_email.to_string(),
            recipient_email: recipient_email.to_string(),
            event_specific: serde_json::json!({
                "owner_display_name": owner_display,
                "path_basename": storage_path.basename(),
                "link_url": link_url,
                "password_protected": password_protected,
                "expiration_dt": expiration_str,
            }),
        };
        let env = match render_template(EventType::LinkEmailed, ctx) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "link_emailed: render_template failed");
                return;
            }
        };
        if let Err(e) = self.mail.enqueue(&env).await {
            tracing::warn!(error = %e, "link_emailed: mail.enqueue failed");
        }
    }
}

/// Build the absolute (or relative-fallback) share-link URL the
/// templates expand into `{{ link_url }}`. When `instance_url` is
/// empty or already ends in `/`, we avoid producing `//s/<token>`.
pub(crate) fn build_link_url(instance_url: &str, token: &str) -> String {
    let trimmed = instance_url.trim_end_matches('/');
    if trimmed.is_empty() {
        format!("/s/{token}")
    } else {
        format!("{trimmed}/s/{token}")
    }
}

/// Resolve a uid's display name, falling back to the raw uid if the
/// user row is missing or the display name is empty. Used by the
/// notification hooks; mirrors the OCS-handler helper.
pub(crate) async fn display_name_of(users: &UsersService, raw_uid: &str) -> String {
    let uid = match UserId::new(raw_uid) {
        Ok(u) => u,
        Err(_) => return raw_uid.to_string(),
    };
    match users.user_store().lookup(&uid).await {
        Ok(Some(u)) if !u.display_name.is_empty() => u.display_name,
        _ => raw_uid.to_string(),
    }
}
