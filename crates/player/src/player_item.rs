use channel::runtime::Runtime;
use channel::{event_channel, EventReceiver, EventSender};
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web_time::SystemTime;

use crate::backend::license_fetcher::LicenseFetcher;
use crate::backend::manifest_fetcher::DrmInfo;
use crate::model::{Error, MediaProduct, MediaTime, NormalizationData};
use crate::platforms::drm::{CertificateRequest, LicenseSession};
use crate::platforms::media::{MediaEvent, MediaInput, MediaItem, MediaPlayerEvent};
use crate::platforms::TidalEventSender;
use crate::reporting::item_event;
use crate::reporting::{MaybeFrom, PlayerItemReportingEvent, PlayerItemReportingRecords};

pub(crate) struct PlayerItem {
    volume: f32,
    product: MediaProduct,
    input: MediaInput,
    drm: Option<DrmInfo>,
    license_fetcher: Arc<LicenseFetcher>,
    runtime: Runtime,
    _license_session: Option<Arc<dyn LicenseSession>>,
    data: Mutex<PlayerItemData>,
}

impl PlayerItem {
    pub(crate) fn media_product(&self) -> MediaProduct {
        self.product.clone()
    }
}

#[async_trait::async_trait]
impl MediaItem for PlayerItem {
    fn media_input(&self) -> MediaInput {
        self.input.clone()
    }

    fn volume(&self) -> f32 {
        self.volume
    }

    // The media player awaits these on the host's executor, where the core's timers for
    // HTTP retries and token refreshes are unavailable, so the whole fetch runs on the core.
    async fn fetch_certificate(
        self: Arc<Self>,
        request: CertificateRequest,
    ) -> Result<Vec<u8>, Error> {
        let runtime = self.runtime.clone();
        runtime
            .run(async move { self.retrieve_certificate(request).await })
            .await
    }

    async fn fetch_license(self: Arc<Self>, challenge: Vec<u8>) -> Result<Vec<u8>, Error> {
        let runtime = self.runtime.clone();
        runtime
            .run(async move { self.retrieve_license(challenge).await })
            .await
    }
}

impl PlayerItem {
    pub(crate) fn new(
        product: MediaProduct,
        input: MediaInput,
        drm: Option<DrmInfo>,
        license_fetcher: Arc<LicenseFetcher>,
        license_session: Option<Arc<dyn LicenseSession>>,
        events: Vec<PlayerItemReportingEvent>,
        reporting_sender: Arc<dyn TidalEventSender>,
    ) -> Self {
        Self {
            volume: input
                .track_normalization_data
                .as_ref()
                .map_or(1.0, AsVolume::as_volume),
            product,
            input,
            drm,
            license_fetcher,
            runtime: Runtime::current(),
            _license_session: license_session,
            data: Mutex::new(PlayerItemData {
                records: PlayerItemReportingRecords::new(events, reporting_sender),
                state: PlayerItemState::Preparing,
                playback_requested: false,
                position: None,
                duration: None,
                notification_sender: None,
            }),
        }
    }

    pub(crate) fn subscribe(&self) -> EventReceiver<PlayerItemEvents> {
        let mut data = self.data.lock().unwrap();
        let (sender, receiver) = event_channel();
        let position = data
            .position
            .or_else(|| data.duration.map(|_| MediaTime::ZERO));

        if let Some(position) = position {
            let _ = sender.send(PlayerItemEvents::PositionChanged {
                position,
                duration: data.duration,
            });
        }

        data.notification_sender = Some(sender);
        receiver
    }

    pub(crate) fn request_playback(
        &self,
        timestamp: SystemTime,
        reason: item_event::PlaybackRequestReason,
    ) {
        let mut data = self.data.lock().unwrap();
        if !data.playback_requested {
            data.records
                .push_event(PlayerItemReportingEvent::PlaybackRequested(
                    item_event::PlaybackRequested { timestamp, reason },
                ));
            data.playback_requested = true;
        }
    }

    pub(crate) fn state(&self) -> PlayerItemState {
        self.data.lock().unwrap().state.clone()
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.data.lock().unwrap().duration
    }

    pub(crate) fn record_error(&self, error: Error) {
        let timestamp = SystemTime::now();
        let mut data = self.data.lock().unwrap();
        data.state = PlayerItemState::Failed(error.clone());
        data.records
            .push_event(PlayerItemReportingEvent::Failed(item_event::Failed {
                timestamp,
                position: None,
                error,
            }));
    }

    async fn retrieve_certificate(&self, request: CertificateRequest) -> Result<Vec<u8>, Error> {
        self.record(PlayerItemReportingEvent::CertificateRetrievalStarted(
            item_event::CertificateRetrievalStarted {
                timestamp: SystemTime::now(),
            },
        ));

        let certificate = self
            .license_fetcher
            .fetch_certificate(self.drm.as_ref(), request)
            .await;

        self.record(PlayerItemReportingEvent::CertificateRetrievalEnded(
            item_event::CertificateRetrievalEnded {
                timestamp: SystemTime::now(),
                result: certificate.as_ref().map(|_| ()).map_err(Clone::clone),
            },
        ));

        certificate
    }

    async fn retrieve_license(&self, challenge: Vec<u8>) -> Result<Vec<u8>, Error> {
        self.record(PlayerItemReportingEvent::LicenseRetrievalStarted(
            item_event::LicenseRetrievalStarted {
                timestamp: SystemTime::now(),
            },
        ));

        let license = self
            .license_fetcher
            .fetch_license(self.drm.as_ref(), challenge)
            .await;

        self.record(PlayerItemReportingEvent::LicenseRetrievalEnded(
            item_event::LicenseRetrievalEnded {
                timestamp: SystemTime::now(),
                result: license.as_ref().map(|_| ()).map_err(Clone::clone),
            },
        ));

        license
    }

    fn record(&self, event: PlayerItemReportingEvent) {
        self.data.lock().unwrap().records.push_event(event);
    }

    pub(crate) fn from_media(item: &Arc<dyn MediaItem>) -> Option<Arc<Self>> {
        let item: Arc<dyn Any + Send + Sync> = item.clone();
        item.downcast().ok()
    }

    pub(crate) fn on_media_player_event(observation: &MediaEvent) {
        let Some(item) = observation.event.item().and_then(Self::from_media) else {
            return;
        };

        let mut data = item.data.lock().unwrap();
        data.on_media_player_event(&observation.event);
        if let Some(event) = PlayerItemReportingEvent::maybe_from(observation) {
            data.records.push_event(event);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlayerItemState {
    Preparing,
    Playing,
    Paused,
    Stalled,
    Ended,
    Failed(Error),
}

struct PlayerItemData {
    records: PlayerItemReportingRecords,
    state: PlayerItemState,
    playback_requested: bool,
    duration: Option<Duration>,
    position: Option<MediaTime>,
    notification_sender: Option<EventSender<PlayerItemEvents>>,
}

impl PlayerItemData {
    fn on_media_player_event(&mut self, event: &MediaPlayerEvent) {
        if matches!(self.state, PlayerItemState::Failed(_)) {
            return;
        }

        match event {
            MediaPlayerEvent::Preparing(_) => self.set_state(PlayerItemState::Preparing),
            MediaPlayerEvent::Prepared(_) if self.state == PlayerItemState::Preparing => {
                self.set_state(PlayerItemState::Paused);
            }
            MediaPlayerEvent::Started(_, position) => {
                self.set_state(PlayerItemState::Playing);
                self.set_position(*position);
            }
            MediaPlayerEvent::Resumed(_, position) | MediaPlayerEvent::Position(_, position) => {
                self.set_state(PlayerItemState::Playing);
                self.set_position(position.position);
            }
            MediaPlayerEvent::Paused(_, position) => {
                self.set_state(PlayerItemState::Paused);
                self.set_position(position.position);
            }
            MediaPlayerEvent::Stalled(_, position) => {
                self.set_state(PlayerItemState::Stalled);
                self.set_position(position.position);
            }
            MediaPlayerEvent::Ended(_, position) => {
                self.set_state(PlayerItemState::Ended);
                self.set_position(*position);
            }
            MediaPlayerEvent::Failed(_, position, error) => {
                self.set_state(PlayerItemState::Failed(error.clone().into()));
                self.set_position(*position);
            }
            MediaPlayerEvent::Seeking(_, position) => self.set_position(*position),
            MediaPlayerEvent::Seeked(_, position) => self.set_position(position.position),
            MediaPlayerEvent::Duration(_, duration) => self.set_duration(*duration),
            _ => {}
        }
    }

    fn set_state(&mut self, state: PlayerItemState) {
        if self.state == state {
            return;
        }
        self.state = state;
        if let Some(sender) = &self.notification_sender {
            let _ = sender.send(PlayerItemEvents::StateChanged(self.state.clone()));
        }
    }

    fn set_position(&mut self, position: MediaTime) {
        if self.position == Some(position) {
            return;
        }
        self.position = Some(position);
        self.emit_position_changed();
    }

    fn set_duration(&mut self, duration: Duration) {
        if self.duration == Some(duration) {
            return;
        }
        self.duration = Some(duration);
        self.emit_position_changed();
    }

    fn emit_position_changed(&self) {
        if let Some(sender) = &self.notification_sender {
            let _ = sender.send(PlayerItemEvents::PositionChanged {
                position: self.position.unwrap_or_default(),
                duration: self.duration,
            });
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PlayerItemEvents {
    StateChanged(PlayerItemState),
    PositionChanged {
        position: MediaTime,
        duration: Option<Duration>,
    },
}

trait AsVolume {
    fn as_volume(&self) -> f32;
}

impl AsVolume for NormalizationData {
    fn as_volume(&self) -> f32 {
        const PREAMP_DB: f32 = 4.0;

        match (self.replay_gain, self.peak_amplitude) {
            (Some(replay_gain), Some(_)) => {
                10.0_f32.powf((replay_gain + PREAMP_DB) / 20.0).min(1.0)
            }
            _ => 1.0,
        }
    }
}
