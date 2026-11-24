#![cfg(target_arch = "wasm32")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use channel::runtime::sleep;
use player::{
    Authenticator, AuthenticatorError, CertificateRequest, DrmError, DrmProvider, HttpRequest,
    HttpResponse, HttpTransport, LicenseSession, MediaError, MediaEvent, MediaItem, MediaPlayer,
    MediaPlayerEvent, MediaPlayerEvents, MediaPlayerFactory, MediaProduct, PlaybackPosition,
    Player, PlayerConfig, PlayerDependencies, PlayerEvent, PlayerState, ReportingEvent,
    TidalEventSender, TransportError,
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod support;

use support::{contracts, player_events};

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::select! {
        result = future => result,
        () = sleep(Duration::from_secs(3)) => panic!("browser player timed out"),
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Retry,
    PendingHttp,
    PendingDrm,
}

struct Platform {
    mode: Mode,
    requests: AtomicUsize,
    started: Arc<tokio::sync::Notify>,
    http_release: tokio::sync::Notify,
    preparation_stopped: Arc<tokio::sync::Notify>,
    media_stopped: Arc<tokio::sync::Notify>,
}

struct NotifyOnDrop(Arc<tokio::sync::Notify>);
impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}
#[async_trait::async_trait]

impl HttpTransport for Platform {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        if request.url == "https://certificate.example" {
            return Ok(HttpResponse {
                status_code: 200,
                body: vec![],
            });
        }
        assert_eq!(
            request.headers.get("Authorization").unwrap(),
            "Bearer fake-token"
        );
        self.requests.fetch_add(1, Ordering::SeqCst);
        if matches!(self.mode, Mode::PendingHttp) {
            let _guard = NotifyOnDrop(self.preparation_stopped.clone());
            self.started.notify_one();
            self.http_release.notified().await;
        }
        if matches!(self.mode, Mode::Retry) && self.requests.load(Ordering::SeqCst) == 1 {
            return Ok(HttpResponse {
                status_code: 503,
                body: vec![],
            });
        }
        let mut attributes =
            serde_json::json!({"uri": "https://media.example/track", "formats": ["FLAC"]});
        if matches!(self.mode, Mode::PendingDrm) {
            attributes["drmData"] = serde_json::json!({
                "drmSystem": "WIDEVINE", "licenseUrl": "https://license.example", "certificateUrl": "https://certificate.example", "initData": ["init"]
            });
        }
        Ok(HttpResponse {
            status_code: 200,
            body: serde_json::to_vec(
                &serde_json::json!({"data": {"id": "resolved-product", "attributes": attributes}}),
            )
            .unwrap(),
        })
    }
}
#[async_trait::async_trait]

impl Authenticator for Platform {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("fake-token".into())
    }
    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("unexpected refresh")
    }
}
impl DrmProvider for Platform {
    fn preloads_licenses(&self) -> bool {
        true
    }

    fn create_session(&self) -> Arc<dyn LicenseSession> {
        assert!(matches!(self.mode, Mode::PendingDrm));
        Arc::new(PendingSession {
            started: self.started.clone(),
            stopped: self.preparation_stopped.clone(),
        })
    }
}

/// Never creates its challenge; notifies when the waiting future is dropped.
struct PendingSession {
    started: Arc<tokio::sync::Notify>,
    stopped: Arc<tokio::sync::Notify>,
}
#[async_trait::async_trait]

impl LicenseSession for PendingSession {
    fn certificate_request(&self) -> CertificateRequest {
        CertificateRequest::Get
    }

    async fn create_challenge(&self, _: Vec<u8>, _: Vec<u8>) -> Result<Vec<u8>, DrmError> {
        let _guard = NotifyOnDrop(self.stopped.clone());
        self.started.notify_one();
        std::future::pending().await
    }

    async fn install_license(&self, _: Vec<u8>) -> Result<Vec<u8>, DrmError> {
        unreachable!("the challenge is never created")
    }
}

impl TidalEventSender for Platform {
    fn send_event(&self, _: ReportingEvent) {}
}

struct MediaFactory(Arc<tokio::sync::Notify>);
impl MediaPlayerFactory for MediaFactory {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer> {
        Arc::new(Media {
            events,
            stopped: self.0.clone(),
        })
    }
}

struct Media {
    events: Arc<dyn MediaPlayerEvents>,
    stopped: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl MediaPlayer for Media {
    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        assert_eq!(item.media_input().url, "https://media.example/track");
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Current {
                item: Some(item.clone()),
            }));
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Seeking(
                item.clone(),
                Duration::ZERO,
            )));
        let position = PlaybackPosition {
            position: Duration::from_secs(1),
            remaining: Duration::from_secs(99),
        };
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Seeked(
                item.clone(),
                position,
            )));
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Started(
                item.clone(),
                position.position,
            )));
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Position(item, position)));
        Ok(())
    }
    async fn set_current(&self, _: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        panic!("unexpected set_current")
    }
    async fn set_next(&self, _: Option<Arc<dyn MediaItem>>) -> Result<(), MediaError> {
        panic!("unexpected set_next")
    }
    async fn pause(&self) {}
    async fn resume(&self) {
        panic!("unexpected resume")
    }
    async fn seek(&self, _: player::MediaTime) -> Result<(), MediaError> {
        panic!("unexpected seek")
    }
    async fn set_volume(&self, _: f32) {
        panic!("unexpected volume")
    }
    async fn reset(&self) {}
    fn dispose(&self) {
        self.stopped.notify_one();
    }
}

fn fixture(
    mode: Mode,
) -> (
    Player,
    Arc<Platform>,
    tokio::sync::mpsc::UnboundedReceiver<player::PlayerEvent>,
) {
    let (player_events, events) = player_events::record();
    let platform = Arc::new(Platform {
        mode,
        requests: AtomicUsize::new(0),
        started: Arc::new(tokio::sync::Notify::new()),
        http_release: tokio::sync::Notify::new(),
        preparation_stopped: Arc::new(tokio::sync::Notify::new()),
        media_stopped: Arc::new(tokio::sync::Notify::new()),
    });
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".into(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events,
            media_factory: Arc::new(MediaFactory(platform.media_stopped.clone())),
            http_transport: platform.clone(),
            authenticator: platform.clone(),
            drm_provider: platform.clone(),
            event_sender: platform.clone(),
        },
    );
    (player, platform, events)
}

#[wasm_bindgen_test]
async fn player_loads_retries_plays_and_delivers_events_without_tokio_runtime() {
    let (player, platform, mut events) = fixture(Mode::Retry);
    let started = web_time::Instant::now();
    let seed = player.prepare(MediaProduct::Track { id: "1".into() });
    contracts::assert_player_contracts(&player, &seed);
    bounded(player.play(seed.clone())).await.unwrap();
    assert_eq!(platform.requests.load(Ordering::SeqCst), 2);
    assert!(started.elapsed() >= Duration::from_millis(390));
    bounded(async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                PlayerEvent::StateChanged {
                    state: PlayerState::Playing,
                    ..
                } | PlayerEvent::CurrentItemChanged {
                    state: PlayerState::Playing,
                    ..
                }
            ) {
                return;
            }
        }
        panic!("missing playing event");
    })
    .await;
    bounded(player.pause()).await;
    bounded(player.reset()).await;
    drop(seed);
    drop(player);
    bounded(platform.media_stopped.notified()).await;
    bounded(async { while events.recv().await.is_some() {} }).await;
}

#[wasm_bindgen_test]
async fn dropping_the_last_seed_cancels_http_and_drm_on_the_browser_executor() {
    for mode in [Mode::PendingHttp, Mode::PendingDrm] {
        let (player, platform, _events) = fixture(mode);
        let seed = player.prepare(MediaProduct::Track { id: "1".into() });
        bounded(platform.started.notified()).await;
        drop(seed);
        bounded(platform.preparation_stopped.notified()).await;
        // The cancelled preparation must not block later commands.
        bounded(player.pause()).await;
        drop(player);
        bounded(platform.media_stopped.notified()).await;
    }
}

fn submitted<F: Future>(future: F) -> Pin<Box<F>> {
    let mut future = Box::pin(future);
    assert!(future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
        .is_pending());
    future
}

#[wasm_bindgen_test]
async fn reset_cancels_seed_wait_and_queued_commands_but_keeps_a_retained_seed() {
    let (player, platform, mut events) = fixture(Mode::PendingHttp);
    let seed = player.prepare(MediaProduct::Track { id: "1".into() });
    bounded(platform.started.notified()).await;
    let load = submitted(player.play(seed.clone()));
    assert!(bounded(events.recv()).await.is_some());
    let pause = submitted(player.pause());
    bounded(player.reset()).await;
    bounded(load).await.unwrap();
    bounded(pause).await;

    platform.http_release.notify_one();
    bounded(player.play(seed.clone())).await.unwrap();
    assert_eq!(platform.requests.load(Ordering::SeqCst), 1);
}
