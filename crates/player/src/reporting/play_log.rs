use serde::Serialize;

use super::item_event::{self, PlayerItemReportingEvent};
use super::payload::{audio_mode, quality, reporting_event, FindEvent, MaybeFrom, ToTimestamp};
use super::records::PlayerItemReportingRecords;
use crate::model::{MediaPresentation, MediaProduct, MediaTime};
use crate::platforms::events::ReportingEvent;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlayLogEvent {
    pub playback_session_id: String,
    pub start_timestamp: u64,
    pub start_asset_position: f64,
    pub is_post_paywall: bool,
    pub product_type: String,
    pub requested_product_id: String,
    pub actual_product_id: String,
    pub actual_asset_presentation: MediaPresentation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_audio_mode: Option<String>,
    pub actual_quality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub actions: Actions,
    pub end_timestamp: u64,
    pub end_asset_position: f64,
}

impl MaybeFrom<&PlayerItemReportingRecords> for PlayLogEvent {
    fn maybe_from(records: &PlayerItemReportingRecords) -> Option<Self> {
        let events = records.events.as_slice();
        events
            .iter()
            .find_first_event::<item_event::PlaybackRequested>()?;

        let session_started = events
            .iter()
            .find_first_event::<item_event::SessionStarted>()?;

        let start = events.iter().find_first_event::<item_event::Started>()?;

        let manifest_retrieval_ended = events
            .iter()
            .find_first_event::<item_event::ManifestRetrievalEnded>()?;

        let product_info = manifest_retrieval_ended.result.as_ref().ok()?;

        let requested_product_id = match &session_started.product {
            MediaProduct::Track { id } | MediaProduct::Video { id } => id,
        };

        let (product_type, actual_product_id) = match &product_info.product {
            MediaProduct::Track { id } => ("TRACK", id),
            MediaProduct::Video { id } => ("VIDEO", id),
        };

        let ended = events.iter().find_first_event::<item_event::Ended>();
        let failed = events.iter().find_first_event::<item_event::Failed>();
        let end_timestamp = failed
            .map(|event| event.timestamp)
            .or_else(|| ended.map(|event| event.timestamp))
            .unwrap_or_else(web_time::SystemTime::now)
            .to_timestamp();

        Some(Self {
            playback_session_id: records.session_id.to_string(),
            start_timestamp: start.timestamp.to_timestamp(),
            start_asset_position: start.position.as_secs_f64(),
            is_post_paywall: true,
            product_type: product_type.into(),
            requested_product_id: requested_product_id.clone(),
            actual_product_id: actual_product_id.clone(),
            actual_asset_presentation: product_info.presentation,
            actual_audio_mode: audio_mode(&product_info.product, &product_info.formats),
            actual_quality: quality(&product_info.product, &product_info.formats),
            source_type: None,
            source_id: None,
            actions: Actions::from(events),
            end_timestamp,
            end_asset_position: last_recorded_position(events)
                .unwrap_or_default()
                .as_secs_f64(),
        })
    }
}

fn last_recorded_position(events: &[PlayerItemReportingEvent]) -> Option<MediaTime> {
    events.iter().rev().find_map(|event| match event {
        PlayerItemReportingEvent::Started(event) => Some(event.position),
        PlayerItemReportingEvent::Paused(event) => Some(event.position),
        PlayerItemReportingEvent::Stalled(event) => Some(event.position),
        PlayerItemReportingEvent::Resumed(event) => Some(event.position),
        PlayerItemReportingEvent::Seeking(event) => Some(event.position),
        PlayerItemReportingEvent::Seeked(event) => Some(event.position),
        PlayerItemReportingEvent::Ended(event) => Some(event.position),
        PlayerItemReportingEvent::Failed(event) => event.position,
        PlayerItemReportingEvent::Position(event) => Some(event.position),
        _ => None,
    })
}

impl TryFrom<PlayLogEvent> for ReportingEvent {
    type Error = serde_json::Error;

    fn try_from(event: PlayLogEvent) -> Result<Self, Self::Error> {
        reporting_event("play_log", "playback_session", event)
    }
}

#[derive(Debug, Serialize)]
#[serde(transparent)]
pub(crate) struct Actions(pub(crate) Vec<Action>);

impl From<&[PlayerItemReportingEvent]> for Actions {
    fn from(events: &[PlayerItemReportingEvent]) -> Self {
        let mut actions: Vec<_> = events
            .iter()
            .filter_map(Action::maybe_from)
            .skip_while(|action| action.action_type == ActionType::PlaybackStart)
            .collect();

        actions.dedup_by(|current, previous| current.action_type == previous.action_type);

        Self(actions)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Action {
    pub action_type: ActionType,
    pub asset_position: f64,
    pub timestamp: u64,
}

impl MaybeFrom<&PlayerItemReportingEvent> for Action {
    fn maybe_from(event: &PlayerItemReportingEvent) -> Option<Self> {
        let (action_type, position, timestamp) = match event {
            PlayerItemReportingEvent::Stalled(event) => {
                (ActionType::PlaybackStop, event.position, event.timestamp)
            }
            PlayerItemReportingEvent::Paused(event) => {
                (ActionType::PlaybackStop, event.position, event.timestamp)
            }
            PlayerItemReportingEvent::Seeking(event) => {
                (ActionType::PlaybackStop, event.position, event.timestamp)
            }
            PlayerItemReportingEvent::Started(event) => {
                (ActionType::PlaybackStart, event.position, event.timestamp)
            }
            PlayerItemReportingEvent::Position(event) => {
                (ActionType::PlaybackStart, event.position, event.timestamp)
            }
            PlayerItemReportingEvent::Resumed(event) => {
                (ActionType::PlaybackStart, event.position, event.timestamp)
            }
            _ => return None,
        };

        Some(Self {
            action_type,
            asset_position: position.as_secs_f64(),
            timestamp: timestamp.to_timestamp(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ActionType {
    PlaybackStart,
    PlaybackStop,
}
