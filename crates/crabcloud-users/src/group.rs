//! `GroupId` newtype + `Group` struct. Same validation shape as `UserId`.

use crate::error::UsersError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(String);

impl GroupId {
    pub fn new(s: impl Into<String>) -> Result<Self, UsersError> {
        let s = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(UsersError::InvalidUid(format!("gid length {}", s.len())));
        }
        for ch in s.chars() {
            if !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '@' | '-') {
                return Err(UsersError::InvalidUid(format!("gid char {:?}", ch)));
            }
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub gid: GroupId,
    pub display_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_gid_accepted() {
        assert!(GroupId::new("admin").is_ok());
    }

    #[test]
    fn whitespace_rejected() {
        assert!(GroupId::new("ad min").is_err());
    }
}
