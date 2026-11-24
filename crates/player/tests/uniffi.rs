#![cfg(all(feature = "uniffi", not(target_arch = "wasm32")))]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use player::{
    Authenticator, AuthenticatorError, CrossfadeConfig, HttpRequest, HttpResponse, HttpTransport,
    MediaError, MediaEvent, MediaItem, MediaPlayer, MediaPlayerEvent, MediaPlayerEvents,
    MediaPlayerFactory, MediaProduct, MediaTime, Player, PlayerConfig, PlayerEvent, PlayerEvents,
    ReportingEvent, TidalEventSender, TransportError,
};
use tokio::sync::{oneshot, Semaphore};

struct Host {
    items: Arc<Mutex<Vec<Arc<dyn MediaItem>>>>,
    released: Arc<Semaphore>,
    media: Mutex<Vec<Arc<Media>>>,
    pending: Arc<Mutex<Option<Pending>>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl HttpTransport for Host {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        assert!(request
            .headers
            .iter()
            .any(|header| header.1 == "Bearer token"));
        Ok(HttpResponse {
            status_code: 200,
            body: br#"{"data":{"id":"42","attributes":{"uri":"https://media.example/track","formats":["FLAC"],"drmData":{"drmSystem":"FAIRPLAY","licenseUrl":"https://license.example","certificateUrl":"https://certificate.example","initData":["init"]}}}}"#.to_vec(),
        })
    }
}

#[async_trait::async_trait]
impl Authenticator for Host {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("token".into())
    }
    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("the successful response does not need a refreshed token")
    }
}

impl TidalEventSender for Host {
    fn send_event(&self, _: ReportingEvent) {}
}
impl PlayerEvents for Host {
    fn on_event(&self, _: PlayerEvent) {}
}

impl Default for Host {
    fn default() -> Self {
        Self {
            items: Arc::new(Mutex::new(Vec::new())),
            released: Arc::new(Semaphore::new(0)),
            media: Mutex::new(Vec::new()),
            pending: Arc::new(Mutex::new(None)),
            order: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

struct Media {
    items: Arc<Mutex<Vec<Arc<dyn MediaItem>>>>,
    released: Arc<Semaphore>,
    dispose_calls: AtomicUsize,
    pending: Arc<Mutex<Option<Pending>>>,
    order: Arc<Mutex<Vec<&'static str>>>,
    events: Arc<dyn MediaPlayerEvents>,
}

struct Factory(Arc<Host>);

impl MediaPlayerFactory for Factory {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer> {
        let media = Arc::new(Media {
            items: self.0.items.clone(),
            released: self.0.released.clone(),
            dispose_calls: AtomicUsize::new(0),
            pending: self.0.pending.clone(),
            order: self.0.order.clone(),
            events,
        });
        self.0.media.lock().unwrap().push(media.clone());
        media
    }
}

#[async_trait::async_trait]
impl MediaPlayer for Media {
    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.set_current(item).await
    }
    async fn set_current(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        let pending = self.pending.lock().unwrap().take();
        if let Some(pending) = pending {
            let _drop = PendingDrop(self.order.clone());
            let _ = pending.entered.send(());
            let _ = pending.proceed.await;
        }
        self.items.lock().unwrap().push(item.clone());
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Prepared(item.clone())));
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Current {
                item: Some(item),
            }));
        Ok(())
    }
    async fn set_next(&self, _: Option<Arc<dyn MediaItem>>) -> Result<(), MediaError> {
        Ok(())
    }
    async fn pause(&self) {}
    async fn resume(&self) {}
    async fn seek(&self, _: MediaTime) -> Result<(), MediaError> {
        Ok(())
    }
    async fn set_volume(&self, _: f32) {}
    async fn reset(&self) {}
    fn dispose(&self) {
        if self.dispose_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.order.lock().unwrap().push("disposed");
            self.released.add_permits(1);
        }
    }
}

fn construct(host: &Arc<Host>, crossfade: Option<CrossfadeConfig>) -> Player {
    // Called from an ordinary thread: the binding constructor must supply an executor.
    Player::from_platforms(
        PlayerConfig {
            base_url: "https://api.example".into(),
            manifest_configuration: Default::default(),
            crossfade,
        },
        host.clone(),
        host.clone(),
        Arc::new(Factory(host.clone())),
        Arc::new(support::drm_provider::NoopDrmProvider),
        host.clone(),
        host.clone(),
    )
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("binding operation timed out")
}

#[test]
fn construction_supplies_a_runtime_and_selects_the_controller() {
    for (crossfade, instances) in [
        (None, 1),
        (
            Some(CrossfadeConfig {
                overlap: Duration::from_secs(3),
            }),
            2,
        ),
    ] {
        let host = Arc::new(Host::default());
        let player = construct(&host, crossfade);
        assert_eq!(host.media.lock().unwrap().len(), instances);
        drop(player);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            bounded(host.released.acquire_many(instances as u32))
                .await
                .unwrap()
                .forget();
        });
    }
}

struct Pending {
    entered: oneshot::Sender<()>,
    proceed: oneshot::Receiver<()>,
}
struct PendingDrop(Arc<Mutex<Vec<&'static str>>>);
impl Drop for PendingDrop {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("pending command dropped");
    }
}

#[tokio::test]
async fn in_flight_owner_retains_media_and_cancellation_drops_future_before_disposal() {
    let host = Arc::new(Host::default());
    let (entered, entering) = oneshot::channel();
    let (proceed, proceeding) = oneshot::channel();
    *host.pending.lock().unwrap() = Some(Pending {
        entered,
        proceed: proceeding,
    });
    let player = Arc::new(construct(&host, None));
    let seed = player.prepare(MediaProduct::Track {
        id: "pending".into(),
    });
    let retained = player.clone();
    let load = tokio::spawn(async move { retained.play(seed).await });
    bounded(entering).await.unwrap();
    drop(player);
    assert_eq!(
        host.media.lock().unwrap()[0]
            .dispose_calls
            .load(Ordering::SeqCst),
        0
    );
    load.abort();
    assert!(bounded(load).await.unwrap_err().is_cancelled());
    bounded(host.released.acquire()).await.unwrap().forget();
    assert_eq!(
        *host.order.lock().unwrap(),
        ["pending command dropped", "disposed"]
    );
    assert_eq!(
        host.media.lock().unwrap()[0]
            .dispose_calls
            .load(Ordering::SeqCst),
        1
    );
    assert!(
        proceed.send(()).is_err(),
        "late platform completion no longer has a receiver"
    );
    assert!(host.items.lock().unwrap().is_empty());
}
