use serde::{Deserialize, Serialize};

use super::{AudioFormat, MediaProduct};

/// A nonnegative position measured from the beginning of an asset.
pub type MediaTime = std::time::Duration;
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct NormalizationData {
    #[serde(default)]
    pub peak_amplitude: Option<f32>,
    #[serde(default)]
    pub replay_gain: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaPresentation {
    Full,
    Preview,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PreviewReason {
    FullRequiresSubscription,
    FullRequiresPurchase,
    FullRequiresHigherAccessTier,
    #[serde(other)]
    Unknown,
}

/// The product and asset properties returned by manifest retrieval.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaProductInfo {
    pub product: MediaProduct,
    pub presentation: MediaPresentation,
    pub preview_reason: Option<PreviewReason>,
    pub formats: Vec<AudioFormat>,
}
