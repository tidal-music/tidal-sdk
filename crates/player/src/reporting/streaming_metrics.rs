use serde::Serialize;

use super::item_event::{self, PlaybackRequestReason, PlayerItemReportingEvent};
use super::payload::{
    audio_mode, error_code, quality, reporting_event, FindEvent, MaybeFrom, ToTimestamp,
};
use super::records::PlayerItemReportingRecords;
use crate::model::error::{other, ErrorSegment};
use crate::model::{Error, MediaPresentation, MediaProduct};
use crate::platforms::events::ReportingEvent;

#[derive(Debug, thiserror::Error)]
#[error("Ended while stalled (counts as error)")]
struct EndedWhileStalled;

impl From<EndedWhileStalled> for Error {
    fn from(error: EndedWhileStalled) -> Self {
        ErrorSegment::PlaybackError.wrap(other(error, "ews"))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Ended and playback failed to start within 5 seconds (counts as error)")]
struct EndedBeforeStarting;

impl From<EndedBeforeStarting> for Error {
    fn from(error: EndedBeforeStarting) -> Self {
        ErrorSegment::PlaybackError.wrap(other(error, "ebs"))
    }
}

pub(crate) struct StreamingMetrics(pub(crate) Vec<StreamingMetricsEvent>);

pub(crate) enum StreamingMetricsEvent {
    StreamingSessionStart(StreamingSessionStart),
    PlaybackInfoFetch(PlaybackInfoFetch),
    DrmLicenseFetch(DrmLicenseFetch),
    PlaybackStatistics(PlaybackStatistics),
    StreamingSessionEnd(StreamingSessionEnd),
}

impl From<&PlayerItemReportingRecords> for StreamingMetrics {
    fn from(records: &PlayerItemReportingRecords) -> Self {
        let streaming_session_start = StreamingSessionStart::maybe_from(records);
        let playback_info_fetch = PlaybackInfoFetch::maybe_from(records);
        let drm_license_fetch = DrmLicenseFetch::maybe_from(records);
        let playback_statistics = PlaybackStatistics::maybe_from(records);
        let streaming_session_end = StreamingSessionEnd::maybe_from(records);

        let events = vec![
            streaming_session_start.map(StreamingMetricsEvent::StreamingSessionStart),
            playback_info_fetch.map(StreamingMetricsEvent::PlaybackInfoFetch),
            drm_license_fetch.map(StreamingMetricsEvent::DrmLicenseFetch),
            playback_statistics.map(StreamingMetricsEvent::PlaybackStatistics),
            streaming_session_end.map(StreamingMetricsEvent::StreamingSessionEnd),
        ];
        Self(events.into_iter().flatten().collect())
    }
}

impl TryFrom<StreamingMetricsEvent> for ReportingEvent {
    type Error = serde_json::Error;

    fn try_from(event: StreamingMetricsEvent) -> Result<Self, Self::Error> {
        match event {
            StreamingMetricsEvent::StreamingSessionStart(payload) => {
                reporting_event("streaming_metrics", "streaming_session_start", payload)
            }
            StreamingMetricsEvent::PlaybackInfoFetch(payload) => {
                reporting_event("streaming_metrics", "playback_info_fetch", payload)
            }
            StreamingMetricsEvent::DrmLicenseFetch(payload) => {
                reporting_event("streaming_metrics", "drm_license_fetch", payload)
            }
            StreamingMetricsEvent::PlaybackStatistics(payload) => {
                reporting_event("streaming_metrics", "playback_statistics", payload)
            }
            StreamingMetricsEvent::StreamingSessionEnd(payload) => {
                reporting_event("streaming_metrics", "streaming_session_end", payload)
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamingSessionStart {
    pub streaming_session_id: String,
    pub timestamp: u64,
    pub is_offline_mode_start: bool,
    pub start_reason: StartReason,
    pub session_type: String,
    pub session_product_type: String,
    pub session_product_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamingSessionEnd {
    pub streaming_session_id: String,
    pub timestamp: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaybackInfoFetch {
    pub streaming_session_id: String,
    pub start_timestamp: u64,
    pub end_timestamp: u64,
    pub end_reason: EndReason,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DrmLicenseFetch {
    pub streaming_session_id: String,
    pub start_timestamp: u64,
    pub end_timestamp: u64,
    pub end_reason: EndReason,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaybackStatistics {
    pub streaming_session_id: String,
    pub ideal_start_timestamp: Option<u64>,
    pub actual_start_timestamp: Option<u64>,
    pub has_ads: bool,
    pub product_type: Option<String>,
    pub actual_product_id: Option<String>,
    pub actual_stream_type: String,
    pub actual_asset_presentation: MediaPresentation,
    pub actual_audio_mode: Option<String>,
    pub actual_quality: String,
    pub cdm: Option<String>,
    pub stalls: Stalls,
    pub start_reason: StartReason,
    pub end_reason: EndReason,
    pub end_timestamp: u64,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(transparent)]
pub(crate) struct Stalls(pub(crate) Vec<Stall>);

impl From<&[PlayerItemReportingEvent]> for Stalls {
    fn from(events: &[PlayerItemReportingEvent]) -> Self {
        let mut timeline: Vec<_> = events.iter().filter_map(Progression::maybe_from).collect();
        timeline.dedup_by_key(|progression| std::mem::discriminant(progression));

        Self(
            timeline
                .iter()
                .enumerate()
                .filter_map(|(index, _)| Stall::maybe_from((index, timeline.as_slice())))
                .collect(),
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stall {
    pub reason: StallReason,
    pub asset_position: f64,
    pub start_timestamp: u64,
    pub end_timestamp: u64,
}

impl MaybeFrom<(usize, &[Progression<'_>])> for Stall {
    fn maybe_from((index, timeline): (usize, &[Progression<'_>])) -> Option<Self> {
        let event = timeline.get(index)?.stalled()?;
        let end_timestamp = timeline[index + 1..]
            .iter()
            .find_map(|progression| match progression {
                Progression::Progressing(timestamp) => Some(*timestamp),
                _ => None,
            })
            .unwrap_or_else(|| web_time::SystemTime::now().to_timestamp());

        Some(Self {
            reason: StallReason::from_timeline(index, timeline),
            asset_position: event.position.as_secs_f64(),
            start_timestamp: event.timestamp.to_timestamp(),
            end_timestamp,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum StallReason {
    Seek,
    Unexpected,
}

impl StallReason {
    fn from_timeline(index: usize, timeline: &[Progression<'_>]) -> Self {
        if index > 0 && matches!(timeline[index - 1], Progression::Seeking) {
            Self::Seek
        } else {
            Self::Unexpected
        }
    }
}

enum Progression<'a> {
    Seeking,
    Stalled(&'a item_event::Stalled),
    Progressing(u64),
}

impl<'a> Progression<'a> {
    fn stalled(&self) -> Option<&'a item_event::Stalled> {
        match self {
            Self::Stalled(event) => Some(event),
            _ => None,
        }
    }
}

impl<'a> MaybeFrom<&'a PlayerItemReportingEvent> for Progression<'a> {
    fn maybe_from(event: &'a PlayerItemReportingEvent) -> Option<Self> {
        match event {
            PlayerItemReportingEvent::Seeking(_) => Some(Self::Seeking),
            PlayerItemReportingEvent::Stalled(event) => Some(Self::Stalled(event)),
            PlayerItemReportingEvent::Started(event) => {
                Some(Self::Progressing(event.timestamp.to_timestamp()))
            }
            PlayerItemReportingEvent::Resumed(event) => {
                Some(Self::Progressing(event.timestamp.to_timestamp()))
            }
            PlayerItemReportingEvent::Position(event) => {
                Some(Self::Progressing(event.timestamp.to_timestamp()))
            }
            PlayerItemReportingEvent::Failed(event) => {
                Some(Self::Progressing(event.timestamp.to_timestamp()))
            }
            PlayerItemReportingEvent::Ended(event) => {
                Some(Self::Progressing(event.timestamp.to_timestamp()))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum StartReason {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum EndReason {
    Complete,
    Error,
    Other,
}

impl<T, E> From<Option<Result<T, E>>> for EndReason {
    fn from(outcome: Option<Result<T, E>>) -> Self {
        match outcome {
            Some(Ok(_)) => Self::Complete,
            Some(Err(_)) => Self::Error,
            None => Self::Other,
        }
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for StreamingSessionStart {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        let session_started = records
            .events
            .iter()
            .find_first_event::<item_event::SessionStarted>()?;

        let (product_type, product_id) = match &session_started.product {
            MediaProduct::Track { id } => ("TRACK", id),
            MediaProduct::Video { id } => ("VIDEO", id),
        };

        Some(Self {
            streaming_session_id: records.session_id.to_string(),
            timestamp: session_started.timestamp.to_timestamp(),
            is_offline_mode_start: false,
            start_reason: StartReason::from(records.events.as_slice()),
            session_type: "PLAYBACK".into(),
            session_product_type: product_type.into(),
            session_product_id: product_id.clone(),
        })
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for StreamingSessionEnd {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        records
            .events
            .iter()
            .find_first_event::<item_event::SessionStarted>()?;

        Some(Self {
            streaming_session_id: records.session_id.to_string(),
            timestamp: web_time::SystemTime::now().to_timestamp(),
        })
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for PlaybackInfoFetch {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        let mut events = records.events.iter();
        let started = events.find_first_event::<item_event::ManifestRetrievalStarted>()?;

        let ended = events.find_first_event::<item_event::ManifestRetrievalEnded>();
        let error = ended.and_then(|event| event.result.as_ref().err());
        let end_timestamp = ended.map_or_else(web_time::SystemTime::now, |event| event.timestamp);

        Some(Self {
            streaming_session_id: records.session_id.to_string(),
            start_timestamp: started.timestamp.to_timestamp(),
            end_timestamp: end_timestamp.to_timestamp(),
            end_reason: EndReason::from(ended.map(|event| event.result.as_ref())),
            error_message: error.map(|error| error.description().into()),
            error_code: error.map(|error| error_code(error).into()),
        })
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for DrmLicenseFetch {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        use PlayerItemReportingEvent::*;

        let started = records.events.iter().find_map(|event| match event {
            CertificateRetrievalStarted(event) => Some(event.timestamp),
            LicenseRetrievalStarted(event) => Some(event.timestamp),
            _ => None,
        })?;

        let ended = records
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                CertificateRetrievalStarted(_) | LicenseRetrievalStarted(_) => Some(None),
                CertificateRetrievalEnded(event) => Some(Some((event.timestamp, &event.result))),
                LicenseRetrievalEnded(event) => Some(Some((event.timestamp, &event.result))),
                _ => None,
            })
            .flatten();

        let error = ended.and_then(|(_, result)| result.as_ref().err());
        let end_timestamp =
            ended.map_or_else(web_time::SystemTime::now, |(timestamp, _)| timestamp);

        Some(Self {
            streaming_session_id: records.session_id.to_string(),
            start_timestamp: started.to_timestamp(),
            end_timestamp: end_timestamp.to_timestamp(),
            end_reason: EndReason::from(ended.map(|(_, result)| result.as_ref())),
            error_message: error.map(|error| error.description().into()),
            error_code: error.map(|error| error_code(error).into()),
        })
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for PlaybackStatistics {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        let events = records.events.as_slice();
        let session_started = events
            .iter()
            .find_first_event::<item_event::SessionStarted>()?;

        let playback_requested = events
            .iter()
            .find_first_event::<item_event::PlaybackRequested>()?;

        let manifest_retrieval_ended = events
            .iter()
            .find_first_event::<item_event::ManifestRetrievalEnded>()?;

        let actual_start_timestamp = events
            .iter()
            .find_first_event::<item_event::Started>()
            .map(|event| event.timestamp.to_timestamp());

        let product_info = manifest_retrieval_ended.result.as_ref().ok();
        let (product_type, actual_product_id) = match product_info {
            Some(info) => match &info.product {
                MediaProduct::Track { id } => (Some("TRACK".into()), Some(id.clone())),
                MediaProduct::Video { id } => (Some("VIDEO".into()), Some(id.clone())),
            },
            None => (None, None),
        };

        let formats = product_info.map_or(&[][..], |info| info.formats.as_slice());
        let ended = events.iter().find_first_event::<item_event::Ended>();
        let error = Error::maybe_from(records);
        let failed = events.iter().find_first_event::<item_event::Failed>();
        let end_timestamp = failed
            .map(|event| event.timestamp)
            .or_else(|| ended.map(|event| event.timestamp))
            .unwrap_or_else(web_time::SystemTime::now)
            .to_timestamp();

        let end_reason = match (ended, error.as_ref()) {
            (Some(_), _) => EndReason::Complete,
            (None, Some(_)) => EndReason::Error,
            _ => EndReason::Other,
        };

        Some(Self {
            streaming_session_id: records.session_id.to_string(),
            ideal_start_timestamp: Some(playback_requested.timestamp.to_timestamp()),
            actual_start_timestamp,
            has_ads: false,
            product_type,
            actual_product_id,
            actual_stream_type: "ON_DEMAND".into(),
            actual_asset_presentation: product_info
                .map_or(MediaPresentation::Unknown, |info| info.presentation),
            actual_audio_mode: audio_mode(&session_started.product, formats),
            actual_quality: quality(&session_started.product, formats),
            cdm: None,
            stalls: Stalls::from(events),
            start_reason: StartReason::from(events),
            end_reason,
            end_timestamp,
            error_message: error.as_ref().map(|error| error.description().into()),
            error_code: error.as_ref().map(|error| error_code(error).into()),
        })
    }
}

impl From<&[PlayerItemReportingEvent]> for StartReason {
    fn from(events: &[PlayerItemReportingEvent]) -> Self {
        events
            .iter()
            .find_first_event::<item_event::PlaybackRequested>()
            .map(|event| Self::from(event.reason))
            .unwrap_or(Self::Implicit)
    }
}

impl From<PlaybackRequestReason> for StartReason {
    fn from(reason: PlaybackRequestReason) -> Self {
        match reason {
            PlaybackRequestReason::Explicit => Self::Explicit,
            PlaybackRequestReason::Implicit => Self::Implicit,
        }
    }
}

impl MaybeFrom<&PlayerItemReportingRecords> for Error {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        let failed = records
            .events
            .iter()
            .find_first_event::<item_event::Failed>();

        if let Some(event) = failed {
            return Some(event.error.clone());
        }

        if records
            .events
            .iter()
            .find_first_event::<item_event::Ended>()
            .is_some()
        {
            return None;
        }

        // Seek completion and preparation facts do not resolve a stall.
        let stalled = records
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                PlayerItemReportingEvent::Stalled(_) => Some(true),
                PlayerItemReportingEvent::Started(_)
                | PlayerItemReportingEvent::Resumed(_)
                | PlayerItemReportingEvent::Position(_)
                | PlayerItemReportingEvent::Paused(_) => Some(false),
                _ => None,
            })
            .unwrap_or(false);

        if stalled {
            return Some(EndedWhileStalled.into());
        }

        let playback_requested = records
            .events
            .iter()
            .find_first_event::<item_event::PlaybackRequested>()?;

        let playback_started = records
            .events
            .iter()
            .find_first_event::<item_event::Started>();

        if playback_started.is_some() {
            return None;
        }

        let time_since_playback_requested = web_time::SystemTime::now()
            .duration_since(playback_requested.timestamp)
            .unwrap_or_default();

        if time_since_playback_requested > std::time::Duration::from_secs(5) {
            return Some(EndedBeforeStarting.into());
        }

        None
    }
}
