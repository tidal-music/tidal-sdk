use std::future::Future;
use std::sync::Arc;

use channel::runtime::{Runtime, Task};
use tokio::sync::watch;

use super::loader;
use crate::backend::manifest_fetcher::DrmInfo;
use crate::model::MediaProduct;
use crate::platforms::drm::LicenseSession;
use crate::platforms::media::MediaInput;
use crate::reporting::PlayerItemReportingEvent;

/// Shared manifest and DRM preparation, reusable across playback occurrences.
/// Dropping the last Arc cancels unfinished preparation.
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct PlaybackSeed {
    pub(crate) product: MediaProduct,
    result: watch::Receiver<Option<Result<SeedData, loader::Error>>>,
    task: Task,
}

impl PlaybackSeed {
    pub(crate) fn new<Fut>(runtime: &Runtime, product: MediaProduct, preparation: Fut) -> Arc<Self>
    where
        Fut: Future<Output = Result<SeedData, loader::Error>> + Send + 'static,
    {
        let (sender, result) = watch::channel(None);
        let task = runtime.spawn(async move {
            let _ = sender.send(Some(preparation.await));
        });

        Arc::new(Self {
            product,
            result,
            task,
        })
    }

    pub(crate) async fn resolve(&self) -> Result<SeedData, loader::Error> {
        let mut result = self.result.clone();
        let result = result
            .wait_for(|result| result.is_some())
            .await
            .map_err(|_| loader::Error::Cancelled)?;

        result
            .as_ref()
            .expect("wait_for requires a completed result")
            .clone()
    }
}

#[derive(Clone)]
pub(crate) struct SeedData {
    pub input: MediaInput,
    /// Present for DRM-protected content.
    pub drm: Option<DrmInfo>,
    /// The session behind a prepared license, kept alive with the seed and its items.
    pub license_session: Option<Arc<dyn LicenseSession>>,
    pub events: Vec<PlayerItemReportingEvent>,
}

impl Drop for PlaybackSeed {
    fn drop(&mut self) {
        self.task.abort();
    }
}
