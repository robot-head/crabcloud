//! `Email` newtype with RFC validation.

use crate::error::UsersError;
use email_address::EmailAddress;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Email(String);

impl Email {
    /// Parse + canonicalize an email: trim, lowercase, validate via RFC 5321/5322.
    pub fn parse(s: impl Into<String>) -> Result<Self, UsersError> {
        let raw = s.into();
        let trimmed = raw.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Err(UsersError::InvalidEmail("empty".into()));
        }
        if trimmed.len() > 255 {
            return Err(UsersError::InvalidEmail(format!(
                "length {}",
                trimmed.len()
            )));
        }
        EmailAddress::from_str(&trimmed).map_err(|e| UsersError::InvalidEmail(e.to_string()))?;
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_emails_parse() {
        for ok in &[
            "a@b.com",
            "alice@example.org",
            "first.last+tag@sub.example.com",
        ] {
            assert!(Email::parse(*ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn invalid_emails_rejected() {
        for bad in &[
            "",
            "not-an-email",
            "@missing-local",
            "missing@",
            "two@@signs.com",
        ] {
            assert!(Email::parse(*bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn canonicalization_lowercases_and_trims() {
        let e = Email::parse("  Alice@Example.COM  ").unwrap();
        assert_eq!(e.as_str(), "alice@example.com");
    }
}
