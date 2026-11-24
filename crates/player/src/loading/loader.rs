use std::sync::{Arc, Mutex};

use channel::runtime::Runtime;

use super::playback_seed::{PlaybackSeed, SeedData};
use crate::backend::license_fetcher::LicenseFetcher;
use crate::backend::manifest_fetcher::DrmInfo;
use crate::backend::manifest_fetcher::{self, ManifestFetcher};
use crate::backend::Manifest;
use crate::model::{MediaProduct, MediaProductInfo};
use crate::platforms::drm::LicenseSession;
use crate::platforms::media::{DrmInput, MediaInput};
use crate::platforms::{DrmProvider, TidalEventSender};
use crate::reporting::item_event;
use crate::reporting::{PlayerItemReportingEvent, PlayerItemReportingRecords};

#[derive(Clone)]
pub(crate) struct Loader {
    manifest_fetcher: Arc<ManifestFetcher>,
    license_fetcher: Arc<LicenseFetcher>,
    drm_provider: Arc<dyn DrmProvider>,
    event_sender: Arc<dyn TidalEventSender>,
    runtime: Runtime,
}

impl Loader {
    pub(crate) fn new(
        manifest_fetcher: Arc<ManifestFetcher>,
        license_fetcher: Arc<LicenseFetcher>,
        drm_provider: Arc<dyn DrmProvider>,
        event_sender: Arc<dyn TidalEventSender>,
    ) -> Self {
        Self {
            manifest_fetcher,
            license_fetcher,
            drm_provider,
            event_sender,
            runtime: Runtime::current(),
        }
    }

    pub(crate) fn prepare(&self, product: MediaProduct) -> Arc<PlaybackSeed> {
        let loader = self.clone();

        let records = Mutex::new(PlayerItemReportingRecords::new(
            vec![PlayerItemReportingEvent::SessionStarted(
                item_event::SessionStarted {
                    timestamp: web_time::SystemTime::now(),
                    product: product.clone(),
                },
            )],
            self.event_sender.clone(),
        ));

        PlaybackSeed::new(&self.runtime, product.clone(), async move {
            let mut records = records;
            let manifest = loader
                .load_manifest(product, records.get_mut().unwrap())
                .await?;
            let (drm, license_session) = match &manifest.drm {
                Some(info) => {
                    let (input, session) = loader.load_license(info, &records).await?;
                    (Some(input), session)
                }
                None => (None, None),
            };

            Ok(SeedData {
                input: MediaInput {
                    url: manifest.manifest_url,
                    drm,
                    album_normalization_data: manifest
                        .playback_metadata
                        .as_ref()
                        .and_then(|m| m.album_normalization_data),
                    track_normalization_data: manifest
                        .playback_metadata
                        .as_ref()
                        .and_then(|m| m.track_normalization_data),
                    formats: manifest
                        .playback_metadata
                        .map(|m| m.formats)
                        .unwrap_or_default(),
                },
                drm: manifest.drm,
                license_session,
                events: records.into_inner().unwrap().take_events(),
            })
        })
    }

    async fn load_manifest(
        &self,
        product: MediaProduct,
        records: &mut PlayerItemReportingRecords,
    ) -> Result<Manifest, Error> {
        records.push_event(PlayerItemReportingEvent::ManifestRetrievalStarted(
            item_event::ManifestRetrievalStarted {
                timestamp: web_time::SystemTime::now(),
            },
        ));

        let manifest = self
            .manifest_fetcher
            .fetch_manifest(product)
            .await
            .map_err(Arc::new);

        records.push_event(PlayerItemReportingEvent::ManifestRetrievalEnded(
            item_event::ManifestRetrievalEnded {
                timestamp: web_time::SystemTime::now(),
                result: manifest
                    .as_ref()
                    .map(MediaProductInfo::from)
                    .map_err(|error| crate::model::Error::from(error.as_ref())),
            },
        ));

        Ok(manifest?)
    }

    async fn load_license(
        &self,
        drm: &DrmInfo,
        records: &Mutex<PlayerItemReportingRecords>,
    ) -> Result<(DrmInput, Option<Arc<dyn LicenseSession>>), Error> {
        if !self.drm_provider.preloads_licenses() {
            return Ok((DrmInput::OnDemand, None));
        }

        let session = self.drm_provider.create_session();
        let handle = self
            .license_fetcher
            .acquire(drm, session.as_ref(), records)
            .await
            .map_err(Error::License)?;

        Ok((DrmInput::Prepared { handle }, Some(session)))
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum Error {
    #[error("Preparation was cancelled")]
    Cancelled,
    #[error(transparent)]
    ManifestFetcher(#[from] Arc<manifest_fetcher::Error>),
    #[error(transparent)]
    License(crate::model::Error),
}
