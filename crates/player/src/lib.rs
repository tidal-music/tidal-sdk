//! Shared playback core with platform-provided media, DRM, authentication, and HTTP.
//!
//! [`Player`] prepares reusable [`PlaybackSeed`]s and plays them through the host's
//! [`MediaPlayerFactory`], either in sequence or crossfaded as [`PlayerConfig::crossfade`]
//! selects. Media events carry each [`MediaItem`] through selection and observations.

#![deny(unreachable_pub)]

mod backend;
#[cfg(feature = "uniffi")]
mod bindings;
mod loading;
mod model;
mod platforms;
mod playback;
mod player;
mod player_item;
mod reporting;

pub use loading::PlaybackSeed;
pub use model::{
    AudioFormat, CrossfadeConfig, DenialReason, Error, ManifestConfiguration, ManifestType,
    ManifestUsage, MediaProduct, MediaTime, NormalizationData, PlayerConfig, PlayerEvent,
    PlayerState, TrackManifestConfiguration, UriScheme, VideoManifestConfiguration,
};
pub use platforms::authenticator::{Authenticator, Error as AuthenticatorError};
pub use platforms::drm::{CertificateRequest, DrmProvider, Error as DrmError, LicenseSession};
pub use platforms::events::{ReportingEvent, TidalEventSender};
pub use platforms::media::{
    DrmInput, Error as MediaError, MediaEvent, MediaInput, MediaItem, MediaPlayer,
    MediaPlayerEvent, MediaPlayerEvents, MediaPlayerFactory, PlaybackPosition,
};
pub use platforms::player_events::PlayerEvents;
pub use platforms::transport::{
    Error as TransportError, HttpMethod, HttpRequest, HttpResponse, HttpTransport,
};
pub use player::{Player, PlayerDependencies};

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(feature = "uniffi", target_arch = "wasm32"))]
extern crate uniffi_runtime_wasm as _;
