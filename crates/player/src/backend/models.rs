use serde::Deserialize;

use crate::model::{MediaPresentation, PreviewReason};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TrackManifestDocument {
    pub data: Option<TrackManifestResource>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TrackManifestResource {
    pub id: String,
    pub attributes: Option<TrackManifestAttributes>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackManifestAttributes {
    pub uri: Option<String>,
    pub formats: Option<Vec<String>>,
    pub album_audio_normalization_data: Option<AudioNormalization>,
    pub track_audio_normalization_data: Option<AudioNormalization>,
    pub drm_data: Option<DrmData>,
    pub preview_reason: Option<PreviewReason>,
    pub track_presentation: Option<MediaPresentation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AudioNormalization {
    pub peak_amplitude: Option<f32>,
    pub replay_gain: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DrmData {
    pub drm_system: DrmSystem,
    pub license_url: String,
    pub certificate_url: String,
    pub init_data: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) enum DrmSystem {
    #[serde(rename = "FAIRPLAY")]
    Fairplay,
    #[serde(rename = "WIDEVINE")]
    Widevine,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct VideoManifestDocument {
    pub data: Option<VideoManifestResource>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct VideoManifestResource {
    pub id: String,
    pub attributes: Option<VideoManifestAttributes>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VideoManifestAttributes {
    pub link: Option<LinkObject>,
    pub drm_data: Option<DrmData>,
    pub preview_reason: Option<PreviewReason>,
    pub video_presentation: Option<MediaPresentation>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LinkObject {
    pub href: Option<String>,
}
