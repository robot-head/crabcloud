//! Mailer transports: SMTP, Log (tracing-event), Disabled (no-op).

use crate::error::MailError;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

/// Operator-tunable transport configuration. Parsed from `FileConfig::mail`.
/// Not `Serialize`-able because `SecretString` only round-trips on the
/// `Deserialize` side (mirrors how `FileConfig` itself is wired).
#[derive(Debug, Clone, Deserialize)]
pub struct TransportConfig {
    pub kind: TransportKind,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<secrecy::SecretString>,
    pub smtp_security: SmtpSecurity,
    pub mail_from: Option<String>,
    pub mail_from_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Smtp,
    Log,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    Tls,
    Starttls,
    None,
}

/// Runtime transport. `Smtp` carries a live lettre client; `Log` writes
/// envelopes to a tracing event; `Disabled` is a no-op that returns Ok.
pub enum Transport {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    Log,
    Disabled,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `AsyncSmtpTransport` is not `Debug`; print the variant name only.
            Transport::Smtp(_) => f.write_str("Transport::Smtp(..)"),
            Transport::Log => f.write_str("Transport::Log"),
            Transport::Disabled => f.write_str("Transport::Disabled"),
        }
    }
}

impl Transport {
    /// Build a transport from config. Validates required fields when
    /// `kind == Smtp`; logs the active kind at info level.
    pub fn from_config(cfg: &TransportConfig) -> Result<Self, MailError> {
        match cfg.kind {
            TransportKind::Disabled => {
                tracing::info!(
                    "mail.transport = disabled — outbound mail will be silently dropped"
                );
                Ok(Self::Disabled)
            }
            TransportKind::Log => {
                tracing::info!(
                    "mail.transport = log — outbound mail will be emitted as tracing events"
                );
                Ok(Self::Log)
            }
            TransportKind::Smtp => {
                let host = cfg.smtp_host.as_deref().ok_or_else(|| {
                    MailError::ConfigInvalid("smtp_host required when transport=smtp".into())
                })?;
                let port = cfg.smtp_port.ok_or_else(|| {
                    MailError::ConfigInvalid("smtp_port required when transport=smtp".into())
                })?;
                let _ = cfg.mail_from.as_deref().ok_or_else(|| {
                    MailError::ConfigInvalid("mail_from required when transport=smtp".into())
                })?;
                let mut builder = match cfg.smtp_security {
                    SmtpSecurity::Tls => {
                        let tls = TlsParameters::new(host.to_string())
                            .map_err(|e| MailError::ConfigInvalid(format!("tls params: {e}")))?;
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
                            .tls(Tls::Wrapper(tls))
                    }
                    SmtpSecurity::Starttls => {
                        let tls = TlsParameters::new(host.to_string())
                            .map_err(|e| MailError::ConfigInvalid(format!("tls params: {e}")))?;
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
                            .tls(Tls::Required(tls))
                    }
                    SmtpSecurity::None => {
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
                    }
                }
                .port(port);
                if let (Some(u), Some(p)) =
                    (cfg.smtp_username.as_deref(), cfg.smtp_password.as_ref())
                {
                    builder = builder
                        .credentials(Credentials::new(
                            u.to_string(),
                            p.expose_secret().to_string(),
                        ))
                        .authentication(vec![Mechanism::Plain, Mechanism::Login]);
                }
                tracing::info!(host, port, "mail.transport = smtp");
                Ok(Self::Smtp(builder.build()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: TransportKind) -> TransportConfig {
        TransportConfig {
            kind,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_security: SmtpSecurity::Starttls,
            mail_from: None,
            mail_from_name: None,
        }
    }

    #[test]
    fn disabled_transport_builds() {
        assert!(matches!(
            Transport::from_config(&cfg(TransportKind::Disabled)).unwrap(),
            Transport::Disabled
        ));
    }

    #[test]
    fn log_transport_builds() {
        assert!(matches!(
            Transport::from_config(&cfg(TransportKind::Log)).unwrap(),
            Transport::Log
        ));
    }

    #[test]
    fn smtp_transport_requires_host() {
        let c = cfg(TransportKind::Smtp);
        let r = Transport::from_config(&c);
        match r {
            Err(MailError::ConfigInvalid(msg)) => assert!(msg.contains("smtp_host")),
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn smtp_transport_requires_mail_from() {
        let mut c = cfg(TransportKind::Smtp);
        c.smtp_host = Some("smtp.example.com".into());
        c.smtp_port = Some(587);
        let r = Transport::from_config(&c);
        match r {
            Err(MailError::ConfigInvalid(msg)) => assert!(msg.contains("mail_from")),
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn smtp_transport_builds_with_required_fields() {
        let mut c = cfg(TransportKind::Smtp);
        c.smtp_host = Some("smtp.example.com".into());
        c.smtp_port = Some(587);
        c.mail_from = Some("noreply@example.com".into());
        assert!(matches!(
            Transport::from_config(&c).unwrap(),
            Transport::Smtp(_)
        ));
    }
}
