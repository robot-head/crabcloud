use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("template render failed: {0}")]
    Render(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("transient transport failure: {0}")]
    Transient(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MailError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}
