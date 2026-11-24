use std::time::Duration;

use super::{CrossfadeConfig, Error, ManifestConfiguration, MediaTime};

#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct PlayerConfig {
    pub base_url: String,
    pub manifest_configuration: ManifestConfiguration,
    /// Crossfade between items; `None` plays them in sequence.
    pub crossfade: Option<CrossfadeConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum MediaProduct {
    Track { id: String },
    Video { id: String },
}

/// State of the whole player. After Failed, call a current load or reset.
/// Other operations have undefined behaviour until then.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum PlayerState {
    /// No current item after startup or reset.
    Idle,
    /// Resolving the requested seed or preparing the current media item.
    Preparing,
    Playing,
    Paused,
    Stalled,
    /// Natural exhaustion, reported by the owner of playback selection.
    Completed,
    /// Terminal until play or reset.
    Failed(Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum PlayerEvent {
    /// Selection and its state at the same observation. None clears the current item.
    CurrentItemChanged {
        item: Option<MediaProduct>,
        state: PlayerState,
    },
    /// During seed resolution, item identifies the requested product.
    StateChanged {
        item: Option<MediaProduct>,
        state: PlayerState,
    },
    /// Observed position, including successful seeks and duration changes while paused.
    PositionChanged {
        item: MediaProduct,
        position: MediaTime,
        duration: Option<Duration>,
    },
}
