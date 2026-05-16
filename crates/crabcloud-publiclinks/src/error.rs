use thiserror::Error;

#[derive(Debug, Error)]
pub enum PublicLinkError {
    #[error("invalid cookie value")]
    InvalidCookie,
    #[error("password hashing failed: {0}")]
    Bcrypt(#[from] bcrypt::BcryptError),
    #[error("password too weak: {0}")]
    PasswordTooWeak(&'static str),
}
