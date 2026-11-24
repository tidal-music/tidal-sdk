use std::str::FromStr;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use super::http_client;
use super::models::{
    AudioNormalization, DrmData, DrmSystem, TrackManifestAttributes, TrackManifestDocument,
    VideoManifestDocument,
};
use super::tidal_api_client::{self, TidalApiClient, NO_BODY};
use crate::model::error::{denied, http_failure, other, ErrorSegment};
use crate::model::{
    AudioFormat, DenialReason, Error as PlayerError, ManifestConfiguration, ManifestType,
    ManifestUsage, MediaPresentation, MediaProduct, MediaProductInfo, NormalizationData,
    PreviewReason, TrackManifestConfiguration, UriScheme, VideoManifestConfiguration,
};
use crate::platforms::transport::HttpMethod;

/// Where and with what the license for DRM-protected content is acquired.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmInfo {
    pub license_url: String,
    pub certificate_url: String,
    pub init_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub product: MediaProduct,
    pub manifest_url: String,
    pub drm: Option<DrmInfo>,
    pub playback_metadata: Option<ManifestMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMetadata {
    pub formats: Vec<AudioFormat>,
    #[serde(default)]
    pub presentation: MediaPresentation,
    #[serde(default)]
    pub preview_reason: Option<PreviewReason>,
    #[serde(default)]
    pub album_normalization_data: Option<NormalizationData>,
    #[serde(default)]
    pub track_normalization_data: Option<NormalizationData>,
}

impl From<&Manifest> for MediaProductInfo {
    fn from(manifest: &Manifest) -> Self {
        let metadata = manifest.playback_metadata.as_ref();
        Self {
            product: manifest.product.clone(),
            presentation: metadata
                .map_or(MediaPresentation::Unknown, |metadata| metadata.presentation),
            preview_reason: metadata.and_then(|metadata| metadata.preview_reason),
            formats: metadata
                .map(|metadata| metadata.formats.clone())
                .unwrap_or_default(),
        }
    }
}

pub(crate) struct ManifestFetcher {
    api_client: Arc<TidalApiClient>,
    configuration: RwLock<ManifestConfiguration>,
}

impl ManifestFetcher {
    pub(crate) fn new(
        api_client: Arc<TidalApiClient>,
        configuration: ManifestConfiguration,
    ) -> Self {
        Self {
            api_client,
            configuration: RwLock::new(configuration),
        }
    }

    pub(crate) fn get_configuration(&self) -> ManifestConfiguration {
        self.configuration.read().unwrap().clone()
    }

    pub(crate) fn set_configuration(&self, configuration: ManifestConfiguration) {
        *self.configuration.write().unwrap() = configuration;
    }

    pub(crate) async fn fetch_manifest(&self, product: MediaProduct) -> Result<Manifest, Error> {
        match product {
            MediaProduct::Track { id } => self.fetch_track_manifest(id).await,
            MediaProduct::Video { id } => self.fetch_video_manifest(id).await,
        }
    }

    pub(crate) async fn fetch_track_manifest(&self, track_id: String) -> Result<Manifest, Error> {
        let configuration = self.get_configuration().track_configuration;
        let query = configuration.as_query_params();

        let response = self
            .api_client
            .execute_resource_request(
                HttpMethod::Get,
                "trackManifests",
                &track_id,
                &query,
                NO_BODY,
            )
            .await?;

        let document: TrackManifestDocument = response.decode()?;
        let resource = document.data.ok_or(Error::MissingManifestUrl)?;
        let attributes = resource.attributes;
        let uri = attributes
            .as_ref()
            .and_then(|attrs| attrs.uri.clone())
            .ok_or(Error::MissingManifestUrl)?;

        let drm = attributes
            .as_ref()
            .and_then(|attrs| attrs.drm_data.as_ref())
            .map(AsDrmInfo::as_drm_info)
            .transpose()?;

        Ok(Manifest {
            product: MediaProduct::Track { id: resource.id },
            manifest_url: uri,
            drm,
            playback_metadata: attributes.as_ref().map(playback_metadata),
        })
    }

    pub(crate) async fn fetch_video_manifest(&self, video_id: String) -> Result<Manifest, Error> {
        let configuration = self.get_configuration().video_configuration;
        let query = configuration.as_query_params();

        let response = self
            .api_client
            .execute_resource_request(
                HttpMethod::Get,
                "videoManifests",
                &video_id,
                &query,
                NO_BODY,
            )
            .await?;

        let document: VideoManifestDocument = response.decode()?;
        let resource = document.data.ok_or(Error::MissingManifestUrl)?;
        let attributes = resource.attributes;

        let href = attributes
            .as_ref()
            .and_then(|attrs| attrs.link.as_ref())
            .and_then(|link| link.href.clone())
            .ok_or(Error::MissingManifestUrl)?;

        let drm = attributes
            .as_ref()
            .and_then(|attrs| attrs.drm_data.as_ref())
            .map(AsDrmInfo::as_drm_info)
            .transpose()?;

        Ok(Manifest {
            product: MediaProduct::Video { id: resource.id },
            manifest_url: href,
            drm,
            playback_metadata: attributes.as_ref().map(|attributes| ManifestMetadata {
                presentation: attributes.video_presentation.unwrap_or_default(),
                preview_reason: attributes.preview_reason,
                formats: Vec::new(),
                album_normalization_data: None,
                track_normalization_data: None,
            }),
        })
    }
}

fn playback_metadata(attributes: &TrackManifestAttributes) -> ManifestMetadata {
    ManifestMetadata {
        presentation: attributes.track_presentation.unwrap_or_default(),
        preview_reason: attributes.preview_reason,
        formats: attributes
            .formats
            .iter()
            .flatten()
            .filter_map(|value| AudioFormat::from_str(value).ok())
            .collect(),
        album_normalization_data: attributes
            .album_audio_normalization_data
            .as_ref()
            .map(normalization_data),
        track_normalization_data: attributes
            .track_audio_normalization_data
            .as_ref()
            .map(normalization_data),
    }
}

fn normalization_data(normalization: &AudioNormalization) -> NormalizationData {
    NormalizationData {
        peak_amplitude: normalization.peak_amplitude,
        replay_gain: normalization.replay_gain,
    }
}

impl AsRef<str> for ManifestType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Hls => "HLS",
            Self::MpegDash => "MPEG_DASH",
        }
    }
}

impl FromStr for ManifestType {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "HLS" => Ok(Self::Hls),
            "MPEG_DASH" => Ok(Self::MpegDash),
            _ => Err("Unsupported ManifestType"),
        }
    }
}

impl AsRef<str> for AudioFormat {
    fn as_ref(&self) -> &str {
        match self {
            Self::HeAacV1 => "HEAACV1",
            Self::AacLc => "AACLC",
            Self::Flac => "FLAC",
            Self::FlacHires => "FLAC_HIRES",
            Self::Eac3Joc => "EAC3_JOC",
        }
    }
}

impl FromStr for AudioFormat {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "HEAACV1" => Ok(Self::HeAacV1),
            "AACLC" => Ok(Self::AacLc),
            "FLAC" => Ok(Self::Flac),
            "FLAC_HIRES" => Ok(Self::FlacHires),
            "EAC3_JOC" => Ok(Self::Eac3Joc),
            _ => Err("Unsupported AudioFormat"),
        }
    }
}

impl AsRef<str> for UriScheme {
    fn as_ref(&self) -> &str {
        match self {
            Self::Https => "HTTPS",
            Self::Data => "DATA",
        }
    }
}

impl FromStr for UriScheme {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "HTTPS" => Ok(Self::Https),
            "DATA" => Ok(Self::Data),
            _ => Err("Unsupported UriScheme"),
        }
    }
}

impl AsRef<str> for ManifestUsage {
    fn as_ref(&self) -> &str {
        match self {
            Self::Playback => "PLAYBACK",
            Self::Download => "DOWNLOAD",
        }
    }
}

impl FromStr for ManifestUsage {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "PLAYBACK" => Ok(Self::Playback),
            "DOWNLOAD" => Ok(Self::Download),
            _ => Err("Unsupported ManifestUsage"),
        }
    }
}
impl serde::Serialize for AudioFormat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> serde::Deserialize<'de> for AudioFormat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

trait AsQueryParams {
    fn as_query_params(&self) -> Vec<(&'static str, String)>;
}

impl AsQueryParams for TrackManifestConfiguration {
    fn as_query_params(&self) -> Vec<(&'static str, String)> {
        let mut query = vec![("adaptive", self.adaptive.to_string())];
        for format in &self.audio_formats {
            query.push(("formats", format.as_ref().to_string()));
        }
        query.push(("manifestType", self.manifest_type.as_ref().to_string()));
        query.push(("uriScheme", self.uri_scheme.as_ref().to_string()));
        query.push(("usage", self.usage.as_ref().to_string()));
        query
    }
}

impl AsQueryParams for VideoManifestConfiguration {
    fn as_query_params(&self) -> Vec<(&'static str, String)> {
        vec![
            ("uriScheme", self.uri_scheme.as_ref().to_string()),
            ("usage", self.usage.as_ref().to_string()),
        ]
    }
}

trait AsDrmInfo {
    fn as_drm_info(&self) -> Result<DrmInfo, Error>;
}

impl AsDrmInfo for DrmData {
    fn as_drm_info(&self) -> Result<DrmInfo, Error> {
        if self.drm_system == DrmSystem::Unknown {
            return Err(Error::UnsupportedDrmSystem);
        }

        Ok(DrmInfo {
            license_url: self.license_url.clone(),
            certificate_url: self.certificate_url.clone(),
            init_data: self
                .init_data
                .as_ref()
                .and_then(|init_data| init_data.first())
                .map(|value| value.as_bytes().to_vec()),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Content does not exist or is no longer available")]
    ContentNotFound,
    #[error("Required playback prerequisites are missing")]
    PrerequisiteMissing,
    #[error("Content is unavailable in your location")]
    GeoRestricted,
    #[error("Content must be purchased before playback")]
    PurchaseRequired,
    #[error("Client cannot access this content")]
    ClientNotEntitled,
    #[error("Account is playing on another app or device")]
    ConcurrentPlayback,
    /// An unrecognised unsuccessful HTTP response in the 4xx range.
    #[error("unsuccessful manifest HTTP response ({http_status})")]
    Client { http_status: u16 },
    /// An unsuccessful HTTP response in the 5xx range.
    #[error("unsuccessful manifest HTTP response ({http_status})")]
    Server { http_status: u16 },
    #[error("unexpected manifest HTTP response ({http_status})")]
    UnexpectedHttpStatus { http_status: u16 },
    #[error(transparent)]
    HttpClient(#[from] http_client::Error),
    #[error("{method} {path} body encode failed: {source}")]
    Encode {
        method: &'static str,
        path: String,
        source: serde_json::Error,
    },
    #[error("{method} {path} response decode failed: {source}")]
    Decode {
        method: &'static str,
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid base URL {base_url}")]
    InvalidBaseUrl {
        base_url: String,
        source: Option<url::ParseError>,
    },
    #[error("Manifest URL is missing")]
    MissingManifestUrl,
    #[error("Unsupported DRM system in manifest")]
    UnsupportedDrmSystem,
}

impl From<tidal_api_client::Error> for Error {
    fn from(error: tidal_api_client::Error) -> Self {
        let error_code =
            if let tidal_api_client::Error::UnsuccessfulResponse { error_document, .. } = &error {
                error_document
                    .errors
                    .first()
                    .and_then(|error| error.code.as_deref())
            } else {
                None
            };

        match error {
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 404, ..
            } if error_code == Some("CONTENT_NOT_FOUND") => Self::ContentNotFound,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 403, ..
            } if error_code == Some("PREREQUISITE_MISSING") => Self::PrerequisiteMissing,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 403, ..
            } if error_code == Some("GEO_RESTRICTED") => Self::GeoRestricted,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 403, ..
            } if error_code == Some("PURCHASE_REQUIRED") => Self::PurchaseRequired,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 403, ..
            } if error_code == Some("CLIENT_NOT_ENTITLED") => Self::ClientNotEntitled,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: 403, ..
            } if error_code == Some("CONCURRENT_PLAYBACK") => Self::ConcurrentPlayback,
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: http_status @ 400..=499,
                ..
            } => Self::Client { http_status },
            tidal_api_client::Error::UnsuccessfulResponse {
                http_status: http_status @ 500..=599,
                ..
            } => Self::Server { http_status },
            tidal_api_client::Error::UnsuccessfulResponse { http_status, .. } => {
                Self::UnexpectedHttpStatus { http_status }
            }
            tidal_api_client::Error::HttpClient(error) => Self::HttpClient(error),
            tidal_api_client::Error::Encode {
                method,
                path,
                source,
            } => Self::Encode {
                method,
                path,
                source,
            },
            tidal_api_client::Error::Decode {
                method,
                path,
                source,
            } => Self::Decode {
                method,
                path,
                source,
            },
            tidal_api_client::Error::InvalidBaseUrl { base_url, source } => {
                Self::InvalidBaseUrl { base_url, source }
            }
        }
    }
}

impl From<Error> for PlayerError {
    fn from(error: Error) -> Self {
        Self::from(&error)
    }
}

impl From<&Error> for PlayerError {
    fn from(error: &Error) -> Self {
        ErrorSegment::ManifestError.wrap(match error {
            Error::Client { http_status }
            | Error::Server { http_status }
            | Error::UnexpectedHttpStatus { http_status } => http_failure(error, *http_status),
            Error::ContentNotFound => denied(error, DenialReason::ContentNotFound, "cnf"),
            Error::PrerequisiteMissing => denied(error, DenialReason::PrerequisiteMissing, "prm"),
            Error::GeoRestricted => denied(error, DenialReason::GeoRestricted, "geo"),
            Error::PurchaseRequired => denied(error, DenialReason::PurchaseRequired, "pur"),
            Error::ClientNotEntitled => denied(error, DenialReason::ClientNotEntitled, "cne"),
            Error::ConcurrentPlayback => denied(error, DenialReason::ConcurrentPlayback, "ccp"),
            Error::HttpClient(error) => Self::from(error),
            Error::Encode { .. } => other(error, "enc"),
            Error::Decode { .. } => other(error, "dec"),
            Error::InvalidBaseUrl { .. } => other(error, "ibu"),
            Error::MissingManifestUrl => other(error, "mmu"),
            Error::UnsupportedDrmSystem => other(error, "uds"),
        })
    }
}
