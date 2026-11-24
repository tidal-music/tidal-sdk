#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum AudioFormat {
    HeAacV1,
    AacLc,
    Flac,
    FlacHires,
    Eac3Joc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum ManifestType {
    Hls,
    MpegDash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum UriScheme {
    Https,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum ManifestUsage {
    Playback,
    Download,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct ManifestConfiguration {
    pub track_configuration: TrackManifestConfiguration,
    pub video_configuration: VideoManifestConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TrackManifestConfiguration {
    pub manifest_type: ManifestType,
    pub audio_formats: Vec<AudioFormat>,
    pub adaptive: bool,
    pub uri_scheme: UriScheme,
    pub usage: ManifestUsage,
}

impl Default for TrackManifestConfiguration {
    fn default() -> Self {
        Self {
            manifest_type: ManifestType::Hls,
            audio_formats: vec![AudioFormat::HeAacV1],
            adaptive: false,
            uri_scheme: UriScheme::Data,
            usage: ManifestUsage::Download,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct VideoManifestConfiguration {
    pub uri_scheme: UriScheme,
    pub usage: ManifestUsage,
}

impl Default for VideoManifestConfiguration {
    fn default() -> Self {
        Self {
            uri_scheme: UriScheme::Data,
            usage: ManifestUsage::Download,
        }
    }
}

/// Plays the end of an item over the start of its successor. Without it, items play in sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct CrossfadeConfig {
    pub overlap: std::time::Duration,
}
