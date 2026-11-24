use crate::model::error::{other, ErrorSegment};
use crate::model::Error as PlayerError;

/// Connects the core HTTP client to host-managed credentials and token refresh.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
#[async_trait::async_trait]
pub trait Authenticator: Send + Sync {
    async fn access_token(&self) -> Result<String, Error>;
    async fn refreshed_access_token(&self) -> Result<String, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi", uniffi(name = "AuthenticatorError"))]
pub enum Error {
    #[error("authentication failed: {description}")]
    AuthenticationFailed { description: String },
}

impl From<&Error> for PlayerError {
    fn from(error: &Error) -> Self {
        match error {
            Error::AuthenticationFailed { .. } => {
                other(error, ErrorSegment::AuthenticationError.code(None))
            }
        }
    }
}
