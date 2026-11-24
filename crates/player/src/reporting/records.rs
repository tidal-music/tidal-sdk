use std::sync::Arc;

use uuid::Uuid;

use super::item_event::PlayerItemReportingEvent;
use super::payload::MaybeFrom;
use super::play_log::PlayLogEvent;
use super::streaming_metrics::StreamingMetrics;
use crate::platforms::events::ReportingEvent;
use crate::platforms::TidalEventSender;

pub(crate) struct PlayerItemReportingRecords {
    pub(crate) events: Vec<PlayerItemReportingEvent>,
    pub(crate) session_id: Uuid,
    reporting_sender: Arc<dyn TidalEventSender>,
}

impl PlayerItemReportingRecords {
    pub(crate) fn new(
        events: Vec<PlayerItemReportingEvent>,
        reporting_sender: Arc<dyn TidalEventSender>,
    ) -> Self {
        Self {
            events,
            session_id: Uuid::new_v4(),
            reporting_sender,
        }
    }

    pub(crate) fn take_events(mut self) -> Vec<PlayerItemReportingEvent> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn push_event(&mut self, event: PlayerItemReportingEvent) {
        use PlayerItemReportingEvent::Position;

        let Position(_) = event else {
            self.events.push(event);
            return;
        };

        // Keep the first rendering timestamp and latest position in each progress run.
        let last_two_events = self.events.last_chunk_mut::<2>();
        let Some([Position(_), second @ Position(_)]) = last_two_events else {
            self.events.push(event);
            return;
        };

        *second = event;
    }
}

impl Drop for PlayerItemReportingRecords {
    fn drop(&mut self) {
        PlayLogEvent::maybe_from(&*self)
            .into_iter()
            .filter_map(|event| ReportingEvent::try_from(event).ok())
            .for_each(|event| self.reporting_sender.send_event(event));

        StreamingMetrics::from(&*self)
            .0
            .into_iter()
            .filter_map(|event| ReportingEvent::try_from(event).ok())
            .for_each(|event| self.reporting_sender.send_event(event));
    }
}
