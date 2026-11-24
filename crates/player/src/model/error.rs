use std::fmt::Display;

/// Public meaning, recovery guidance, and diagnostics of a playback failure.
/// `Other::can_retry` refers to a fresh attempt. Preparation failures require a new seed.
/// Descriptions are free-text diagnostics for logging, not user-facing messages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi", uniffi(name = "PlayerError"))]
pub enum Error {
    #[error("Playback denied ({reason:?}, {code}): {description}")]
    Denied {
        reason: DenialReason,
        code: String,
        description: String,
    },
    #[error("Network failure ({code}): {description}")]
    Network { code: String, description: String },
    #[error("Other failure ({code}): {description}")]
    Other {
        can_retry: bool,
        code: String,
        description: String,
    },
}

impl Error {
    pub fn description(&self) -> &str {
        match self {
            Self::Denied { description, .. }
            | Self::Network { description, .. }
            | Self::Other { description, .. } => description,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DenialReason {
    ContentNotFound,
    PrerequisiteMissing,
    GeoRestricted,
    PurchaseRequired,
    ClientNotEntitled,
    ConcurrentPlayback,
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum ErrorSegment {
    ManifestError = b'm',
    DrmError = b'd',
    PlaybackError = b'p',
    NetworkError = b'n',
    OtherError = b'o',
    ContentError = b'c',
    TimeoutError = b't',
    TransportError = b'x',
    AuthenticationError = b'a',
    HttpError = b'h',
}

impl ErrorSegment {
    fn as_char(self) -> char {
        char::from(self as u8)
    }

    pub(crate) fn code(self, detail: Option<&str>) -> String {
        let segment = self.as_char();
        match detail {
            Some(detail) => format!("{segment}:{detail}"),
            None => segment.to_string(),
        }
    }

    pub(crate) fn wrap(self, mut error: Error) -> Error {
        let code = match &mut error {
            Error::Denied { code, .. }
            | Error::Network { code, .. }
            | Error::Other { code, .. } => code,
        };
        code.insert(0, self.as_char());
        error
    }
}

pub(crate) fn denied(error: impl Display, reason: DenialReason, code: &str) -> Error {
    Error::Denied {
        reason,
        code: code.into(),
        description: error.to_string(),
    }
}

pub(crate) fn network(error: impl Display, code: impl Into<String>) -> Error {
    Error::Network {
        code: code.into(),
        description: error.to_string(),
    }
}

pub(crate) fn other(error: impl Display, code: impl Into<String>) -> Error {
    Error::Other {
        can_retry: false,
        code: code.into(),
        description: error.to_string(),
    }
}

pub(crate) fn http_failure(error: impl Display, http_status: u16) -> Error {
    Error::Other {
        can_retry: matches!(http_status, 408 | 429 | 500..=599),
        code: ErrorSegment::HttpError.code(Some(&http_status.to_string())),
        description: error.to_string(),
    }
}
