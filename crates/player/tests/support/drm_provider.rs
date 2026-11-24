use player::{DrmProvider, LicenseSession};
use std::sync::Arc;

/// Leaves certificate and license fetches to the media player.
pub struct NoopDrmProvider;

impl DrmProvider for NoopDrmProvider {
    fn preloads_licenses(&self) -> bool {
        false
    }

    fn create_session(&self) -> Arc<dyn LicenseSession> {
        panic!("a provider that does not preload creates no sessions")
    }
}
