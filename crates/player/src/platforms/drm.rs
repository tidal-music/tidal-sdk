use std::sync::Arc;

/// Wraps the platform CDM. The core performs all certificate and license HTTP.
/// With preloading, the core installs each seed's license through a [`LicenseSession`] while
/// preparing it, and the media player attaches the returned handle. Without preloading, the
/// media player fetches the certificate and license through the item when its engine asks.
/// Pair the provider with a media factory that understands its handles.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait DrmProvider: Send + Sync {
    fn preloads_licenses(&self) -> bool;

    /// Called once per protected seed, only when [`DrmProvider::preloads_licenses`] is true.
    fn create_session(&self) -> Arc<dyn LicenseSession>;
}

/// Acquires one seed's license: the core fetches the certificate, asks for a challenge, posts
/// it and hands back the license to install. The core keeps the session as long as the seed.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
#[async_trait::async_trait]
pub trait LicenseSession: Send + Sync {
    fn certificate_request(&self) -> CertificateRequest;

    async fn create_challenge(
        &self,
        certificate: Vec<u8>,
        init_data: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, Error>;

    /// Returns an opaque handle that the media player receives as `DrmInput::Prepared`.
    async fn install_license(&self, license: Vec<u8>) -> Result<Vec<u8>, Error>;
}

/// How the core requests the certificate from the manifest's certificate URL.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum CertificateRequest {
    Get,
    Post(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi", uniffi(name = "DrmError"))]
pub enum Error {
    #[error("{description}")]
    Failed {
        description: String,
        code: Option<String>,
    },
}
