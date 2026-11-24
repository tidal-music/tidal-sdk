use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use web_time::SystemTime;

use super::http_client::{self, HttpClient};
use super::manifest_fetcher::DrmInfo;
use crate::model::error::{http_failure, other, ErrorSegment};
use crate::model::Error as PlayerError;
use crate::platforms::authenticator::Authenticator;
use crate::platforms::drm::{self, CertificateRequest, LicenseSession};
use crate::platforms::transport::{HttpMethod, HttpRequest, HttpTransport};
use crate::reporting::item_event;
use crate::reporting::{PlayerItemReportingEvent, PlayerItemReportingRecords};

pub(crate) struct LicenseFetcher {
    client: HttpClient,
    certificates: CertificateCache,
}

impl LicenseFetcher {
    pub(crate) fn new(
        transport: Arc<dyn HttpTransport>,
        authenticator: Arc<dyn Authenticator>,
    ) -> Self {
        Self {
            client: HttpClient::new(transport, Some(authenticator)),
            certificates: CertificateCache::default(),
        }
    }

    pub(crate) async fn acquire(
        &self,
        drm: &DrmInfo,
        session: &dyn LicenseSession,
        records: &Mutex<PlayerItemReportingRecords>,
    ) -> Result<Vec<u8>, PlayerError> {
        record(
            records,
            PlayerItemReportingEvent::LicenseRetrievalStarted(
                item_event::LicenseRetrievalStarted {
                    timestamp: SystemTime::now(),
                },
            ),
        );

        let handle = async {
            let certificate = self
                .fetch_certificate(Some(drm), session.certificate_request())
                .await?;

            let challenge = session
                .create_challenge(certificate, drm.init_data.clone())
                .await
                .map_err(Error::Session)?;

            let license = self.fetch_license(Some(drm), challenge).await?;

            Ok(session
                .install_license(license)
                .await
                .map_err(Error::Session)?)
        }
        .await;

        record(
            records,
            PlayerItemReportingEvent::LicenseRetrievalEnded(item_event::LicenseRetrievalEnded {
                timestamp: SystemTime::now(),
                result: handle.as_ref().map(|_| ()).map_err(Clone::clone),
            }),
        );

        handle
    }

    pub(crate) async fn fetch_certificate(
        &self,
        drm: Option<&DrmInfo>,
        request: CertificateRequest,
    ) -> Result<Vec<u8>, PlayerError> {
        let drm = drm.ok_or(Error::NoDrm)?;
        let key = CertificateCacheKey {
            certificate_url: drm.certificate_url.clone(),
            certificate_request: request,
        };

        if let Some(certificate) = self.certificates.get(&key) {
            return Ok(certificate);
        }

        let url = key.certificate_url.clone();
        let request = match key.certificate_request.clone() {
            CertificateRequest::Get => HttpRequest {
                method: HttpMethod::Get,
                url,
                headers: HashMap::new(),
                body: None,
            },
            CertificateRequest::Post(body) => post(url, body),
        };

        let certificate = send(&self.client, request).await?;
        self.certificates.insert(key, certificate.clone());
        Ok(certificate)
    }

    pub(crate) async fn fetch_license(
        &self,
        drm: Option<&DrmInfo>,
        challenge: Vec<u8>,
    ) -> Result<Vec<u8>, PlayerError> {
        let drm = drm.ok_or(Error::NoDrm)?;
        Ok(send(&self.client, post(drm.license_url.clone(), challenge)).await?)
    }
}

// Certificates do not change in practice, so only the last one is kept.
#[derive(Default)]
struct CertificateCache(Mutex<Option<(CertificateCacheKey, Vec<u8>)>>);

#[derive(PartialEq, Eq)]
struct CertificateCacheKey {
    certificate_url: String,
    certificate_request: CertificateRequest,
}

impl CertificateCache {
    fn get(&self, key: &CertificateCacheKey) -> Option<Vec<u8>> {
        match &*self.0.lock().unwrap() {
            Some((cached, certificate)) if cached == key => Some(certificate.clone()),
            _ => None,
        }
    }

    fn insert(&self, key: CertificateCacheKey, certificate: Vec<u8>) {
        *self.0.lock().unwrap() = Some((key, certificate));
    }
}

fn record(records: &Mutex<PlayerItemReportingRecords>, event: PlayerItemReportingEvent) {
    records.lock().unwrap().push_event(event);
}

fn post(url: String, body: Vec<u8>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url,
        headers: HashMap::from([(
            "Content-Type".to_string(),
            "application/octet-stream".to_string(),
        )]),
        body: Some(body),
    }
}

async fn send(client: &HttpClient, request: HttpRequest) -> Result<Vec<u8>, Error> {
    let response = client.execute(request).await?;
    match response.status_code {
        200..=299 => Ok(response.body),
        http_status => Err(Error::UnsuccessfulResponse { http_status }),
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("unsuccessful DRM HTTP response ({http_status})")]
    UnsuccessfulResponse { http_status: u16 },
    #[error(transparent)]
    HttpClient(#[from] http_client::Error),
    #[error(transparent)]
    Session(#[from] drm::Error),
    #[error("content has no DRM")]
    NoDrm,
}

impl From<Error> for PlayerError {
    fn from(error: Error) -> Self {
        Self::from(&error)
    }
}

impl From<&Error> for PlayerError {
    fn from(error: &Error) -> Self {
        match error {
            Error::UnsuccessfulResponse { http_status } => {
                ErrorSegment::DrmError.wrap(http_failure(error, *http_status))
            }
            Error::HttpClient(error) => ErrorSegment::DrmError.wrap(Self::from(error)),
            Error::Session(drm::Error::Failed { code, .. }) => ErrorSegment::DrmError
                .wrap(other(error, ErrorSegment::OtherError.code(code.as_deref()))),
            Error::NoDrm => ErrorSegment::DrmError.wrap(other(error, "nd")),
        }
    }
}
