use std::any::Any;
use std::sync::Arc;
use std::time::Duration;
use web_time::SystemTime;

use super::drm::CertificateRequest;
use crate::model::error::{network, other, ErrorSegment};
use crate::model::{AudioFormat, Error as PlayerError, MediaTime, NormalizationData};

/// Creates an independently controlled media instance for each call.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait MediaPlayerFactory: Send + Sync {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer>;
}

/// Receives timestamped facts synchronously in occurrence order. Record emission time
/// before calling; the callback queues each fact without waiting for processing.
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub trait MediaPlayerEvents: Send + Sync {
    fn on_event(&self, event: MediaEvent);
}

/// One playback occurrence handed to the media player. Only Rust implements it: the host
/// receives items, reports them back in events and reads the media to render.
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[async_trait::async_trait]
pub trait MediaItem: Send + Sync + Any {
    fn media_input(&self) -> MediaInput;
    /// Track-normalized output volume before applying a transition's fade gain.
    fn volume(&self) -> f32;
    /// Fetches the certificate the engine's CDM asked for. Fails for content without DRM.
    async fn fetch_certificate(
        self: Arc<Self>,
        request: CertificateRequest,
    ) -> Result<Vec<u8>, PlayerError>;
    /// Posts the CDM's challenge and returns the license for the engine to install.
    /// Fails for content without DRM.
    async fn fetch_license(self: Arc<Self>, challenge: Vec<u8>) -> Result<Vec<u8>, PlayerError>;
}

/// Controls one platform media instance. Without a current item, pause, resume and seek do
/// nothing.
/// Implementations retain the injected callback and report readiness and rendering as events.
/// Attach a prepared license, or fetch one through the item when the engine asks. Licenses are
/// not renewed.
/// Installation failure from set_current_and_play, set_current or set_next is returned as Err,
/// without a Failed callback. Earlier callbacks, including Preparing and Current, are allowed.
/// An accepted next item that fails asynchronously stays selected as next. Report
/// Failed once, then Current { item: Some(item) } when advancement reaches it; do not render
/// or silently discard it. A failed current item may be released.
/// Core calls dispose when releasing its media owner, even if the host retains this object.
/// A dropped command future may still reach the platform later: foreign bindings run each
/// call in a task that dropping only marks as cancelled. Ignore such commands.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
#[async_trait::async_trait]
pub trait MediaPlayer: Send + Sync {
    /// Replace current, clear next, and request playback. Apply item volume before rendering.
    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), Error>;
    /// Replace current and clear next, preserving playback intent (initially paused).
    /// Stop replaced callbacks, preserving already reported facts and occurrence identity.
    async fn set_current(&self, item: Arc<dyn MediaItem>) -> Result<(), Error>;
    /// Update next atomically with advancement; leave placement unchanged on failure.
    /// Apply its volume only when it becomes current. Without a current item, select it as
    /// current, preserving playback intent.
    async fn set_next(&self, item: Option<Arc<dyn MediaItem>>) -> Result<(), Error>;
    async fn pause(&self);
    async fn resume(&self);
    /// Emit Seeking, then start the engine's seek.
    /// Complete when the engine has finished seeking. A seek interrupted by another seek,
    /// replacement or reset completes without Seeked. Return Err only if the engine rejects
    /// the seek.
    async fn seek(&self, position: MediaTime) -> Result<(), Error>;
    /// Apply the final output gain, including normalization and fades, before completing.
    async fn set_volume(&self, gain: f32);
    /// A pending operation's future is dropped before reset runs. Cancel work that
    /// outlives it, suppress old callbacks, and clear placement without emitting events.
    /// Preserve all facts already reported before reset or replacement.
    /// A replacement may cancel this future and call reset again.
    async fn reset(&self);

    /// Permanently retire this instance. Stop rendering and callbacks, cancel outstanding
    /// work and initiate resource cleanup before returning. Must be idempotent.
    /// Called once by the core after dropping any in-flight media command future.
    /// Unlike reset, this synchronous hook makes the instance unusable.
    fn dispose(&self);
}

/// Report actions and state changes for all items, including the staged successor.
/// Position payloads describe the asset when the event is reported.
/// Emit at most one terminal observation per item: `Ended` or `Failed`.
/// Do not emit further observations for that item after its terminal event, except `Current`.
#[derive(Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum MediaPlayerEvent {
    /// Report the current playback item, or `None` when selection is cleared. Reset emits no events.
    Current { item: Option<Arc<dyn MediaItem>> },
    /// The current item ended naturally and there is no accepted successor.
    /// Emit after Ended, instead of Current { item: None }. Never emit for reset or failure.
    Completed,
    /// Emit when the item's asset duration becomes known or changes.
    Duration(Arc<dyn MediaItem>, Duration),
    /// Report the media position while playback advances, about every 100 ms.
    /// Never emit while paused, or between Seeking and the engine reaching the seek target.
    Position(Arc<dyn MediaItem>, PlaybackPosition),
    /// Emit before the media engine seeks, carrying the actual source position.
    Seeking(Arc<dyn MediaItem>, MediaTime),
    /// Emit once the engine is done seeking. Carry the observed position.
    Seeked(Arc<dyn MediaItem>, PlaybackPosition),
    /// Emit when the media engine begins preparing the item.
    Preparing(Arc<dyn MediaItem>),
    /// Emit when the item is ready to play.
    Prepared(Arc<dyn MediaItem>),
    /// Emit once when the item first begins rendering in this playback occurrence.
    Started(Arc<dyn MediaItem>, MediaTime),
    /// Emit when playback of the item has paused.
    Paused(Arc<dyn MediaItem>, PlaybackPosition),
    /// Emit when playback is requested but insufficient buffered media prevents rendering.
    /// The next Resumed or Position ends the stall.
    Stalled(Arc<dyn MediaItem>, PlaybackPosition),
    /// Emit when rendering resumes after a pause or stall.
    Resumed(Arc<dyn MediaItem>, PlaybackPosition),
    /// Emit when the item reaches successful natural completion.
    Ended(Arc<dyn MediaItem>, MediaTime),
    /// Emit when playback fails after the item was successfully loaded.
    /// Installation rejected by a set_* method is reported only through its returned Err.
    Failed(Arc<dyn MediaItem>, MediaTime, Error),
}

impl MediaPlayerEvent {
    pub(crate) fn item(&self) -> Option<&Arc<dyn MediaItem>> {
        match self {
            Self::Completed => None,
            Self::Current { item } => item.as_ref(),
            Self::Position(item, _)
            | Self::Duration(item, _)
            | Self::Seeking(item, _)
            | Self::Seeked(item, _)
            | Self::Preparing(item)
            | Self::Prepared(item)
            | Self::Started(item, _)
            | Self::Paused(item, _)
            | Self::Stalled(item, _)
            | Self::Resumed(item, _)
            | Self::Ended(item, _)
            | Self::Failed(item, _, _) => Some(item),
        }
    }
}

/// Asset position and remaining duration observed at event emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlaybackPosition {
    pub position: MediaTime,
    pub remaining: Duration,
}

#[derive(Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct MediaEvent {
    pub event: MediaPlayerEvent,
    /// Record when the action or state change occurred, before callback delivery or buffering.
    pub emitted_at: SystemTime,
}

impl MediaEvent {
    pub fn new(event: MediaPlayerEvent) -> Self {
        Self {
            event,
            emitted_at: SystemTime::now(),
        }
    }
}

#[derive(Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct MediaInput {
    pub url: String,
    /// None for content without DRM.
    pub drm: Option<DrmInput>,
    pub formats: Vec<AudioFormat>,
    pub album_normalization_data: Option<NormalizationData>,
    pub track_normalization_data: Option<NormalizationData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DrmInput {
    /// A license installed while preparing playback; attach it.
    Prepared { handle: Vec<u8> },
    /// Fetch the certificate and license through the item when the engine asks.
    OnDemand,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi", uniffi(name = "MediaError"))]
pub enum Error {
    #[error("network error ({code}): {description}")]
    Network { code: String, description: String },
    #[error("content error ({code}): {description}")]
    Content { code: String, description: String },
    #[error("other error ({code}): {description}")]
    Other { code: String, description: String },
}

impl From<Error> for PlayerError {
    fn from(error: Error) -> Self {
        ErrorSegment::PlaybackError.wrap(match &error {
            Error::Network { code, .. } => {
                network(&error, ErrorSegment::NetworkError.code(Some(code)))
            }
            Error::Content { code, .. } => {
                other(&error, ErrorSegment::ContentError.code(Some(code)))
            }
            Error::Other { code, .. } => {
                other(&error, ErrorSegment::PlaybackError.code(Some(code)))
            }
        })
    }
}
