//! Top-level mailer facade. Holds a configured `Transport` + the from-address
//! pair. `send` actually transmits.

use crate::envelope::MailEnvelope;
use crate::error::MailError;
use crate::transport::{Transport, TransportConfig};
use lettre::message::{header, MultiPart, SinglePart};
use lettre::{AsyncTransport, Message};

pub struct Mailer {
    transport: Transport,
    from_address: String,
    from_name: Option<String>,
}

impl Mailer {
    pub fn from_config(cfg: &TransportConfig) -> Result<Self, MailError> {
        let transport = Transport::from_config(cfg)?;
        let from_address = cfg
            .mail_from
            .clone()
            .unwrap_or_else(|| "no-reply@localhost".to_string());
        Ok(Self {
            transport,
            from_address,
            from_name: cfg.mail_from_name.clone(),
        })
    }

    pub async fn send(&self, env: &MailEnvelope) -> Result<(), MailError> {
        match &self.transport {
            Transport::Disabled => Ok(()),
            Transport::Log => {
                tracing::info!(
                    target: "crabcloud_mail::log_transport",
                    recipient = %env.recipient,
                    subject = %env.subject,
                    event_type = %env.event_type.as_str(),
                    text_body_bytes = env.text_body.len(),
                    "mail.transport=log envelope captured (not sent)"
                );
                Ok(())
            }
            Transport::Smtp(client) => {
                let from = match &self.from_name {
                    Some(name) => format!("{} <{}>", name, self.from_address),
                    None => self.from_address.clone(),
                };
                let msg = Message::builder()
                    .from(
                        from.parse()
                            .map_err(|e| MailError::ConfigInvalid(format!("mail_from parse: {e}")))?,
                    )
                    .to(env
                        .recipient
                        .parse()
                        .map_err(|e| MailError::Transport(format!("recipient parse: {e}")))?)
                    .subject(&env.subject)
                    .multipart(
                        MultiPart::alternative()
                            .singlepart(
                                SinglePart::builder()
                                    .header(header::ContentType::TEXT_PLAIN)
                                    .body(env.text_body.clone()),
                            )
                            .singlepart(
                                SinglePart::builder()
                                    .header(header::ContentType::TEXT_HTML)
                                    .body(env.html_body.clone()),
                            ),
                    )
                    .map_err(|e| MailError::Transport(format!("message build: {e}")))?;
                client.send(msg).await.map_err(|e| {
                    if e.is_transient() {
                        MailError::Transient(format!("smtp: {e}"))
                    } else {
                        MailError::Transport(format!("smtp: {e}"))
                    }
                })?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventType;
    use crate::transport::{SmtpSecurity, TransportConfig, TransportKind};

    fn env() -> MailEnvelope {
        MailEnvelope {
            recipient: "bob@example.com".to_string(),
            subject: "Hello".to_string(),
            html_body: "<p>hi</p>".to_string(),
            text_body: "hi".to_string(),
            event_type: EventType::ShareCreated,
        }
    }

    fn disabled() -> TransportConfig {
        TransportConfig {
            kind: TransportKind::Disabled,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_security: SmtpSecurity::None,
            mail_from: Some("noreply@example.com".to_string()),
            mail_from_name: None,
        }
    }

    fn log() -> TransportConfig {
        let mut c = disabled();
        c.kind = TransportKind::Log;
        c
    }

    #[tokio::test]
    async fn disabled_send_is_noop() {
        let m = Mailer::from_config(&disabled()).unwrap();
        m.send(&env()).await.unwrap();
    }

    #[tokio::test]
    async fn log_send_emits_tracing_event() {
        // tracing event capture isn't trivial without `tracing-test`; for
        // now just assert the send returns Ok. The e2e tests in Batch B
        // will install a subscriber and assert capture.
        let m = Mailer::from_config(&log()).unwrap();
        m.send(&env()).await.unwrap();
    }
}
