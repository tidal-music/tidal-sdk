use std::collections::HashMap;

use crate::model::error::{network, other, ErrorSegment};
use crate::model::Error as PlayerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// Executes requests through the host HTTP stack; the core handles authentication and retries.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi", uniffi(name = "TransportError"))]
pub enum Error {
    #[error("network error: {description}")]
    NetworkError { description: String },
    #[error("timeout error: {description}")]
    TimeoutError { description: String },
    #[error("transport error: {description}")]
    OtherError { description: String },
}

impl From<&Error> for PlayerError {
    fn from(error: &Error) -> Self {
        match error {
            Error::NetworkError { .. } => network(error, ErrorSegment::NetworkError.code(None)),
            Error::TimeoutError { .. } => network(error, ErrorSegment::TimeoutError.code(None)),
            Error::OtherError { .. } => other(error, ErrorSegment::TransportError.code(None)),
        }
    }
}
