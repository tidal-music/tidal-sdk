use web_time::SystemTime;

use super::payload::MaybeFrom;
use crate::model::{Error, MediaProduct, MediaProductInfo, MediaTime};
use crate::platforms::media::{MediaEvent, MediaPlayerEvent};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionStarted {
    pub timestamp: SystemTime,
    pub product: MediaProduct,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestRetrievalStarted {
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestRetrievalEnded {
    pub timestamp: SystemTime,
    pub result: Result<MediaProductInfo, Error>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CertificateRetrievalStarted {
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CertificateRetrievalEnded {
    pub timestamp: SystemTime,
    pub result: Result<(), Error>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LicenseRetrievalStarted {
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LicenseRetrievalEnded {
    pub timestamp: SystemTime,
    pub result: Result<(), Error>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackRequested {
    pub timestamp: SystemTime,
    pub reason: PlaybackRequestReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRequestReason {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preparing {
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paused {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stalled {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resumed {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seeking {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seeked {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub timestamp: SystemTime,
    pub position: MediaTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failed {
    pub timestamp: SystemTime,
    /// Installation rejection has no positional observation.
    pub position: Option<MediaTime>,
    pub error: Error,
}

#[derive(Debug, Clone, PartialEq, derive_more::TryInto)]
#[try_into(ref)]
pub(crate) enum PlayerItemReportingEvent {
    SessionStarted(SessionStarted),
    ManifestRetrievalStarted(ManifestRetrievalStarted),
    ManifestRetrievalEnded(ManifestRetrievalEnded),
    CertificateRetrievalStarted(CertificateRetrievalStarted),
    CertificateRetrievalEnded(CertificateRetrievalEnded),
    LicenseRetrievalStarted(LicenseRetrievalStarted),
    LicenseRetrievalEnded(LicenseRetrievalEnded),
    PlaybackRequested(PlaybackRequested),
    Position(Position),
    Preparing(Preparing),
    Prepared(Prepared),
    Started(Started),
    Paused(Paused),
    Stalled(Stalled),
    Resumed(Resumed),
    Seeking(Seeking),
    Seeked(Seeked),
    Ended(Ended),
    Failed(Failed),
}

impl MaybeFrom<&MediaEvent> for PlayerItemReportingEvent {
    fn maybe_from(observation: &MediaEvent) -> Option<Self> {
        let timestamp = observation.emitted_at;
        Some(match &observation.event {
            MediaPlayerEvent::Completed
            | MediaPlayerEvent::Duration(_, _)
            | MediaPlayerEvent::Current { item: _ } => return None,
            MediaPlayerEvent::Position(_, position) => Self::Position(Position {
                timestamp,
                position: position.position,
            }),
            MediaPlayerEvent::Seeking(_, position) => Self::Seeking(Seeking {
                timestamp,
                position: *position,
            }),
            MediaPlayerEvent::Seeked(_, position) => Self::Seeked(Seeked {
                timestamp,
                position: position.position,
            }),
            MediaPlayerEvent::Preparing(_) => Self::Preparing(Preparing { timestamp }),
            MediaPlayerEvent::Prepared(_) => Self::Prepared(Prepared { timestamp }),
            MediaPlayerEvent::Started(_, position) => Self::Started(Started {
                timestamp,
                position: *position,
            }),
            MediaPlayerEvent::Paused(_, position) => Self::Paused(Paused {
                timestamp,
                position: position.position,
            }),
            MediaPlayerEvent::Stalled(_, position) => Self::Stalled(Stalled {
                timestamp,
                position: position.position,
            }),
            MediaPlayerEvent::Resumed(_, position) => Self::Resumed(Resumed {
                timestamp,
                position: position.position,
            }),
            MediaPlayerEvent::Ended(_, position) => Self::Ended(Ended {
                timestamp,
                position: *position,
            }),
            MediaPlayerEvent::Failed(_, position, error) => Self::Failed(Failed {
                timestamp,
                position: Some(*position),
                error: error.clone().into(),
            }),
        })
    }
}
