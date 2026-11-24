mod support;

use player::{
    Authenticator, AuthenticatorError, CertificateRequest, CrossfadeConfig, DrmError, DrmProvider,
    HttpRequest, HttpResponse, HttpTransport, LicenseSession, MediaError, MediaEvent, MediaItem,
    MediaPlayer, MediaPlayerEvent, MediaPlayerEvents, MediaPlayerFactory, MediaProduct, MediaTime,
    PlaybackPosition, PlaybackSeed, Player, PlayerConfig, PlayerDependencies, PlayerEvent,
    PlayerEvents, PlayerState, ReportingEvent, TidalEventSender, TransportError,
};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use web_time::SystemTime;

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("timed out")
}

fn timestamp(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn at(millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(millis)
}

fn position(seconds: u64) -> PlaybackPosition {
    PlaybackPosition {
        position: Duration::from_secs(seconds),
        remaining: Duration::from_secs(100 - seconds),
    }
}

fn manifest(actual_id: &str, encrypted: bool, video: bool) -> HttpResponse {
    let mut attributes = json!({
        "uri": "https://media.example/track",
        "link": {"href": "https://media.example/video"},
        "formats": ["FLAC", "FUTURE_FORMAT"],
        "trackPresentation": "PREVIEW",
        "videoPresentation": "FULL",
    });
    if encrypted {
        attributes["drmData"] = json!({
            "drmSystem": "WIDEVINE",
            "licenseUrl": "https://license.example",
            "certificateUrl": "https://certificate.example",
            "initData": ["init"],
        });
    }
    if video {
        attributes
            .as_object_mut()
            .unwrap()
            .remove("trackPresentation");
    }
    HttpResponse {
        status_code: 200,
        body: serde_json::to_vec(&json!({"data": {
            "id": actual_id, "attributes": attributes,
        }}))
        .unwrap(),
    }
}

struct HttpCall(oneshot::Sender<HttpResponse>);
/// A license installation waiting for the test's result.
struct LicenseCall {
    respond: oneshot::Sender<Result<Vec<u8>, DrmError>>,
}

struct Platform {
    http: mpsc::UnboundedSender<HttpCall>,
    licenses: mpsc::UnboundedSender<LicenseCall>,
    reports: mpsc::UnboundedSender<ReportingEvent>,
    preloads: bool,
    license_status: AtomicU16,
}

#[async_trait::async_trait]
impl HttpTransport for Platform {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        // DRM requests are answered directly; the session or `license_status` decides the
        // outcome.
        let status_code = match request.url.as_str() {
            "https://certificate.example" => Some(200),
            "https://license.example" => Some(self.license_status.load(Ordering::Relaxed)),
            _ => None,
        };
        if let Some(status_code) = status_code {
            return Ok(HttpResponse {
                status_code,
                body: Vec::new(),
            });
        }
        let (respond, response) = oneshot::channel();
        self.http.send(HttpCall(respond)).unwrap();
        Ok(response.await.expect("missing HTTP response"))
    }
}

#[async_trait::async_trait]
impl Authenticator for Platform {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("token".into())
    }

    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("unexpected token refresh")
    }
}

impl DrmProvider for Platform {
    fn preloads_licenses(&self) -> bool {
        self.preloads
    }

    fn create_session(&self) -> Arc<dyn LicenseSession> {
        Arc::new(Session(self.licenses.clone()))
    }
}

/// Installs each license with the result the test sends through its `LicenseCall`.
struct Session(mpsc::UnboundedSender<LicenseCall>);

#[async_trait::async_trait]
impl LicenseSession for Session {
    fn certificate_request(&self) -> CertificateRequest {
        CertificateRequest::Get
    }

    async fn create_challenge(&self, _: Vec<u8>, _: Option<Vec<u8>>) -> Result<Vec<u8>, DrmError> {
        Ok(Vec::new())
    }

    async fn install_license(&self, _: Vec<u8>) -> Result<Vec<u8>, DrmError> {
        let (respond, response) = oneshot::channel();
        assert!(self.0.send(LicenseCall { respond }).is_ok());
        response.await.expect("missing license response")
    }
}

impl TidalEventSender for Platform {
    fn send_event(&self, event: ReportingEvent) {
        self.reports.send(event).unwrap();
    }
}

struct Notifications(mpsc::UnboundedSender<PlayerEvent>);

impl PlayerEvents for Notifications {
    fn on_event(&self, event: PlayerEvent) {
        let _ = self.0.send(event);
    }
}

struct InstallationCall {
    item: Arc<dyn MediaItem>,
    respond: oneshot::Sender<Result<(), MediaError>>,
}

struct Slot {
    events: Arc<dyn MediaPlayerEvents>,
    current: Option<Arc<dyn MediaItem>>,
    next: Option<Arc<dyn MediaItem>>,
    installation_error: Option<MediaError>,
    installation: Option<oneshot::Sender<InstallationCall>>,
}

#[derive(Default)]
struct MediaControl(
    Mutex<Vec<Slot>>,
    Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
);

impl MediaControl {
    fn gate_installation(&self, slot: usize) -> oneshot::Receiver<InstallationCall> {
        let (send, receive) = oneshot::channel();
        self.0.lock().unwrap()[slot].installation = Some(send);
        receive
    }

    fn current(&self, slot: usize) -> Arc<dyn MediaItem> {
        self.0.lock().unwrap()[slot].current.clone().unwrap()
    }

    fn next(&self) -> Arc<dyn MediaItem> {
        self.0.lock().unwrap()[0].next.clone().unwrap()
    }

    fn emit(&self, slot: usize, event: MediaPlayerEvent, emitted_at: SystemTime) {
        let callback = self.0.lock().unwrap()[slot].events.clone();
        callback.on_event(MediaEvent { event, emitted_at });
    }

    fn advance(&self) {
        let incoming = {
            let mut slots = self.0.lock().unwrap();
            let incoming = slots[0].next.take().unwrap();
            slots[0].current = Some(incoming.clone());
            incoming
        };
        self.emit(
            0,
            MediaPlayerEvent::Current {
                item: Some(incoming),
            },
            SystemTime::now(),
        );
    }
}

struct MediaFactory {
    control: Arc<MediaControl>,
    resumes: mpsc::UnboundedSender<usize>,
}

impl MediaPlayerFactory for MediaFactory {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer> {
        let mut slots = self.control.0.lock().unwrap();
        let slot = slots.len();
        slots.push(Slot {
            events,
            current: None,
            next: None,
            installation_error: None,
            installation: None,
        });
        Arc::new(TestMedia {
            slot,
            control: self.control.clone(),
            resumes: self.resumes.clone(),
        })
    }
}

struct TestMedia {
    slot: usize,
    control: Arc<MediaControl>,
    resumes: mpsc::UnboundedSender<usize>,
}

impl TestMedia {
    async fn install(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        let gate = self.control.0.lock().unwrap()[self.slot]
            .installation
            .take();
        if let Some(gate) = gate {
            let (respond, response) = oneshot::channel();
            assert!(gate.send(InstallationCall { item, respond }).is_ok());
            response.await.expect("missing installation response")?;
        }
        Ok(())
    }

    fn prepared(&self, item: Arc<dyn MediaItem>) {
        self.control.emit(
            self.slot,
            MediaPlayerEvent::Prepared(item.clone()),
            SystemTime::now(),
        );
        self.control.emit(
            self.slot,
            MediaPlayerEvent::Duration(item, Duration::from_secs(100)),
            SystemTime::now(),
        );
    }
}

#[async_trait::async_trait]
impl MediaPlayer for TestMedia {
    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.set_current(item).await
    }

    async fn set_current(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.install(item.clone()).await?;
        {
            let mut slots = self.control.0.lock().unwrap();
            if let Some(error) = slots[self.slot].installation_error.take() {
                return Err(error);
            }
            slots[self.slot].current = Some(item.clone());
            slots[self.slot].next = None;
        }
        self.control.emit(
            self.slot,
            MediaPlayerEvent::Current {
                item: Some(item.clone()),
            },
            SystemTime::now(),
        );
        self.prepared(item);
        Ok(())
    }

    async fn set_next(&self, item: Option<Arc<dyn MediaItem>>) -> Result<(), MediaError> {
        {
            let mut slots = self.control.0.lock().unwrap();
            if let Some(error) = slots[self.slot].installation_error.take() {
                return Err(error);
            }
        }
        if let Some(item) = &item {
            self.install(item.clone()).await?;
        }
        self.control.0.lock().unwrap()[self.slot].next = item.clone();
        if let Some(item) = item {
            self.prepared(item);
        }
        Ok(())
    }

    async fn pause(&self) {}
    async fn resume(&self) {
        self.resumes.send(self.slot).unwrap();
    }
    async fn seek(&self, _: MediaTime) -> Result<(), MediaError> {
        Ok(())
    }
    async fn set_volume(&self, _: f32) {}
    async fn reset(&self) {
        self.dispose();
        let gate = self.control.1.lock().unwrap().take();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.await;
        }
    }
    fn dispose(&self) {
        let mut slots = self.control.0.lock().unwrap();
        slots[self.slot].current = None;
        slots[self.slot].next = None;
    }
}

#[derive(Clone, Copy)]
enum PlaybackMode {
    Queued,
    Crossfade(u64),
}

struct Fixture {
    player: Arc<Player>,
    platform: Arc<Platform>,
    media: Arc<MediaControl>,
    http: mpsc::UnboundedReceiver<HttpCall>,
    licenses: mpsc::UnboundedReceiver<LicenseCall>,
    reports: mpsc::UnboundedReceiver<ReportingEvent>,
    resumes: mpsc::UnboundedReceiver<usize>,
    notifications: mpsc::UnboundedReceiver<PlayerEvent>,
}

impl Fixture {
    fn new(mode: PlaybackMode) -> Self {
        Self::with_preloading(mode, true)
    }

    fn with_preloading(mode: PlaybackMode, preloads: bool) -> Self {
        let (http_send, http) = mpsc::unbounded_channel();
        let (license_send, licenses) = mpsc::unbounded_channel();
        let (report_send, reports) = mpsc::unbounded_channel();
        let platform = Arc::new(Platform {
            http: http_send,
            licenses: license_send,
            reports: report_send,
            preloads,
            license_status: AtomicU16::new(200),
        });
        let media = Arc::new(MediaControl::default());
        let (resume_send, resumes) = mpsc::unbounded_channel();
        let (notify, notifications) = mpsc::unbounded_channel();
        let factory = MediaFactory {
            control: media.clone(),
            resumes: resume_send,
        };
        let crossfade = match mode {
            PlaybackMode::Queued => None,
            PlaybackMode::Crossfade(seconds) => Some(CrossfadeConfig {
                overlap: Duration::from_secs(seconds),
            }),
        };
        let player = Arc::new(Player::new(
            PlayerConfig {
                base_url: "https://api.example/v2".into(),
                manifest_configuration: Default::default(),
                crossfade,
            },
            PlayerDependencies {
                player_events: Arc::new(Notifications(notify)),
                event_sender: platform.clone(),
                http_transport: platform.clone(),
                authenticator: platform.clone(),
                drm_provider: platform.clone(),
                media_factory: Arc::new(factory),
            },
        ));
        Self {
            player,
            platform,
            media,
            http,
            licenses,
            reports,
            resumes,
            notifications,
        }
    }

    async fn selected(&mut self) {
        while !matches!(
            bounded(self.notifications.recv()).await,
            Some(PlayerEvent::CurrentItemChanged { item: Some(_), .. })
        ) {}
    }

    async fn prepare(
        &mut self,
        product: MediaProduct,
        actual_id: &str,
        encrypted: bool,
    ) -> Arc<PlaybackSeed> {
        let video = matches!(product, MediaProduct::Video { .. });
        let seed = self.player.prepare(product);
        let HttpCall(respond) = bounded(self.http.recv()).await.unwrap();
        respond.send(manifest(actual_id, encrypted, video)).unwrap();
        if encrypted {
            let call = bounded(self.licenses.recv()).await.unwrap();
            assert!(call.respond.send(Ok(b"handle".to_vec())).is_ok());
        }
        seed
    }

    fn current(&self) -> Arc<dyn MediaItem> {
        self.media.current(0)
    }

    async fn collect(&mut self, count: usize) -> Vec<(String, Value)> {
        let mut events = Vec::new();
        for _ in 0..count {
            let report = bounded(self.reports.recv()).await.unwrap();
            let group = match report.name.as_str() {
                "playback_session" => "play_log",
                _ => "streaming_metrics",
            };
            assert_eq!(
                (report.group.as_str(), report.version),
                (group, 2),
                "{}",
                report.name
            );
            events.push((report.name, serde_json::from_str(&report.payload).unwrap()));
        }
        assert!(self.reports.try_recv().is_err(), "unexpected extra report");
        events
    }

    async fn position_changed(&mut self, seconds: u64) {
        bounded(async {
            while let Some(event) = self.notifications.recv().await {
                if matches!(event, PlayerEvent::PositionChanged { position, .. } if position == Duration::from_secs(seconds)) {
                    return;
                }
            }
            panic!("missing position event");
        }).await;
    }
}

fn payload<'a>(reports: &'a [(String, Value)], name: &str) -> &'a Value {
    let matches: Vec<_> = reports.iter().filter(|(key, _)| key == name).collect();
    assert_eq!(matches.len(), 1, "expected exactly one {name}");
    &matches[0].1
}

#[tokio::test]
async fn completed_playback_reports_observed_timing_metadata_stalls_and_actions() {
    for video in [false, true] {
        let mut f = Fixture::new(PlaybackMode::Queued);
        let product = if video {
            MediaProduct::Video {
                id: "requested".into(),
            }
        } else {
            MediaProduct::Track {
                id: "requested".into(),
            }
        };
        let seed = f.prepare(product, "actual", true).await;
        let before_play = timestamp(SystemTime::now());
        bounded(f.player.play(seed.clone())).await.unwrap();
        let after_play = timestamp(SystemTime::now());
        let item = f.current();
        for (millis, event) in [
            (
                100_050,
                MediaPlayerEvent::Stalled(item.clone(), position(0)),
            ),
            (
                100_100,
                MediaPlayerEvent::Started(item.clone(), Duration::from_secs(7)),
            ),
            (
                100_200,
                MediaPlayerEvent::Stalled(item.clone(), position(7)),
            ),
            (100_250, MediaPlayerEvent::Paused(item.clone(), position(7))),
            (
                100_300,
                MediaPlayerEvent::Seeking(item.clone(), Duration::from_secs(8)),
            ),
            (
                100_350,
                MediaPlayerEvent::Seeked(item.clone(), position(20)),
            ),
            (
                100_400,
                MediaPlayerEvent::Resumed(item.clone(), position(20)),
            ),
            (
                100_500,
                MediaPlayerEvent::Seeking(item.clone(), Duration::from_secs(22)),
            ),
            (
                100_550,
                MediaPlayerEvent::Stalled(item.clone(), position(22)),
            ),
            (
                100_575,
                MediaPlayerEvent::Seeked(item.clone(), position(40)),
            ),
            (
                100_600,
                MediaPlayerEvent::Position(item.clone(), position(40)),
            ),
            (
                100_610,
                MediaPlayerEvent::Position(item.clone(), position(40)),
            ),
            (
                100_620,
                MediaPlayerEvent::Position(item.clone(), position(41)),
            ),
            (
                100_650,
                MediaPlayerEvent::Stalled(item.clone(), position(41)),
            ),
            (
                100_700,
                MediaPlayerEvent::Position(item.clone(), position(42)),
            ),
            (
                100_800,
                MediaPlayerEvent::Resumed(item.clone(), position(42)),
            ),
            (
                100_900,
                MediaPlayerEvent::Paused(item.clone(), position(41)),
            ),
            (
                101_000,
                MediaPlayerEvent::Ended(item.clone(), Duration::from_secs(97)),
            ),
        ] {
            f.media.emit(0, event, at(millis));
        }
        bounded(f.player.reset()).await;
        let retained = item.clone();
        drop(item);
        drop(seed);
        assert!(f.reports.try_recv().is_err());
        drop(retained);
        let reports = f.collect(6).await;
        let start = payload(&reports, "streaming_session_start");
        let log = payload(&reports, "playback_session");
        let stats = payload(&reports, "playback_statistics");
        let manifest = payload(&reports, "playback_info_fetch");
        let license = payload(&reports, "drm_license_fetch");
        let end = payload(&reports, "streaming_session_end");
        let id = start["streamingSessionId"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok());
        assert_eq!(log["playbackSessionId"], id);
        for report in [stats, manifest, license, end] {
            assert_eq!(report["streamingSessionId"], id);
        }
        assert_eq!(start["sessionProductId"], "requested");
        assert_eq!(log["requestedProductId"], "requested");
        for report in [log, stats] {
            assert_eq!(report["actualProductId"], "actual");
            assert_eq!(report["productType"], if video { "VIDEO" } else { "TRACK" });
            assert_eq!(
                report["actualAssetPresentation"],
                if video { "FULL" } else { "PREVIEW" }
            );
            assert_eq!(
                report["actualQuality"],
                if video { "VQ N/A" } else { "LOSSLESS" }
            );
            assert_eq!(report["endTimestamp"], 101_000);
        }
        assert_eq!(log["startTimestamp"], 100_100);
        assert_eq!(log["startAssetPosition"], 7.0);
        assert_eq!(log["endAssetPosition"], 97.0);
        assert_eq!(stats["actualStartTimestamp"], 100_100);
        assert!(
            (before_play..=after_play).contains(&stats["idealStartTimestamp"].as_u64().unwrap())
        );
        assert_eq!(stats["startReason"], "EXPLICIT");
        assert_eq!(stats["endReason"], "COMPLETE");
        for fetch in [manifest, license] {
            assert_eq!(fetch["endReason"], "COMPLETE");
        }
        assert!(
            manifest["startTimestamp"].as_u64().unwrap()
                <= manifest["endTimestamp"].as_u64().unwrap()
        );
        assert!(
            manifest["endTimestamp"].as_u64().unwrap()
                <= license["startTimestamp"].as_u64().unwrap()
        );
        assert_eq!(
            stats["stalls"],
            json!([
                {"reason":"UNEXPECTED", "assetPosition":0.0, "startTimestamp":100_050, "endTimestamp":100_100},
                {"reason":"UNEXPECTED", "assetPosition":7.0, "startTimestamp":100_200, "endTimestamp":100_400},
                {"reason":"SEEK", "assetPosition":22.0, "startTimestamp":100_550, "endTimestamp":100_600},
                {"reason":"UNEXPECTED", "assetPosition":41.0, "startTimestamp":100_650, "endTimestamp":100_700},
            ])
        );
        assert_eq!(
            log["actions"],
            json!([
                {"actionType":"PLAYBACK_STOP", "assetPosition":0.0, "timestamp":100_050},
                {"actionType":"PLAYBACK_START", "assetPosition":7.0, "timestamp":100_100},
                {"actionType":"PLAYBACK_STOP", "assetPosition":7.0, "timestamp":100_200},
                {"actionType":"PLAYBACK_START", "assetPosition":20.0, "timestamp":100_400},
                {"actionType":"PLAYBACK_STOP", "assetPosition":22.0, "timestamp":100_500},
                {"actionType":"PLAYBACK_START", "assetPosition":40.0, "timestamp":100_600},
                {"actionType":"PLAYBACK_STOP", "assetPosition":41.0, "timestamp":100_650},
                {"actionType":"PLAYBACK_START", "assetPosition":42.0, "timestamp":100_700},
                {"actionType":"PLAYBACK_STOP", "assetPosition":41.0, "timestamp":100_900},
            ])
        );
    }
}

#[tokio::test]
async fn repeated_stall_reports_count_as_one_stall() {
    let mut f = Fixture::new(PlaybackMode::Queued);
    let seed = f
        .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
        .await;
    bounded(f.player.play(seed.clone())).await.unwrap();
    let item = f.current();
    for (millis, event) in [
        (
            100_100,
            MediaPlayerEvent::Started(item.clone(), Duration::ZERO),
        ),
        (
            100_200,
            MediaPlayerEvent::Seeking(item.clone(), Duration::from_secs(20)),
        ),
        (
            100_210,
            MediaPlayerEvent::Stalled(item.clone(), position(20)),
        ),
        (
            100_220,
            MediaPlayerEvent::Stalled(item.clone(), position(21)),
        ),
        (
            100_300,
            MediaPlayerEvent::Resumed(item.clone(), position(20)),
        ),
        (
            100_400,
            MediaPlayerEvent::Stalled(item.clone(), position(30)),
        ),
        (
            100_410,
            MediaPlayerEvent::Stalled(item.clone(), position(31)),
        ),
        (
            100_500,
            MediaPlayerEvent::Position(item.clone(), position(30)),
        ),
        (
            100_600,
            MediaPlayerEvent::Stalled(item.clone(), position(40)),
        ),
        (
            100_700,
            MediaPlayerEvent::Resumed(item.clone(), position(40)),
        ),
        (
            100_800,
            MediaPlayerEvent::Ended(item.clone(), Duration::from_secs(50)),
        ),
    ] {
        f.media.emit(0, event, at(millis));
    }
    bounded(f.player.reset()).await;
    drop(item);
    drop(seed);
    let reports = f.collect(5).await;
    assert_eq!(
        payload(&reports, "playback_statistics")["stalls"],
        json!([
            {"reason":"SEEK", "assetPosition":20.0, "startTimestamp":100_210, "endTimestamp":100_300},
            {"reason":"UNEXPECTED", "assetPosition":30.0, "startTimestamp":100_400, "endTimestamp":100_500},
            {"reason":"UNEXPECTED", "assetPosition":40.0, "startTimestamp":100_600, "endTimestamp":100_700},
        ])
    );
}

#[tokio::test]
async fn play_logs_require_started() {
    for started in [false, true] {
        // Without preloading, the media player acquires licenses.
        let mut f = Fixture::with_preloading(PlaybackMode::Queued, false);
        let seed = f.player.prepare(MediaProduct::Track { id: "track".into() });
        let HttpCall(respond) = bounded(f.http.recv()).await.unwrap();
        respond.send(manifest("actual", true, false)).unwrap();
        bounded(f.player.play(seed.clone())).await.unwrap();
        let item = f.current();
        if started {
            f.media.emit(
                0,
                MediaPlayerEvent::Started(item.clone(), Duration::ZERO),
                at(100_100),
            );
        }
        f.media.emit(
            0,
            MediaPlayerEvent::Position(item.clone(), position(1)),
            at(100_200),
        );
        f.position_changed(1).await;
        bounded(f.player.reset()).await;
        drop(item);
        let reports = f.collect(4 + usize::from(started)).await;
        assert_eq!(
            reports.iter().any(|(name, _)| name == "playback_session"),
            started
        );
        assert!(!reports.iter().any(|(name, _)| name == "drm_license_fetch"));
        assert_eq!(
            payload(&reports, "playback_statistics")["actualStartTimestamp"],
            if started { json!(100_100) } else { Value::Null }
        );
        if started {
            assert_eq!(payload(&reports, "playback_session")["actions"], json!([]));
        }
        assert_eq!(
            payload(&reports, "playback_info_fetch")["endReason"],
            "COMPLETE"
        );
    }
}

#[tokio::test]
async fn fetches_the_media_player_makes_are_reported_with_its_item() {
    for license_status in [200, 403] {
        let mut f = Fixture::with_preloading(PlaybackMode::Queued, false);
        f.platform
            .license_status
            .store(license_status, Ordering::Relaxed);
        let seed = f.player.prepare(MediaProduct::Track { id: "track".into() });
        let HttpCall(respond) = bounded(f.http.recv()).await.unwrap();
        respond.send(manifest("actual", true, false)).unwrap();
        bounded(f.player.play(seed.clone())).await.unwrap();
        let item = f.current();
        let before = timestamp(SystemTime::now());
        bounded(item.clone().fetch_certificate(CertificateRequest::Get))
            .await
            .unwrap();
        let license = bounded(item.clone().fetch_license(Vec::new())).await;
        assert_eq!(license.is_ok(), license_status == 200);
        let after = timestamp(SystemTime::now());
        bounded(f.player.reset()).await;
        drop(item);
        drop(seed);

        let reports = f.collect(5).await;
        let id = payload(&reports, "streaming_session_start")["streamingSessionId"].clone();
        let license = payload(&reports, "drm_license_fetch");
        assert_eq!(license["streamingSessionId"], id);
        assert!(before <= license["startTimestamp"].as_u64().unwrap());
        assert!(license["endTimestamp"].as_u64().unwrap() <= after);
        if license_status == 200 {
            assert_eq!(license["endReason"], "COMPLETE");
        } else {
            assert_eq!(license["endReason"], "ERROR");
            assert_eq!(license["errorCode"], "dh:403");
        }
    }
}

#[tokio::test]
async fn resume_preserves_the_play_request() {
    for mode in [PlaybackMode::Queued, PlaybackMode::Crossfade(0)] {
        let mut f = Fixture::new(mode);
        let seed = f
            .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
            .await;
        let player = f.player.clone();
        let before = timestamp(SystemTime::now());
        bounded(player.play(seed)).await.unwrap();
        let after = timestamp(SystemTime::now());
        // Make a later resume's timestamp distinguishable from the play request.
        tokio::time::sleep(Duration::from_millis(3)).await;
        let item = f.current();
        bounded(player.pause()).await;
        bounded(player.resume()).await;
        assert_eq!(bounded(f.resumes.recv()).await, Some(0));
        bounded(player.reset()).await;
        drop(item);
        let reports = f.collect(4).await;
        let stats = payload(&reports, "playback_statistics");
        assert_eq!(stats["startReason"], "EXPLICIT");
        assert!((before..=after).contains(&stats["idealStartTimestamp"].as_u64().unwrap()));
    }
}

#[tokio::test]
async fn reset_barrier_clears_a_retained_selection_observed_during_cleanup() {
    let mut f = Fixture::new(PlaybackMode::Queued);
    let seed = f
        .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
        .await;
    // Media advancement without an Ended item selects the item without playback intent,
    // so a resume reaching the retained selection would add playback statistics.
    bounded(f.player.set_next(Some(seed))).await.unwrap();
    let item = f.media.next();
    f.media.emit(
        0,
        MediaPlayerEvent::Current {
            item: Some(item.clone()),
        },
        SystemTime::now(),
    );
    f.selected().await;
    while f.notifications.try_recv().is_ok() {}

    let (started, resetting) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    *f.media.1.lock().unwrap() = Some((started, gate));
    let player = f.player.clone();
    let reset = tokio::spawn(async move { player.reset().await });
    bounded(resetting).await.unwrap();
    // The old FIFO can still deliver Current before the reset acknowledgement.
    f.media.emit(
        0,
        MediaPlayerEvent::Current {
            item: Some(item.clone()),
        },
        SystemTime::now(),
    );
    f.selected().await;
    release.send(()).unwrap();
    bounded(reset).await.unwrap();
    bounded(f.player.resume()).await;
    bounded(f.player.reset()).await;
    drop(item);
    let reports = f.collect(3).await;
    assert!(!reports
        .iter()
        .any(|(name, _)| name == "playback_statistics"));
}

#[tokio::test]
async fn reset_preserves_buffered_reporting_facts_without_advancing_playback() {
    for (mode, slot, superseded) in [
        (PlaybackMode::Queued, 0, false),
        (PlaybackMode::Crossfade(5), 0, false),
        (PlaybackMode::Crossfade(5), 0, true),
        (PlaybackMode::Crossfade(5), 1, false),
    ] {
        let mut f = Fixture::new(mode);
        let seed = f
            .prepare(MediaProduct::Track { id: "first".into() }, "first", false)
            .await;
        bounded(f.player.play(seed)).await.unwrap();
        let first = f.current();
        f.selected().await;
        let seed = f
            .prepare(MediaProduct::Track { id: "next".into() }, "next", false)
            .await;
        bounded(f.player.set_next(Some(seed))).await.unwrap();
        let next = if matches!(mode, PlaybackMode::Queued) {
            f.media.next()
        } else {
            f.media.current(1)
        };
        if slot == 1 {
            f.media.emit(
                0,
                MediaPlayerEvent::Position(first.clone(), position(95)),
                at(100_000),
            );
            assert_eq!(bounded(f.resumes.recv()).await, Some(1));
        }

        let (started, resetting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *f.media.1.lock().unwrap() = Some((started, gate));
        let player = f.player.clone();
        let reset = tokio::spawn(async move {
            if slot == 0 {
                player.reset().await;
            } else {
                player.set_next(None).await.unwrap();
            }
        });
        bounded(resetting).await.unwrap();
        let item = if slot == 0 { &first } else { &next };
        for (millis, event) in [
            (
                100_100,
                MediaPlayerEvent::Started(item.clone(), Duration::from_secs(3)),
            ),
            (
                100_200,
                MediaPlayerEvent::Stalled(item.clone(), position(4)),
            ),
            (
                100_300,
                MediaPlayerEvent::Position(item.clone(), position(99)),
            ),
            (
                100_400,
                MediaPlayerEvent::Ended(item.clone(), Duration::from_secs(100)),
            ),
        ] {
            f.media.emit(slot, event, at(millis));
        }
        if superseded {
            bounded(f.player.reset()).await;
            assert!(release.is_closed());
        } else {
            release.send(()).unwrap();
        }
        bounded(reset).await.unwrap();
        if slot == 1 {
            bounded(f.player.reset()).await;
        }
        while !matches!(
            bounded(f.notifications.recv()).await,
            Some(PlayerEvent::CurrentItemChanged {
                item: None,
                state: PlayerState::Idle,
            })
        ) {}
        assert!(
            tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
                .await
                .is_err()
        );
        assert!(f.resumes.try_recv().is_err());
        assert!(f.reports.try_recv().is_err());

        let (item, other) = if slot == 0 {
            (first, next)
        } else {
            (next, first)
        };
        let weak = Arc::downgrade(&item);
        drop(item);
        let reports = f.collect(5).await;
        assert!(weak.upgrade().is_none());
        let log = payload(&reports, "playback_session");
        assert_eq!(
            log["actualProductId"],
            if slot == 0 { "first" } else { "next" }
        );
        assert_eq!(log["startTimestamp"], 100_100);
        assert_eq!(log["startAssetPosition"], 3.0);
        assert_eq!(log["endTimestamp"], 100_400);
        assert_eq!(log["endAssetPosition"], 100.0);
        let stats = payload(&reports, "playback_statistics");
        assert_eq!(stats["actualStartTimestamp"], 100_100);
        assert_eq!(stats["endTimestamp"], 100_400);
        assert_eq!(stats["endReason"], "COMPLETE");
        assert_eq!(
            stats["stalls"],
            json!([{
                "reason": "UNEXPECTED", "assetPosition": 4.0,
                "startTimestamp": 100_200, "endTimestamp": 100_300,
            }])
        );
        drop(other);
        let reports = f.collect(if slot == 0 { 3 } else { 4 }).await;
        if slot == 0 {
            assert!(!reports
                .iter()
                .any(|(name, _)| name == "playback_statistics"));
        }
    }
}

#[tokio::test]
async fn reusing_a_seed_reports_each_occurrence_with_its_own_identity() {
    let mut f = Fixture::new(PlaybackMode::Queued);
    let seed = f
        .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
        .await;
    bounded(f.player.play(seed.clone())).await.unwrap();
    let first = f.current();
    f.media.emit(
        0,
        MediaPlayerEvent::Started(first.clone(), Duration::from_secs(7)),
        at(100_100),
    );
    bounded(f.player.play(seed.clone())).await.unwrap();
    let second = f.current();
    f.media.emit(
        0,
        MediaPlayerEvent::Started(second.clone(), Duration::ZERO),
        at(100_200),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Position(second.clone(), position(40)),
        at(100_300),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Paused(second.clone(), position(40)),
        at(100_400),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Seeking(second.clone(), Duration::from_secs(40)),
        at(100_450),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Seeked(second.clone(), position(60)),
        at(100_500),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Seeking(second.clone(), Duration::from_secs(60)),
        at(100_575),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Seeked(second.clone(), position(70)),
        at(100_600),
    );
    f.position_changed(70).await;
    drop(seed);
    assert!(f.reports.try_recv().is_err());
    drop(first);
    let first_reports = f.collect(5).await;
    assert_eq!(
        payload(&first_reports, "playback_session")["endAssetPosition"],
        7.0
    );
    let before_drop = timestamp(SystemTime::now());
    bounded(f.player.reset()).await;
    drop(second);
    let second_reports = f.collect(5).await;
    let after_drop = timestamp(SystemTime::now());
    assert_ne!(
        payload(&first_reports, "playback_session")["playbackSessionId"],
        payload(&second_reports, "playback_session")["playbackSessionId"]
    );
    for name in ["streaming_session_start", "playback_info_fetch"] {
        let field = if name == "streaming_session_start" {
            "timestamp"
        } else {
            "startTimestamp"
        };
        assert_eq!(
            payload(&first_reports, name)[field],
            payload(&second_reports, name)[field]
        );
    }
    let log = payload(&second_reports, "playback_session");
    assert_eq!(log["endAssetPosition"], 70.0);
    assert_eq!(
        log["actions"],
        json!([
            {"actionType":"PLAYBACK_STOP", "assetPosition":40.0, "timestamp":100_400},
        ])
    );
    for name in ["playback_session", "playback_statistics"] {
        assert!((before_drop..=after_drop).contains(
            &payload(&second_reports, name)["endTimestamp"]
                .as_u64()
                .unwrap()
        ));
    }
    assert_eq!(
        payload(&second_reports, "playback_statistics")["endReason"],
        "OTHER"
    );
    assert!(f.http.try_recv().is_err());
}

#[tokio::test]
async fn failed_and_cancelled_preparation_report_only_the_retrievals_attempted() {
    enum Outcome {
        ManifestRejected,
        InvalidManifest,
        LicenseRejected,
        ManifestCancelled,
        LicenseCancelled,
    }
    use Outcome::*;

    for case in [
        ManifestRejected,
        InvalidManifest,
        LicenseRejected,
        ManifestCancelled,
        LicenseCancelled,
    ] {
        let mut f = Fixture::new(PlaybackMode::Queued);
        let before = timestamp(SystemTime::now());
        let seed = f.player.prepare(MediaProduct::Track { id: "track".into() });
        let HttpCall(respond) = bounded(f.http.recv()).await.unwrap();
        let license_stage = matches!(case, LicenseRejected | LicenseCancelled);
        let cancelled = matches!(case, ManifestCancelled | LicenseCancelled);
        if license_stage {
            respond.send(manifest("actual", true, false)).unwrap();
            let mut call = bounded(f.licenses.recv()).await.unwrap();
            if cancelled {
                drop(seed);
                bounded(call.respond.closed()).await;
            } else {
                assert!(call
                    .respond
                    .send(Err(DrmError::Failed {
                        description: "license rejected".into(),
                        code: Some("rejected".into()),
                    }))
                    .is_ok());
                bounded(f.player.play(seed.clone())).await.unwrap_err();
                // A failed seed is cached, including its reporting outcome.
                bounded(f.player.play(seed.clone())).await.unwrap_err();
            }
        } else if cancelled {
            drop(seed);
        } else {
            respond
                .send(if matches!(case, ManifestRejected) {
                    HttpResponse {
                        status_code: 404,
                        body: br#"{"errors":[{"code":"CONTENT_NOT_FOUND"}]}"#.to_vec(),
                    }
                } else {
                    HttpResponse {
                        status_code: 200,
                        body: br#"{"data":{"id":42,"attributes":{"uri":"https://media.example"}}}"#
                            .to_vec(),
                    }
                })
                .unwrap();
            bounded(f.player.play(seed.clone())).await.unwrap_err();
            bounded(f.player.play(seed.clone())).await.unwrap_err();
        }
        let license_fetched = license_stage;
        let reports = f.collect(if license_fetched { 4 } else { 3 }).await;
        let after = timestamp(SystemTime::now());
        assert!(!reports
            .iter()
            .any(|(name, _)| name == "playback_statistics" || name == "playback_session"));
        assert_eq!(
            reports.iter().any(|(name, _)| name == "drm_license_fetch"),
            license_fetched
        );
        let fetch = payload(
            &reports,
            if license_fetched {
                "drm_license_fetch"
            } else {
                "playback_info_fetch"
            },
        );
        assert_eq!(
            fetch["endReason"],
            match case {
                ManifestCancelled | LicenseCancelled => "OTHER",
                _ => "ERROR",
            }
        );
        assert_eq!(
            fetch["errorCode"],
            match case {
                ManifestRejected => json!("mcnf"),
                InvalidManifest => json!("mdec"),
                LicenseRejected => json!("do:rejected"),
                _ => Value::Null,
            }
        );
        assert!((before..=after).contains(&fetch["endTimestamp"].as_u64().unwrap()));
        if license_stage {
            assert_eq!(
                payload(&reports, "playback_info_fetch")["endReason"],
                "COMPLETE"
            );
        }
        assert!(f.http.try_recv().is_err());
    }
}

#[tokio::test]
async fn interrupted_playback_reports_observed_errors_and_loading_abandonment() {
    enum Interruption {
        BeforeStarting,
        LoadingTimeout,
        Stalled,
        StartedAfterStall,
        ResumedAfterStall,
        PositionAfterStall,
        PausedAfterStall,
        EndedAfterStall,
        Failed,
    }
    use Interruption::*;

    for case in [
        BeforeStarting,
        LoadingTimeout,
        Stalled,
        StartedAfterStall,
        ResumedAfterStall,
        PositionAfterStall,
        PausedAfterStall,
        EndedAfterStall,
        Failed,
    ] {
        let mut f = Fixture::new(PlaybackMode::Queued);
        let seed = f
            .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
            .await;
        bounded(f.player.play(seed.clone())).await.unwrap();
        let item = f.current();
        let started = !matches!(case, BeforeStarting | LoadingTimeout);
        if started {
            if !matches!(case, StartedAfterStall) {
                f.media.emit(
                    0,
                    MediaPlayerEvent::Started(item.clone(), Duration::ZERO),
                    at(100_100),
                );
            }
            f.media.emit(
                0,
                MediaPlayerEvent::Stalled(item.clone(), position(40)),
                at(100_200),
            );
            f.media.emit(
                0,
                MediaPlayerEvent::Seeked(item.clone(), position(47)),
                at(100_300),
            );
            let failure = MediaError::Other {
                description: "key expired".into(),
                code: "expired".into(),
            };
            let final_event = match case {
                StartedAfterStall => Some(MediaPlayerEvent::Started(
                    item.clone(),
                    Duration::from_secs(47),
                )),
                ResumedAfterStall => Some(MediaPlayerEvent::Resumed(item.clone(), position(47))),
                PositionAfterStall => Some(MediaPlayerEvent::Position(item.clone(), position(47))),
                PausedAfterStall => Some(MediaPlayerEvent::Paused(item.clone(), position(47))),
                EndedAfterStall => Some(MediaPlayerEvent::Ended(
                    item.clone(),
                    Duration::from_secs(47),
                )),
                Failed => Some(MediaPlayerEvent::Failed(
                    item.clone(),
                    Duration::from_secs(47),
                    failure.clone(),
                )),
                _ => None,
            };
            if let Some(event) = final_event {
                f.media.emit(0, event, at(100_400));
            }
        } else if matches!(case, LoadingTimeout) {
            // Loading abandonment uses SystemTime, so this exercises the real five-second threshold.
            tokio::time::sleep(Duration::from_millis(5_100)).await;
        }
        let before_drop = timestamp(SystemTime::now());
        bounded(f.player.reset()).await;
        drop(item);
        let reports = f.collect(if started { 5 } else { 4 }).await;
        let after_drop = timestamp(SystemTime::now());
        let stats = payload(&reports, "playback_statistics");
        assert_eq!(
            stats["endReason"],
            match case {
                LoadingTimeout | Stalled | Failed => "ERROR",
                EndedAfterStall => "COMPLETE",
                _ => "OTHER",
            }
        );
        assert_eq!(
            stats["errorCode"],
            match case {
                LoadingTimeout => json!("pebs"),
                Stalled => json!("pews"),
                Failed => json!("pp:expired"),
                _ => Value::Null,
            }
        );
        if matches!(case, Failed) {
            assert_eq!(stats["stalls"][0]["endTimestamp"], 100_400);
            assert_eq!(stats["endTimestamp"], 100_400);
            assert_eq!(
                payload(&reports, "playback_session")["endTimestamp"],
                100_400
            );
        } else if matches!(case, EndedAfterStall) {
            assert_eq!(stats["stalls"][0]["endTimestamp"], 100_400);
            assert_eq!(stats["endTimestamp"], 100_400);
        } else {
            assert!((before_drop..=after_drop).contains(&stats["endTimestamp"].as_u64().unwrap()));
        }
        if !started {
            assert!(stats["actualStartTimestamp"].is_null());
        } else {
            assert_eq!(
                payload(&reports, "playback_session")["endAssetPosition"],
                47.0
            );
        }
        if matches!(case, Stalled) {
            let stall = &stats["stalls"][0];
            assert_eq!(stall["startTimestamp"], 100_200);
            assert!((before_drop..=after_drop).contains(&stall["endTimestamp"].as_u64().unwrap()));
        }
    }
}

#[tokio::test]
async fn rejected_installations_record_failure_without_a_failed_callback() {
    for mode in [PlaybackMode::Queued, PlaybackMode::Crossfade(5)] {
        for (callbacks, abandon_wait) in [(false, false), (true, false), (true, true)] {
            let mut f = Fixture::new(mode);
            let product = MediaProduct::Track { id: "track".into() };
            let seed = f.prepare(product.clone(), "actual", false).await;
            let installation = f.media.gate_installation(0);
            let player = f.player.clone();
            let pending = tokio::spawn(async move { player.play(seed).await });
            let InstallationCall { item, respond } = bounded(installation).await.unwrap();
            if abandon_wait {
                pending.abort();
            }
            if callbacks {
                for event in [
                    MediaPlayerEvent::Preparing(item.clone()),
                    MediaPlayerEvent::Current {
                        item: Some(item.clone()),
                    },
                    MediaPlayerEvent::Prepared(item.clone()),
                    MediaPlayerEvent::Started(item.clone(), Duration::from_secs(7)),
                    MediaPlayerEvent::Position(item.clone(), position(47)),
                    MediaPlayerEvent::Duration(item.clone(), Duration::from_secs(100)),
                ] {
                    f.media.emit(0, event, SystemTime::now());
                }
            }
            let failure = MediaError::Content {
                code: "installation".into(),
                description: "rejected".into(),
            };
            assert!(respond.send(Err(failure.clone())).is_ok());
            let result = bounded(pending).await;
            let error = player::Error::from(failure);
            if abandon_wait {
                assert!(result.unwrap_err().is_cancelled());
            } else {
                assert_eq!(result.unwrap(), Err(error.clone()));
            }
            loop {
                if let PlayerEvent::StateChanged {
                    item: failed,
                    state: PlayerState::Failed(actual),
                } = bounded(f.notifications.recv()).await.unwrap()
                {
                    assert_eq!(failed, Some(product));
                    assert_eq!(actual, error);
                    break;
                }
            }
            drop(item);
            let reports = f.collect(4 + usize::from(callbacks)).await;
            bounded(f.player.reset()).await;
            // Idle is a public delivery barrier, including any earlier item notifications.
            loop {
                match bounded(f.notifications.recv()).await.unwrap() {
                    PlayerEvent::CurrentItemChanged {
                        item: None,
                        state: PlayerState::Idle,
                    } => break,
                    PlayerEvent::StateChanged {
                        state: PlayerState::Failed(_),
                        ..
                    } => panic!("installation failure reported twice"),
                    _ => {}
                }
            }
            let stats = payload(&reports, "playback_statistics");
            assert_eq!(stats["endReason"], "ERROR");
            assert_eq!(stats["errorCode"], "pc:installation");
            assert_eq!(stats["errorMessage"], error.description());
            if callbacks {
                assert_eq!(
                    payload(&reports, "playback_session")["endAssetPosition"],
                    47.0
                );
            }
        }
    }
}

#[tokio::test]
async fn reset_cancels_installation_without_recording_a_failure() {
    for mode in [PlaybackMode::Queued, PlaybackMode::Crossfade(5)] {
        let mut f = Fixture::new(mode);
        let seed = f
            .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
            .await;
        let installation = f.media.gate_installation(0);
        let player = f.player.clone();
        let pending = tokio::spawn(async move { player.play(seed).await });
        let InstallationCall { item, mut respond } = bounded(installation).await.unwrap();
        bounded(f.player.reset()).await;
        bounded(pending).await.unwrap().unwrap();
        bounded(respond.closed()).await;
        assert!(respond
            .send(Err(MediaError::Other {
                code: "late".into(),
                description: "abandoned installation".into(),
            }))
            .is_err());
        drop(item);
        let reports = f.collect(4).await;
        let stats = payload(&reports, "playback_statistics");
        assert_eq!(stats["endReason"], "OTHER");
        assert_eq!(stats["errorCode"], Value::Null);
        loop {
            match bounded(f.notifications.recv()).await.unwrap() {
                PlayerEvent::CurrentItemChanged {
                    item: None,
                    state: PlayerState::Idle,
                } => break,
                PlayerEvent::StateChanged {
                    state: PlayerState::Failed(_),
                    ..
                } => panic!("cancelled installation reported as failure"),
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn crossfade_replacement_reports_the_request_from_outgoing_progress() {
    let mut f = Fixture::new(PlaybackMode::Crossfade(5));
    let seed = f
        .prepare(MediaProduct::Track { id: "track".into() }, "actual", false)
        .await;
    bounded(f.player.play(seed.clone())).await.unwrap();
    let first = f.current();
    f.selected().await;
    f.media.emit(
        0,
        MediaPlayerEvent::Started(first.clone(), Duration::ZERO),
        at(100_100),
    );
    bounded(f.player.set_next(Some(seed.clone())))
        .await
        .unwrap();
    let next = f.media.current(1);
    f.media.emit(
        0,
        MediaPlayerEvent::Position(first.clone(), position(95)),
        at(100_200),
    );
    assert_eq!(bounded(f.resumes.recv()).await, Some(1));
    f.media.emit(
        1,
        MediaPlayerEvent::Started(next.clone(), Duration::ZERO),
        at(100_250),
    );
    f.media.emit(
        0,
        MediaPlayerEvent::Stalled(first.clone(), position(95)),
        at(100_300),
    );

    bounded(f.player.set_next(Some(seed))).await.unwrap();
    let replacement = f.media.current(1);
    assert!(f.resumes.try_recv().is_err());
    f.media.emit(
        0,
        MediaPlayerEvent::Resumed(first.clone(), position(96)),
        at(100_400),
    );
    assert_eq!(bounded(f.resumes.recv()).await, Some(1));
    f.media.emit(
        1,
        MediaPlayerEvent::Started(replacement.clone(), Duration::ZERO),
        at(100_500),
    );
    bounded(f.player.reset()).await;
    drop(first);
    f.collect(5).await;
    drop(next);
    f.collect(5).await;
    drop(replacement);
    let reports = f.collect(5).await;
    let stats = payload(&reports, "playback_statistics");
    assert_eq!(stats["startReason"], "IMPLICIT");
    assert_eq!(stats["idealStartTimestamp"], 100_400);
    assert_eq!(stats["actualStartTimestamp"], 100_500);
}

#[tokio::test]
async fn automatic_transitions_report_the_intended_start_time_of_the_incoming_item() {
    for mode in [
        PlaybackMode::Queued,
        PlaybackMode::Crossfade(0),
        PlaybackMode::Crossfade(5),
    ] {
        let mut f = Fixture::new(mode);
        let first_seed = f
            .prepare(MediaProduct::Track { id: "first".into() }, "first", false)
            .await;
        bounded(f.player.play(first_seed.clone())).await.unwrap();
        let first = f.current();
        f.selected().await;
        f.media.emit(
            0,
            MediaPlayerEvent::Started(first.clone(), Duration::ZERO),
            at(100_100),
        );
        let next_seed = f
            .prepare(MediaProduct::Track { id: "next".into() }, "next", false)
            .await;
        bounded(f.player.set_next(Some(next_seed.clone())))
            .await
            .unwrap();
        let next_slot = if matches!(mode, PlaybackMode::Queued) {
            0
        } else {
            1
        };
        let next = if next_slot == 0 {
            f.media.next()
        } else {
            f.media.current(1)
        };
        f.media.0.lock().unwrap()[next_slot].installation_error = Some(MediaError::Content {
            code: "installation".into(),
            description: "rejected".into(),
        });
        assert!(bounded(f.player.set_next(Some(next_seed))).await.is_err());
        let rejected_reports = f.collect(3).await;
        assert!(!rejected_reports
            .iter()
            .any(|(name, _)| name == "playback_statistics"));
        let accepted = if next_slot == 0 {
            f.media.next()
        } else {
            f.media.current(1)
        };
        assert!(Arc::ptr_eq(&accepted, &next));
        drop(accepted);
        let rejected_seed = f
            .prepare(
                MediaProduct::Track {
                    id: "rejected".into(),
                },
                "rejected",
                false,
            )
            .await;
        let installation = f.media.gate_installation(next_slot);
        let player = f.player.clone();
        let pending = tokio::spawn(async move { player.set_next(Some(rejected_seed)).await });
        let InstallationCall {
            item: rejected,
            respond,
        } = bounded(installation).await.unwrap();
        f.media.emit(
            next_slot,
            MediaPlayerEvent::Preparing(rejected.clone()),
            SystemTime::now(),
        );
        let failure = MediaError::Other {
            code: "next".into(),
            description: "rejected successor".into(),
        };
        assert!(respond.send(Err(failure.clone())).is_ok());
        assert_eq!(bounded(pending).await.unwrap(), Err(failure.clone().into()));
        let accepted = if next_slot == 0 {
            f.media.next()
        } else {
            f.media.current(1)
        };
        assert!(Arc::ptr_eq(&accepted, &next));
        drop(accepted);
        drop(rejected);
        let rejected_reports = f.collect(3).await;
        assert_eq!(
            payload(&rejected_reports, "streaming_session_start")["sessionProductId"],
            "rejected"
        );
        let overlaps = matches!(mode, PlaybackMode::Crossfade(5));
        if overlaps {
            for (millis, seconds) in [(100_200, 95), (100_250, 96)] {
                f.media.emit(
                    0,
                    MediaPlayerEvent::Position(first.clone(), position(seconds)),
                    at(millis),
                );
                assert_eq!(bounded(f.resumes.recv()).await, Some(1));
            }
        }
        f.media.emit(
            0,
            MediaPlayerEvent::Ended(first.clone(), Duration::from_secs(100)),
            at(100_300),
        );
        if matches!(mode, PlaybackMode::Queued) {
            f.media.advance();
        } else {
            assert_eq!(bounded(f.resumes.recv()).await, Some(1));
        }
        f.media.emit(
            next_slot,
            MediaPlayerEvent::Started(next.clone(), Duration::ZERO),
            at(100_400),
        );
        f.selected().await;
        bounded(f.player.resume()).await;
        bounded(f.player.reset()).await;
        drop(first);
        let first_reports = f.collect(5).await;
        drop(next);
        let next_reports = f.collect(5).await;
        let stats = payload(&next_reports, "playback_statistics");
        assert_eq!(stats["startReason"], "IMPLICIT");
        assert_eq!(
            stats["idealStartTimestamp"],
            if overlaps { 100_200 } else { 100_300 }
        );
        assert_eq!(stats["actualStartTimestamp"], 100_400);
        assert_ne!(
            stats["streamingSessionId"],
            payload(&first_reports, "playback_statistics")["streamingSessionId"]
        );
    }
}
