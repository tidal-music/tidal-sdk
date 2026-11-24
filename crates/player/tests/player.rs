mod support;

use support::playback::{input, pending, ready, resource_id};
use support::{contracts, drm_provider, player_events};

use player::{
    Authenticator, AuthenticatorError, CertificateRequest, DenialReason, DrmError, DrmInput,
    DrmProvider, Error, HttpMethod, HttpRequest, HttpResponse, HttpTransport, LicenseSession,
    MediaError, MediaEvent, MediaInput, MediaItem, MediaPlayer, MediaPlayerEvent,
    MediaPlayerEvents, MediaPlayerFactory, MediaProduct, MediaTime, Player, PlayerConfig,
    PlayerDependencies, ReportingEvent, TidalEventSender, TransportError,
};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

// Descriptions are free text; focused assertions check useful context separately.
fn assert_error(actual: Result<(), Error>, expected: Result<(), Error>) {
    fn without_description(mut error: Error) -> Error {
        match &mut error {
            Error::Denied { description, .. }
            | Error::Network { description, .. }
            | Error::Other { description, .. } => description.clear(),
        }
        error
    }
    assert!(!actual.as_ref().unwrap_err().description().is_empty());
    assert_eq!(
        actual.map_err(without_description),
        expected.map_err(without_description)
    );
}

struct UnusedPlatform;

impl TidalEventSender for UnusedPlatform {
    fn send_event(&self, _event: ReportingEvent) {}
}
#[async_trait::async_trait]

impl HttpTransport for UnusedPlatform {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        panic!("command tests supply prepared seeds");
    }
}
#[async_trait::async_trait]

impl Authenticator for UnusedPlatform {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("command tests supply prepared seeds");
    }

    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("command tests supply prepared seeds");
    }
}

type Items = Arc<Mutex<Vec<Arc<dyn MediaItem>>>>;

struct TestMedia {
    events: Arc<dyn MediaPlayerEvents>,
    items: Option<Items>,
    inputs: Arc<Mutex<Vec<MediaInput>>>,
    calls: Arc<Mutex<Vec<String>>>,
    fail_current: bool,
    play_gate: Mutex<Option<oneshot::Receiver<()>>>,
}

struct TestMediaFactory {
    items: Option<Items>,
    inputs: Arc<Mutex<Vec<MediaInput>>>,
    calls: Arc<Mutex<Vec<String>>>,
    fail_current: bool,
    play_gate: Mutex<Option<oneshot::Receiver<()>>>,
}

impl MediaPlayerFactory for TestMediaFactory {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer> {
        Arc::new(TestMedia {
            events,
            items: self.items.clone(),
            inputs: self.inputs.clone(),
            calls: self.calls.clone(),
            fail_current: self.fail_current,
            play_gate: Mutex::new(self.play_gate.lock().unwrap().take()),
        })
    }
}

impl TestMedia {
    fn observe(&self, item: &Arc<dyn MediaItem>) {
        if let Some(items) = &self.items {
            items.lock().unwrap().push(item.clone());
            self.events
                .on_event(MediaEvent::new(MediaPlayerEvent::Prepared(item.clone())));
        }
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }
}

#[async_trait::async_trait]
impl MediaPlayer for TestMedia {
    async fn set_current(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.record("clear_next");
        self.install(item).await
    }

    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.set_current(item).await?;
        self.play().await;
        Ok(())
    }

    async fn set_next(&self, item: Option<Arc<dyn MediaItem>>) -> Result<(), MediaError> {
        self.record("clear_next");
        if let Some(item) = item {
            let input = item.media_input();
            self.observe(&item);
            self.record(format!("next:{}", resource_id(&input)));
            self.inputs.lock().unwrap().push(input);
        }
        Ok(())
    }

    async fn resume(&self) {
        self.play().await;
    }
    async fn pause(&self) {
        self.record("pause");
    }
    async fn seek(&self, _position: MediaTime) -> Result<(), MediaError> {
        self.record("seek");
        Ok(())
    }
    async fn set_volume(&self, _gain: f32) {}
    async fn reset(&self) {
        self.reset_media();
    }
    fn dispose(&self) {
        self.reset_media();
    }
}

impl TestMedia {
    async fn install(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        let input = item.media_input();
        self.observe(&item);
        self.record(format!("current:{}", resource_id(&input)));
        self.inputs.lock().unwrap().push(input);
        if self.fail_current {
            Err(MediaError::Other {
                description: "media rejected current item".into(),
                code: "rejected".into(),
            })
        } else {
            self.events
                .on_event(MediaEvent::new(MediaPlayerEvent::Current {
                    item: Some(item),
                }));
            Ok(())
        }
    }
    async fn play(&self) {
        self.record("play");
        let gate = self.play_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.await;
        }
    }
    fn reset_media(&self) {
        self.record("reset");
    }
}

fn player(fail_current: bool) -> (Player, Arc<Mutex<Vec<String>>>) {
    player_with_events(fail_current, Arc::new(player_events::Ignore))
}

fn player_with_events(
    fail_current: bool,
    player_events: Arc<dyn player::PlayerEvents>,
) -> (Player, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".to_string(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events,
            event_sender: Arc::new(UnusedPlatform),
            http_transport: Arc::new(UnusedPlatform),
            authenticator: Arc::new(UnusedPlatform),
            drm_provider: Arc::new(drm_provider::NoopDrmProvider),
            media_factory: Arc::new(TestMediaFactory {
                items: None,
                inputs: Arc::new(Mutex::new(Vec::new())),
                calls: calls.clone(),
                fail_current,
                play_gate: Mutex::new(None),
            }),
        },
    );
    (player, calls)
}

#[tokio::test]
async fn play_finishes_only_after_the_controller_play_call_completes() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (finish_play, play_gate) = oneshot::channel();
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".to_string(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events: Arc::new(player_events::Ignore),
            event_sender: Arc::new(UnusedPlatform),
            http_transport: Arc::new(UnusedPlatform),
            authenticator: Arc::new(UnusedPlatform),
            drm_provider: Arc::new(drm_provider::NoopDrmProvider),
            media_factory: Arc::new(TestMediaFactory {
                items: None,
                inputs: Arc::new(Mutex::new(Vec::new())),
                calls: calls.clone(),
                fail_current: false,
                play_gate: Mutex::new(Some(play_gate)),
            }),
        },
    );
    let play_seed = ready(Ok(input()));
    let play = poll_command(player.play(play_seed.clone()));
    tokio::pin!(play);
    wait_for_call(&calls, "play").await;
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(play.as_mut().poll(cx).is_pending()))
            .await
    );
    finish_play.send(()).unwrap();
    play.await.unwrap();
}

async fn wait_for_call(calls: &Mutex<Vec<String>>, expected: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !calls.lock().unwrap().iter().any(|call| call == expected) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

// Poll once so tests can control submission order without waiting for completion.
fn poll_command<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let mut future = Box::pin(future);
    let first = future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
    async move {
        match first {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => future.await,
        }
    }
}

async fn completes(future: impl Future<Output = Result<(), Error>>) {
    tokio::time::timeout(std::time::Duration::from_secs(1), future)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn commands_follow_poll_order_when_awaited_in_reverse() {
    let (player, calls) = player(false);
    let pause = player.pause();
    let resume = player.resume();
    bounded(resume).await;
    bounded(pause).await;
    assert_eq!(*calls.lock().unwrap(), ["play", "pause"]);
}

#[tokio::test]
async fn play_finishes_before_seek_and_pause() {
    let (player, calls) = player(false);
    let (completion, preparation) = pending();
    let play = poll_command(player.play(preparation.clone()));
    wait_for_call(&calls, "reset").await;
    let seek = poll_command(player.seek(MediaTime::ZERO));
    let pause = poll_command(player.pause());
    let item = input();
    let id = item.id;
    assert!(completion.send(Ok(item)).is_ok());
    bounded(pause).await;
    completes(seek).await;
    bounded(play).await.unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "reset".into(),
            "clear_next".into(),
            format!("current:{id}"),
            "play".into(),
            "seek".into(),
            "pause".into()
        ]
    );
}

#[tokio::test]
async fn reset_cancels_loading_and_discards_earlier_queued_commands() {
    let (player, calls) = player(false);
    let (mut completion, preparation) = pending();
    let load = poll_command(player.play(preparation.clone()));
    wait_for_call(&calls, "reset").await;
    let seek = poll_command(player.seek(MediaTime::ZERO));
    let next = poll_command(player.set_next(Some(ready(Ok(input())))));
    let pause = poll_command(player.pause());
    bounded(player.reset()).await;
    bounded(load).await.unwrap();
    completes(seek).await;
    completes(next).await;
    bounded(pause).await;
    drop(preparation);
    tokio::time::timeout(std::time::Duration::from_secs(1), completion.closed())
        .await
        .unwrap();
    assert!(completion.send(Ok(input())).is_err());
    assert_eq!(*calls.lock().unwrap(), ["reset", "reset"]);
}

#[tokio::test]
async fn replacement_discards_old_commands_but_preserves_commands_after_it() {
    let (player, calls) = player(false);
    let (mut completion, preparation) = pending();
    let old = poll_command(player.play(preparation.clone()));
    wait_for_call(&calls, "reset").await;
    let old_seek = poll_command(player.seek(MediaTime::ZERO));
    let old_pause = poll_command(player.pause());
    let item = input();
    let id = item.id;
    let play_seed = ready(Ok(item));
    let play = poll_command(player.play(play_seed.clone()));
    let seek = poll_command(player.seek(MediaTime::ZERO));
    completes(seek).await;
    bounded(play).await.unwrap();
    bounded(old).await.unwrap();
    completes(old_seek).await;
    bounded(old_pause).await;
    drop(preparation);
    tokio::time::timeout(std::time::Duration::from_secs(1), completion.closed())
        .await
        .unwrap();
    assert!(completion.send(Ok(input())).is_err());
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "reset".into(),
            "reset".into(),
            "clear_next".into(),
            format!("current:{id}"),
            "play".into(),
            "seek".into()
        ]
    );
}

#[tokio::test]
async fn seek_and_pause_wait_for_next_in_fifo_order() {
    let (player, calls) = player(false);
    let (completion, preparation) = pending();
    let next = poll_command(player.set_next(Some(preparation)));
    let seek = poll_command(player.seek(MediaTime::ZERO));
    let pause = poll_command(player.pause());
    let item = input();
    let id = item.id;
    assert!(completion.send(Ok(item)).is_ok());
    bounded(pause).await;
    completes(seek).await;
    completes(next).await;
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "clear_next".into(),
            format!("next:{id}"),
            "seek".into(),
            "pause".into()
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn reset_cancels_next_preparation_and_queued_seek() {
    let (player, calls) = player(false);
    let (mut completion, preparation) = pending();
    let mut next = Box::pin(poll_command(player.set_next(Some(preparation))));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), next.as_mut())
            .await
            .is_err()
    );
    let seek = poll_command(player.seek(MediaTime::ZERO));
    bounded(player.reset()).await;
    completes(next).await;
    completes(seek).await;
    tokio::time::timeout(std::time::Duration::from_secs(1), completion.closed())
        .await
        .unwrap();
    assert!(completion.send(Ok(input())).is_err());
    assert_eq!(*calls.lock().unwrap(), ["reset"]);
}

#[tokio::test]
async fn clearing_next_is_an_ordinary_queued_command() {
    let (player, calls) = player(false);
    let (completion, preparation) = pending();
    let next = poll_command(player.set_next(Some(preparation)));
    let clear = poll_command(player.set_next(None));
    let item = input();
    let id = item.id;
    assert!(completion.send(Ok(item)).is_ok());
    completes(clear).await;
    completes(next).await;
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "clear_next".into(),
            format!("next:{id}"),
            "clear_next".into()
        ]
    );
}

#[tokio::test]
async fn rapid_play_requests_only_start_the_latest_item() {
    let (player, calls) = player(false);
    let first_seed = ready(Ok(input()));
    let first = poll_command(player.play(first_seed.clone()));
    let pause = poll_command(player.pause());
    let reset = poll_command(player.reset());
    let item = input();
    let id = item.id;
    let last_seed = ready(Ok(item));
    let last = poll_command(player.play(last_seed.clone()));
    bounded(last).await.unwrap();
    bounded(first).await.unwrap();
    bounded(pause).await;
    bounded(reset).await;
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "reset".into(),
            "clear_next".into(),
            format!("current:{id}"),
            "play".into()
        ]
    );
}

#[tokio::test]
async fn commands_after_reset_are_preserved() {
    let (player, calls) = player(false);
    let (_ready, preparation) = pending();
    let play = poll_command(player.play(preparation.clone()));
    wait_for_call(&calls, "reset").await;
    let reset = poll_command(player.reset());
    let pause = poll_command(player.pause());
    bounded(pause).await;
    bounded(reset).await;
    bounded(play).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["reset", "reset", "pause"]);
}

#[tokio::test]
async fn controller_failure_is_emitted_without_starting_playback() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(true, delegate);
    let play_seed = ready(Ok(input()));
    let result = player.play(play_seed.clone()).await;
    let error = emitted_failure(&mut events).await;
    assert_eq!(result, Err(error.clone()));
    assert_error(
        Err(error),
        Err(Error::Other {
            description: String::new(),
            can_retry: false,
            code: "pp:rejected".into(),
        }),
    );
    assert!(!calls.lock().unwrap().iter().any(|call| call == "play"));
}

#[tokio::test]
async fn preparation_failure_emits_error_and_allows_reset() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    let play_seed = ready(Err(TransportError::OtherError {
        description: "load failed".into(),
    }));
    let load = poll_command(player.play(play_seed.clone()));
    let result = bounded(load).await;
    let error = emitted_failure(&mut events).await;
    assert_eq!(result, Err(error.clone()));
    assert_error(
        Err(error),
        Err(Error::Other {
            description: String::new(),
            can_retry: false,
            code: "mx".into(),
        }),
    );
    assert_eq!(*calls.lock().unwrap(), ["reset"]);
    bounded(player.reset()).await;
    bounded(player.pause()).await;
    assert_eq!(*calls.lock().unwrap(), ["reset", "reset", "pause"]);
}

#[tokio::test]
async fn next_preparation_failure_does_not_reach_controller_or_block_following_commands() {
    let (player, calls) = player(false);
    let seed = ready(Err(TransportError::OtherError {
        description: "load failed".into(),
    }));
    let next = poll_command(player.set_next(Some(seed)));
    let seek = poll_command(player.seek(MediaTime::ZERO));
    assert_error(
        bounded(next).await,
        Err(Error::Other {
            description: String::new(),
            can_retry: false,
            code: "mx".into(),
        }),
    );
    completes(seek).await;
    assert_eq!(*calls.lock().unwrap(), ["seek"]);
}

#[tokio::test]
async fn dropping_the_player_releases_preparation_and_queued_commands() {
    let (player, calls) = player(false);
    let (mut completion, preparation) = pending();
    let play = poll_command(player.play(preparation.clone()));
    wait_for_call(&calls, "reset").await;
    let seek = poll_command(player.seek(MediaTime::ZERO));
    drop(play);
    drop(seek);
    drop(player);
    drop(preparation);
    tokio::time::timeout(std::time::Duration::from_secs(1), completion.closed())
        .await
        .unwrap();
    assert!(completion.send(Ok(input())).is_err());
    wait_for_call(&calls, "reset").await;
    assert_eq!(*calls.lock().unwrap(), ["reset", "reset"]);
}

#[tokio::test]
async fn dropping_an_idle_player_resets_the_controller() {
    let (player, calls) = player(false);
    drop(player);
    wait_for_call(&calls, "reset").await;
}

#[tokio::test]
async fn reset_interrupts_an_active_controller_call_and_discards_waiting_seek() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (finish_play, play_gate) = oneshot::channel();
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".to_string(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events: Arc::new(player_events::Ignore),
            event_sender: Arc::new(UnusedPlatform),
            http_transport: Arc::new(UnusedPlatform),
            authenticator: Arc::new(UnusedPlatform),
            drm_provider: Arc::new(drm_provider::NoopDrmProvider),
            media_factory: Arc::new(TestMediaFactory {
                items: None,
                inputs: Arc::new(Mutex::new(Vec::new())),
                calls: calls.clone(),
                fail_current: false,
                play_gate: Mutex::new(Some(play_gate)),
            }),
        },
    );
    let play_seed = ready(Ok(input()));
    let play = poll_command(player.play(play_seed.clone()));
    wait_for_call(&calls, "play").await;
    let seek = poll_command(player.seek(MediaTime::ZERO));
    let reset = poll_command(player.reset());
    bounded(reset).await;
    bounded(play).await.unwrap();
    assert!(finish_play.send(()).is_err());
    completes(seek).await;
    let calls = calls.lock().unwrap();
    assert_eq!(calls.last().unwrap(), "reset");
    assert!(!calls.iter().any(|call| call == "seek"));
}

#[tokio::test]
async fn commands_continue_after_the_player_has_been_idle() {
    let (player, calls) = player(false);
    bounded(player.pause()).await;
    tokio::task::yield_now().await;
    bounded(player.resume()).await;
    tokio::task::yield_now().await;
    bounded(player.pause()).await;
    assert_eq!(*calls.lock().unwrap(), ["pause", "play", "pause"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_callers_do_not_lose_commands() {
    let (player, calls) = player(false);
    let player = Arc::new(player);
    let mut callers = Vec::new();
    for _ in 0..8 {
        let player = player.clone();
        callers.push(tokio::spawn(async move {
            for _ in 0..10 {
                poll_command(player.pause()).await;
                tokio::task::yield_now().await;
                poll_command(player.resume()).await;
            }
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        for caller in callers {
            caller.await.unwrap();
        }
    })
    .await
    .unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|call| *call == "pause").count(), 80);
    assert_eq!(calls.iter().filter(|call| *call == "play").count(), 80);
}

type DrmResult = Result<Vec<u8>, DrmError>;

/// Holds the manifest response until the test sends it and answers DRM requests directly.
struct LoadingPlatform {
    requests: Mutex<Vec<HttpRequest>>,
    response: Mutex<Option<oneshot::Receiver<HttpResponse>>>,
    request_started: tokio::sync::Notify,
    session: Arc<LoadingSession>,
}

/// The certificate and init data a challenge was created from.
type ChallengeInput = (Vec<u8>, Option<Vec<u8>>);

/// Records each challenge it creates and installs the license with the result the test sends.
struct LoadingSession {
    challenges: Mutex<Vec<ChallengeInput>>,
    drm_result: Mutex<Option<oneshot::Receiver<DrmResult>>>,
    drm_started: tokio::sync::Notify,
}
#[async_trait::async_trait]

impl HttpTransport for LoadingPlatform {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let drm_body = match request.url.as_str() {
            "https://certificate.example" => Some(b"certificate".to_vec()),
            "https://license.example" => Some(b"license".to_vec()),
            _ => None,
        };
        self.requests.lock().unwrap().push(request);
        if let Some(body) = drm_body {
            return Ok(HttpResponse {
                status_code: 200,
                body,
            });
        }
        let response = self.response.lock().unwrap().take().unwrap();
        self.request_started.notify_one();
        Ok(response.await.unwrap())
    }
}
#[async_trait::async_trait]

impl Authenticator for LoadingPlatform {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("token".into())
    }
    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("unexpected refresh")
    }
}

impl DrmProvider for LoadingPlatform {
    fn preloads_licenses(&self) -> bool {
        true
    }

    fn create_session(&self) -> Arc<dyn LicenseSession> {
        self.session.clone()
    }
}
#[async_trait::async_trait]

impl LicenseSession for LoadingSession {
    fn certificate_request(&self) -> CertificateRequest {
        CertificateRequest::Get
    }

    async fn create_challenge(
        &self,
        certificate: Vec<u8>,
        init_data: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, DrmError> {
        self.challenges
            .lock()
            .unwrap()
            .push((certificate, init_data));
        Ok(b"challenge".to_vec())
    }

    async fn install_license(&self, license: Vec<u8>) -> Result<Vec<u8>, DrmError> {
        assert_eq!(license, b"license");
        let result = self.drm_result.lock().unwrap().take().unwrap();
        self.drm_started.notify_one();
        result.await.unwrap()
    }
}

struct LoadingFixture {
    events: tokio::sync::mpsc::UnboundedReceiver<player::PlayerEvent>,
    inputs: Arc<Mutex<Vec<MediaInput>>>,
    items: Items,
    player: Player,
    platform: Arc<LoadingPlatform>,
    response: oneshot::Sender<HttpResponse>,
    license: oneshot::Sender<DrmResult>,
    calls: Arc<Mutex<Vec<String>>>,
}

fn loading_fixture() -> LoadingFixture {
    loading_fixture_with_transport(None)
}

fn loading_fixture_with_transport(transport: Option<Arc<dyn HttpTransport>>) -> LoadingFixture {
    loading_fixture_with_options(transport, true)
}

fn loading_fixture_with_options(
    transport: Option<Arc<dyn HttpTransport>>,
    prepare_drm: bool,
) -> LoadingFixture {
    let (response, receive_response) = oneshot::channel();
    let (license, receive_license) = oneshot::channel();
    let platform = Arc::new(LoadingPlatform {
        requests: Mutex::new(Vec::new()),
        response: Mutex::new(Some(receive_response)),
        request_started: tokio::sync::Notify::new(),
        session: Arc::new(LoadingSession {
            challenges: Mutex::new(Vec::new()),
            drm_result: Mutex::new(Some(receive_license)),
            drm_started: tokio::sync::Notify::new(),
        }),
    });
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let items = Arc::new(Mutex::new(Vec::new()));
    let (player_events, events) = player_events::record();
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".into(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events,
            event_sender: Arc::new(UnusedPlatform),
            media_factory: Arc::new(TestMediaFactory {
                items: Some(items.clone()),
                inputs: inputs.clone(),
                calls: calls.clone(),
                fail_current: false,
                play_gate: Mutex::new(None),
            }),
            http_transport: transport.unwrap_or_else(|| platform.clone()),
            authenticator: platform.clone(),
            drm_provider: if prepare_drm {
                platform.clone()
            } else {
                Arc::new(drm_provider::NoopDrmProvider)
            },
        },
    );
    LoadingFixture {
        events,
        inputs,
        items,
        player,
        platform,
        response,
        license,
        calls,
    }
}

fn manifest_response(encrypted: bool) -> HttpResponse {
    let mut attributes = json!({
        "uri": "https://media.example/track",
        "link": {"href": "https://media.example/video"},
        "formats": ["FLAC"],
        "trackAudioNormalizationData": {"peakAmplitude": 0.5, "replayGain": -2.0},
    });
    if encrypted {
        attributes["drmData"] = json!({
            "drmSystem": "FAIRPLAY",
            "licenseUrl": "https://license.example",
            "certificateUrl": "https://certificate.example",
            "initData": ["init"],
        });
    }
    HttpResponse {
        status_code: 200,
        body: serde_json::to_vec(
            &json!({"data": {"id": "resolved-product", "attributes": attributes}}),
        )
        .unwrap(),
    }
}

async fn emitted_failure(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<player::PlayerEvent>,
) -> Error {
    loop {
        if let player::PlayerEvent::StateChanged {
            state: player::PlayerState::Failed(error),
            ..
        } = bounded(events.recv()).await.expect("player events closed")
        {
            return error;
        }
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(2), future)
        .await
        .expect("operation timed out")
}

#[tokio::test]
async fn loading_fetches_the_manifest_then_prepares_drm_without_starting_playback() {
    for product in [
        MediaProduct::Track { id: "1".into() },
        MediaProduct::Video { id: "2".into() },
    ] {
        let fixture = loading_fixture();
        let seed = fixture.player.prepare(product.clone());
        bounded(fixture.platform.request_started.notified()).await;
        assert!(fixture
            .platform
            .session
            .challenges
            .lock()
            .unwrap()
            .is_empty());
        fixture.response.send(manifest_response(true)).unwrap();
        bounded(fixture.platform.session.drm_started.notified()).await;
        assert!(fixture.calls.lock().unwrap().is_empty());
        let mut waiting = Box::pin(poll_command(fixture.player.play(seed.clone())));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                waiting.as_mut().poll(cx).is_pending()
            ))
            .await
        );
        assert!(fixture.license.send(Ok(b"handle".to_vec())).is_ok());
        bounded(waiting).await.unwrap();
        let inputs = fixture.inputs.lock().unwrap();
        let input = &inputs[0];
        match &product {
            MediaProduct::Track { .. } => {
                assert_eq!(input.url, "https://media.example/track");
                assert_eq!(input.formats, vec![player::AudioFormat::Flac]);
                assert_eq!(
                    input.track_normalization_data.unwrap().replay_gain,
                    Some(-2.0)
                );
            }
            MediaProduct::Video { .. } => {
                assert_eq!(input.url, "https://media.example/video");
                assert!(input.formats.is_empty());
                assert!(input.album_normalization_data.is_none());
                assert!(input.track_normalization_data.is_none());
            }
        }
        assert_eq!(
            input.drm,
            Some(DrmInput::Prepared {
                handle: b"handle".to_vec()
            })
        );
        assert_eq!(
            *fixture.platform.session.challenges.lock().unwrap(),
            [(b"certificate".to_vec(), Some(b"init".to_vec()))]
        );
        let requests = fixture.platform.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0].headers.get("Authorization").unwrap(),
            "Bearer token"
        );
        let (certificate, license) = (&requests[1], &requests[2]);
        assert_eq!(certificate.method, HttpMethod::Get);
        assert_eq!(certificate.url, "https://certificate.example");
        assert_eq!(
            certificate.headers.get("Authorization").unwrap(),
            "Bearer token"
        );
        assert_eq!(license.method, HttpMethod::Post);
        assert_eq!(license.url, "https://license.example");
        assert_eq!(license.body.as_deref(), Some(&b"challenge"[..]));
        assert_eq!(
            license.headers.get("Authorization").unwrap(),
            "Bearer token"
        );
        let path = match product {
            MediaProduct::Track { .. } => "/trackManifests/1?",
            MediaProduct::Video { .. } => "/videoManifests/2?",
        };
        assert!(requests[0].url.contains(path));
        assert!(fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == "play"));
    }
}

#[tokio::test]
async fn loading_without_preloading_leaves_fetches_to_the_media_player() {
    for encrypted in [false, true] {
        let fixture = loading_fixture_with_options(None, false);
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        fixture.response.send(manifest_response(encrypted)).unwrap();
        bounded(fixture.player.play(seed.clone())).await.unwrap();
        let inputs = fixture.inputs.lock().unwrap();
        assert_eq!(inputs[0].drm, encrypted.then_some(DrmInput::OnDemand));
        assert_eq!(fixture.platform.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn the_preloading_session_lives_as_long_as_its_seed_and_items() {
    let fixture = loading_fixture();
    let session = &fixture.platform.session;
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture.response.send(manifest_response(true)).unwrap();
    assert!(fixture.license.send(Ok(b"handle".to_vec())).is_ok());
    bounded(fixture.player.play(seed.clone())).await.unwrap();

    // Held by the platform, the seed and the item.
    assert_eq!(Arc::strong_count(session), 3);
    drop(seed);
    assert_eq!(Arc::strong_count(session), 2);
}

#[tokio::test]
async fn the_media_player_fetches_certificates_and_licenses_through_the_item() {
    for (certificate_request, method, body) in [
        (CertificateRequest::Get, HttpMethod::Get, None),
        (
            CertificateRequest::Post(b"service certificate request".to_vec()),
            HttpMethod::Post,
            Some(&b"service certificate request"[..]),
        ),
    ] {
        let fixture = loading_fixture_with_options(None, false);
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        fixture.response.send(manifest_response(true)).unwrap();
        bounded(fixture.player.play(seed)).await.unwrap();
        let item = fixture.items.lock().unwrap()[0].clone();

        let certificate = bounded(item.clone().fetch_certificate(certificate_request)).await;
        let license = bounded(item.clone().fetch_license(b"challenge".to_vec())).await;

        assert_eq!(certificate, Ok(b"certificate".to_vec()));
        assert_eq!(license, Ok(b"license".to_vec()));
        let requests = fixture.platform.requests.lock().unwrap();
        let (certificate, license) = (&requests[1], &requests[2]);
        assert_eq!(certificate.method, method);
        assert_eq!(certificate.url, "https://certificate.example");
        assert_eq!(certificate.body.as_deref(), body);
        assert_eq!(license.method, HttpMethod::Post);
        assert_eq!(license.url, "https://license.example");
        assert_eq!(license.body.as_deref(), Some(&b"challenge"[..]));
    }
}

#[tokio::test]
async fn a_certificate_is_reused_for_the_same_url_and_request() {
    let fixture = loading_fixture_with_options(None, false);
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture.response.send(manifest_response(true)).unwrap();
    bounded(fixture.player.play(seed)).await.unwrap();
    let item = fixture.items.lock().unwrap()[0].clone();
    let post = CertificateRequest::Post(b"service certificate request".to_vec());

    for request in [
        CertificateRequest::Get,
        CertificateRequest::Get,
        post.clone(),
        post,
    ] {
        let certificate = bounded(item.clone().fetch_certificate(request)).await;
        assert_eq!(certificate, Ok(b"certificate".to_vec()));
    }

    let requests = fixture.platform.requests.lock().unwrap();
    let methods: Vec<_> = requests[1..].iter().map(|request| request.method).collect();
    assert_eq!(methods, [HttpMethod::Get, HttpMethod::Post]);
}

#[tokio::test]
async fn content_without_drm_has_no_certificate_or_license_to_fetch() {
    let fixture = loading_fixture_with_options(None, false);
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture.response.send(manifest_response(false)).unwrap();
    bounded(fixture.player.play(seed)).await.unwrap();
    let item = fixture.items.lock().unwrap()[0].clone();
    let no_drm = Err(Error::Other {
        description: String::new(),
        can_retry: false,
        code: "dnd".into(),
    });

    let certificate = bounded(item.clone().fetch_certificate(CertificateRequest::Get)).await;
    let license = bounded(item.clone().fetch_license(b"challenge".to_vec())).await;

    assert_error(certificate.map(|_| ()), no_drm.clone());
    assert_error(license.map(|_| ()), no_drm);
    assert_eq!(fixture.platform.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn license_failures_during_playback_carry_the_drm_stage() {
    for (license, expected) in license_failure_cases() {
        let fixture =
            loading_fixture_with_options(Some(LicenseFailureTransport::new(license)), false);
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        bounded(fixture.player.play(seed)).await.unwrap();
        let item = fixture.items.lock().unwrap()[0].clone();

        let result = bounded(item.clone().fetch_license(b"challenge".to_vec())).await;

        assert_error(result.map(|_| ()), Err(expected));
    }
}

#[tokio::test]
async fn license_failures_while_preloading_fail_the_load() {
    for (license, expected) in license_failure_cases() {
        let mut fixture =
            loading_fixture_with_options(Some(LicenseFailureTransport::new(license)), true);
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        bounded(fixture.player.play(seed)).await.unwrap_err();
        assert_error(
            Err(emitted_failure(&mut fixture.events).await),
            Err(expected),
        );
        assert!(fixture.inputs.lock().unwrap().is_empty());
    }
}

fn license_failure_cases() -> Vec<(Result<u16, TransportError>, Error)> {
    let http = |status: u16, can_retry| {
        (
            Ok(status),
            Error::Other {
                description: String::new(),
                can_retry,
                code: format!("dh:{status}"),
            },
        )
    };
    vec![
        http(403, false),
        http(429, true),
        http(503, true),
        (
            Err(TransportError::NetworkError {
                description: "offline".into(),
            }),
            Error::Network {
                description: String::new(),
                code: "dn".into(),
            },
        ),
    ]
}

/// Serves a manifest and certificate, and answers license requests with the given outcome.
struct LicenseFailureTransport {
    license: Result<u16, TransportError>,
}

impl LicenseFailureTransport {
    fn new(license: Result<u16, TransportError>) -> Arc<Self> {
        Arc::new(Self { license })
    }
}
#[async_trait::async_trait]

impl HttpTransport for LicenseFailureTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        Ok(match request.url.as_str() {
            "https://license.example" => HttpResponse {
                status_code: self.license.clone()?,
                body: Vec::new(),
            },
            "https://certificate.example" => HttpResponse {
                status_code: 200,
                body: b"certificate".to_vec(),
            },
            _ => HttpResponse {
                status_code: 200,
                body: serde_json::to_vec(&json!({"data": {"id": "1", "attributes": {
                    "uri": "https://media.example/track",
                    "drmData": {
                        "drmSystem": "WIDEVINE",
                        "licenseUrl": "https://license.example",
                        "certificateUrl": "https://certificate.example",
                        "initData": ["init"],
                    },
                }}}))
                .unwrap(),
            },
        })
    }
}

#[tokio::test]
async fn seed_reuse_shares_preparation() {
    let fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    contracts::assert_player_contracts(&fixture.player, &seed);
    let clone = seed.clone();
    fixture.response.send(manifest_response(true)).unwrap();
    assert!(fixture.license.send(Ok(b"handle".to_vec())).is_ok());
    bounded(fixture.player.play(seed.clone())).await.unwrap();
    bounded(fixture.player.play(clone.clone())).await.unwrap();
    completes(fixture.player.set_next(Some(seed.clone()))).await;
    let inputs = fixture.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 3);
    for input in &inputs[1..] {
        assert_eq!(input.drm, inputs[0].drm);
    }
    assert_eq!(fixture.platform.requests.lock().unwrap().len(), 3);
    assert_eq!(fixture.platform.session.challenges.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn missing_null_or_empty_init_data_reaches_the_session_as_none() {
    for init_data in [None, Some(json!(null)), Some(json!([]))] {
        let fixture = loading_fixture();
        let _seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        let mut response: Value = serde_json::from_slice(&manifest_response(true).body).unwrap();
        let drm = &mut response["data"]["attributes"]["drmData"];
        match init_data {
            Some(init_data) => drm["initData"] = init_data,
            None => {
                drm.as_object_mut().unwrap().remove("initData");
            }
        }
        fixture
            .response
            .send(HttpResponse {
                status_code: 200,
                body: serde_json::to_vec(&response).unwrap(),
            })
            .unwrap();
        bounded(fixture.platform.session.drm_started.notified()).await;
        assert_eq!(
            *fixture.platform.session.challenges.lock().unwrap(),
            [(b"certificate".to_vec(), None)]
        );
    }
}

#[tokio::test]
async fn unencrypted_content_skips_drm_and_load_can_run_outside_the_runtime() {
    let fixture = loading_fixture();
    let seed = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                fixture
                    .player
                    .prepare(MediaProduct::Track { id: "1".into() })
            })
            .join()
            .unwrap()
    });
    fixture.response.send(manifest_response(false)).unwrap();
    bounded(fixture.player.play(seed.clone())).await.unwrap();
    let inputs = fixture.inputs.lock().unwrap();
    let input = &inputs[0];
    assert!(input.drm.is_none());
    assert!(fixture
        .platform
        .session
        .challenges
        .lock()
        .unwrap()
        .is_empty());
}

struct FixedTransport(Result<HttpResponse, TransportError>);
#[async_trait::async_trait]

impl HttpTransport for FixedTransport {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.0.clone()
    }
}

#[tokio::test(start_paused = true)]
async fn manifest_errors_preserve_stage_and_expose_retry_guidance() {
    let cases = [
        (
            Err(TransportError::TimeoutError {
                description: "timed out".into(),
            }),
            Error::Network {
                description: String::new(),
                code: "mt".into(),
            },
        ),
        (
            Err(TransportError::NetworkError {
                description: "offline".into(),
            }),
            Error::Network {
                description: String::new(),
                code: "mn".into(),
            },
        ),
        (
            Ok(HttpResponse {
                status_code: 503,
                body: Vec::new(),
            }),
            Error::Other {
                description: String::new(),
                can_retry: true,
                code: "mh:503".into(),
            },
        ),
        (
            Ok(HttpResponse {
                status_code: 200,
                body: b"invalid json".to_vec(),
            }),
            Error::Other {
                description: String::new(),
                can_retry: false,
                code: "mdec".into(),
            },
        ),
    ];
    for (response, expected) in cases {
        let description_fragment = match &response {
            Err(source) => source.to_string(),
            Ok(response) if response.status_code == 200 => "/trackManifests/1?".into(),
            Ok(response) => response.status_code.to_string(),
        };
        let mut fixture = loading_fixture_with_transport(Some(Arc::new(FixedTransport(response))));
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        fixture.player.play(seed.clone()).await.unwrap_err();
        let error = emitted_failure(&mut fixture.events).await;
        assert!(error.description().contains(&description_fragment));
        assert_error(Err(error), Err(expected));
        assert!(fixture.inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn manifest_failure_is_cached_without_starting_drm() {
    let mut fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture
        .response
        .send(HttpResponse {
            status_code: 403,
            body: Vec::new(),
        })
        .unwrap();
    for _ in 0..2 {
        bounded(fixture.player.play(seed.clone()))
            .await
            .unwrap_err();
        assert_error(
            Err(emitted_failure(&mut fixture.events).await),
            Err(Error::Other {
                description: String::new(),
                can_retry: false,
                code: "mh:403".into(),
            }),
        );
    }
    assert!(fixture
        .platform
        .session
        .challenges
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(fixture.platform.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn manifest_denials_report_reasons_and_distinct_diagnostic_codes() {
    let cases = [
        (
            404,
            "CONTENT_NOT_FOUND",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::ContentNotFound,
                code: "mcnf".into(),
            },
        ),
        (
            403,
            "PREREQUISITE_MISSING",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::PrerequisiteMissing,
                code: "mprm".into(),
            },
        ),
        (
            403,
            "GEO_RESTRICTED",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::GeoRestricted,
                code: "mgeo".into(),
            },
        ),
        (
            403,
            "PURCHASE_REQUIRED",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::PurchaseRequired,
                code: "mpur".into(),
            },
        ),
        (
            403,
            "CLIENT_NOT_ENTITLED",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::ClientNotEntitled,
                code: "mcne".into(),
            },
        ),
        (
            403,
            "CONCURRENT_PLAYBACK",
            Error::Denied {
                description: String::new(),
                reason: DenialReason::ConcurrentPlayback,
                code: "mccp".into(),
            },
        ),
    ];
    for (status_code, api_code, expected) in cases {
        for product in [
            MediaProduct::Track { id: "1".into() },
            MediaProduct::Video { id: "2".into() },
        ] {
            let mut fixture = loading_fixture();
            let seed = fixture.player.prepare(product);
            fixture
                .response
                .send(HttpResponse {
                    status_code,
                    body: serde_json::to_vec(&json!({"errors": [{"code": api_code}]})).unwrap(),
                })
                .unwrap();
            bounded(fixture.player.play(seed.clone()))
                .await
                .unwrap_err();
            let error = emitted_failure(&mut fixture.events).await;
            assert!(!error.description().contains(api_code));
            assert_error(Err(error), Err(expected.clone()));
            assert!(fixture
                .platform
                .session
                .challenges
                .lock()
                .unwrap()
                .is_empty());
            assert!(fixture.inputs.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn manifest_responses_that_are_not_a_recognised_denial_are_reported_by_status() {
    let errors = |errors: Value| serde_json::to_vec(&json!({ "errors": errors })).unwrap();
    let mut cases = vec![
        (302, Vec::new()),
        (400, Vec::new()),
        (404, Vec::new()),
        (429, Vec::new()),
        (403, Vec::new()),
        (403, b"not JSON".to_vec()),
        (403, br#"{"errors":null}"#.to_vec()),
        (403, errors(json!([]))),
        (403, errors(json!([{}]))),
        (403, errors(json!([{"code": 42}]))),
        (403, errors(json!([{"code": "FUTURE_CODE"}]))),
        // A known code only counts with the status it belongs to.
        (403, errors(json!([{"code": "CONTENT_NOT_FOUND"}]))),
        (404, errors(json!([{"code": "GEO_RESTRICTED"}]))),
        (409, errors(json!([{"code": "CONCURRENT_PLAYBACK"}]))),
        (500, errors(json!([{"code": "GEO_RESTRICTED"}]))),
        (503, Vec::new()),
        (503, b"not JSON".to_vec()),
        // Only the first error object decides.
        (
            403,
            errors(json!([{"code": "FUTURE_CODE"}, {"code": "GEO_RESTRICTED"}])),
        ),
        (
            403,
            errors(json!([{"detail": "No code"}, {"code": "GEO_RESTRICTED"}])),
        ),
    ];
    cases.sort_by_key(|(status, _)| *status);
    for (status_code, body) in cases {
        for product in [
            MediaProduct::Track { id: "1".into() },
            MediaProduct::Video { id: "2".into() },
        ] {
            let mut fixture =
                loading_fixture_with_transport(Some(Arc::new(FixedTransport(Ok(HttpResponse {
                    status_code,
                    body: body.clone(),
                })))));
            let seed = fixture.player.prepare(product);
            // Retried server errors take longer than `bounded` allows in paused time.
            let returned = fixture.player.play(seed.clone()).await.unwrap_err();
            let emitted = emitted_failure(&mut fixture.events).await;
            assert_eq!(returned, emitted);
            assert_error(
                Err(emitted),
                Err(Error::Other {
                    description: String::new(),
                    can_retry: matches!(status_code, 408 | 429 | 500..=599),
                    code: format!("mh:{status_code}"),
                }),
            );
        }
    }
}

#[tokio::test]
async fn the_first_error_object_of_a_manifest_response_decides_the_denial() {
    let body = serde_json::to_vec(
        &json!({"errors": [{"code": "PURCHASE_REQUIRED"}, {"code": "GEO_RESTRICTED"}]}),
    )
    .unwrap();
    let mut fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture
        .response
        .send(HttpResponse {
            status_code: 403,
            body,
        })
        .unwrap();
    bounded(fixture.player.play(seed.clone()))
        .await
        .unwrap_err();
    assert_error(
        Err(emitted_failure(&mut fixture.events).await),
        Err(Error::Denied {
            description: String::new(),
            reason: DenialReason::PurchaseRequired,
            code: "mpur".into(),
        }),
    );
}

#[tokio::test]
async fn unrecognised_manifest_failures_preserve_status_without_exposing_api_codes() {
    let mut fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture
        .response
        .send(HttpResponse {
            status_code: 403,
            body: serde_json::to_vec(&json!({"errors": [{"code": "NEW_REASON"}]})).unwrap(),
        })
        .unwrap();
    bounded(fixture.player.play(seed.clone()))
        .await
        .unwrap_err();
    let error = emitted_failure(&mut fixture.events).await;
    assert!(!format!("{error:?}").contains("NEW_REASON"));
    assert!(!error.to_string().contains("NEW_REASON"));
    assert_error(
        Err(error),
        Err(Error::Other {
            description: String::new(),
            can_retry: false,
            code: "mh:403".into(),
        }),
    );
}

#[tokio::test]
async fn license_failure_reaches_the_caller_and_is_cached() {
    for (platform_code, expected_code) in [
        (Some("license_rejected"), "do:license_rejected"),
        (None, "do"),
    ] {
        let mut fixture = loading_fixture();
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "1".into() });
        fixture.response.send(manifest_response(true)).unwrap();
        assert!(fixture
            .license
            .send(Err(DrmError::Failed {
                code: platform_code.map(str::to_owned),
                description: "license rejected".into(),
            }))
            .is_ok());
        for _ in 0..2 {
            bounded(fixture.player.play(seed.clone()))
                .await
                .unwrap_err();
            assert_error(
                Err(emitted_failure(&mut fixture.events).await),
                Err(Error::Other {
                    description: String::new(),
                    can_retry: false,
                    code: expected_code.into(),
                }),
            );
        }
        assert_eq!(fixture.platform.session.challenges.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn dropping_the_last_seed_abandons_http_without_starting_drm() {
    let mut fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    let clone = seed.clone();
    bounded(fixture.platform.request_started.notified()).await;
    drop(seed);
    assert!(!fixture.response.is_closed());
    drop(clone);
    bounded(fixture.response.closed()).await;
    assert!(fixture.response.send(manifest_response(true)).is_err());
    assert!(fixture
        .platform
        .session
        .challenges
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn dropping_the_last_seed_cancels_drm_preparation() {
    let mut fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    fixture.response.send(manifest_response(true)).unwrap();
    bounded(fixture.platform.session.drm_started.notified()).await;
    assert!(!fixture.license.is_closed());
    drop(seed);
    bounded(fixture.license.closed()).await;
}

#[tokio::test]
async fn resetting_play_keeps_preparation_alive_for_a_retained_seed() {
    let fixture = loading_fixture();
    let seed = fixture
        .player
        .prepare(MediaProduct::Track { id: "1".into() });
    let play = poll_command(fixture.player.play(seed.clone()));
    wait_for_call(&fixture.calls, "reset").await;
    bounded(fixture.player.reset()).await;
    bounded(play).await.unwrap();
    fixture.response.send(manifest_response(true)).unwrap();
    assert!(fixture.license.send(Ok(b"handle".to_vec())).is_ok());
    bounded(fixture.platform.session.drm_started.notified()).await;
    assert_eq!(*fixture.calls.lock().unwrap(), ["reset", "reset"]);
    bounded(fixture.player.play(seed.clone())).await.unwrap();
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        [
            "reset",
            "reset",
            "reset",
            "clear_next",
            "current:track",
            "play"
        ]
    );
}

fn item_player(
    play_gate: Option<oneshot::Receiver<()>>,
) -> (
    Player,
    Arc<Mutex<Vec<String>>>,
    Items,
    tokio::sync::mpsc::UnboundedReceiver<player::PlayerEvent>,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let items = Arc::new(Mutex::new(Vec::new()));
    let (player_events, events) = player_events::record();
    let player = Player::new(
        PlayerConfig {
            base_url: "https://openapi.tidal.com/v2".into(),
            manifest_configuration: Default::default(),
            crossfade: None,
        },
        PlayerDependencies {
            player_events,
            event_sender: Arc::new(UnusedPlatform),
            http_transport: Arc::new(UnusedPlatform),
            authenticator: Arc::new(UnusedPlatform),
            drm_provider: Arc::new(drm_provider::NoopDrmProvider),
            media_factory: Arc::new(TestMediaFactory {
                items: Some(items.clone()),
                inputs: Arc::new(Mutex::new(Vec::new())),
                calls: calls.clone(),
                fail_current: false,
                play_gate: Mutex::new(play_gate),
            }),
        },
    );
    (player, calls, items, events)
}

#[tokio::test]
async fn controller_events_reach_player_while_a_command_is_waiting() {
    let (release, gate) = oneshot::channel();
    let (player, calls, _items, mut events) = item_player(Some(gate));
    let seed = ready(Ok(input()));
    let mut play = Box::pin(poll_command(player.play(seed.clone())));
    wait_for_call(&calls, "play").await;

    bounded(events.recv()).await.unwrap();
    bounded(events.recv()).await.unwrap();
    std::future::poll_fn(|cx| {
        assert!(play.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    bounded(play).await.unwrap();
}

#[tokio::test]
async fn play_and_set_next_receive_distinct_items_when_reusing_a_seed() {
    let (player, _calls, items, _events) = item_player(None);
    let seed = ready(Ok(input()));
    bounded(player.play(seed.clone())).await.unwrap();
    completes(player.set_next(Some(seed.clone()))).await;
    bounded(player.play(seed.clone())).await.unwrap();
    completes(player.set_next(None)).await;
    let items = items.lock().unwrap().clone();
    assert_eq!(items.len(), 3);
    assert!(!Arc::ptr_eq(&items[0], &items[1]));
    assert!(!Arc::ptr_eq(&items[0], &items[2]));
    assert!(!Arc::ptr_eq(&items[1], &items[2]));
}

#[tokio::test]
async fn retaining_an_item_does_not_keep_the_controller_alive() {
    let (player, calls, items, mut events) = item_player(None);
    bounded(player.play(ready(Ok(input())))).await.unwrap();
    bounded(events.recv()).await.unwrap();
    bounded(events.recv()).await.unwrap();
    let _retained = items.lock().unwrap()[0].clone();
    let controller_calls = Arc::downgrade(&calls);
    calls.lock().unwrap().clear();
    drop(player);
    wait_for_call(&calls, "reset").await;
    drop(calls);
    assert!(controller_calls.upgrade().is_none());
}

#[tokio::test]
async fn player_subscription_does_not_retain_the_item() {
    let (player, _calls, items, mut events) = item_player(None);
    bounded(player.play(ready(Ok(input())))).await.unwrap();
    while !matches!(
        bounded(events.recv()).await.unwrap(),
        player::PlayerEvent::CurrentItemChanged { item: Some(_), .. }
    ) {}

    let item = items.lock().unwrap().pop().unwrap();
    let weak_item = Arc::downgrade(&item);
    drop(item);
    assert!(weak_item.upgrade().is_none());
    bounded(player.reset()).await;
    bounded(events.recv()).await.unwrap();
    assert!(weak_item.upgrade().is_none());
}

#[tokio::test]
async fn event_task_does_not_keep_player_alive() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
    wait_for_call(&calls, "reset").await;
}

#[tokio::test]
async fn controller_notifications_reach_delegate_and_clear_current() {
    let (player, _calls, _items, mut events) = item_player(None);
    bounded(player.play(ready(Ok(input())))).await.unwrap();
    while !matches!(
        bounded(events.recv()).await.unwrap(),
        player::PlayerEvent::CurrentItemChanged { item: Some(_), .. }
    ) {}

    completes(player.set_next(Some(ready(Ok(input()))))).await;
    bounded(player.reset()).await;
    while !matches!(
        bounded(events.recv()).await.unwrap(),
        player::PlayerEvent::CurrentItemChanged { item: None, .. }
    ) {}
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_events_buffer_and_dispatch_serially_without_blocking_commands() {
    let (delivered, mut events) = tokio::sync::mpsc::unbounded_channel();
    let (release, gate) = std::sync::mpsc::channel();
    let gate = Mutex::new(Some(gate));
    let delegate = Arc::new(player_events::Callback(move |event| {
        let first = gate.lock().unwrap().take();
        delivered.send(event).unwrap();
        if let Some(gate) = first {
            gate.recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
        }
    }));
    let (player, _) = player_with_events(false, delegate);
    bounded(player.reset()).await;
    bounded(events.recv()).await.unwrap();
    // Keep the first callback blocked while more commands produce notifications.
    for _ in 0..100 {
        bounded(player.reset()).await;
        bounded(player.pause()).await;
    }
    assert!(events.try_recv().is_err());
    release.send(()).unwrap();
    for _ in 0..100 {
        bounded(events.recv()).await.unwrap();
    }
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn player_event_callback_can_reenter_player_commands_on_a_single_thread() {
    let target = Arc::new(Mutex::new(std::sync::Weak::<Player>::new()));
    let callback_target = target.clone();
    let (delivered, mut events) = tokio::sync::mpsc::unbounded_channel();
    let delegate = Arc::new(player_events::Callback(move |event| {
        let player = callback_target.lock().unwrap().upgrade().unwrap();
        let configuration = player.get_manifest_configuration();
        player.set_manifest_configuration(configuration.clone());
        assert_eq!(player.get_manifest_configuration(), configuration);
        let delivered = delivered.clone();
        tokio::spawn(async move {
            player.pause().await;
            delivered.send(event).unwrap();
        });
    }));
    let (player, calls) = player_with_events(false, delegate);
    let player = Arc::new(player);
    *target.lock().unwrap() = Arc::downgrade(&player);
    bounded(player.reset()).await;
    bounded(events.recv()).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["reset", "pause"]);
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
    assert!(target.lock().unwrap().upgrade().is_none());
}

#[tokio::test]
async fn play_and_next_items_calculate_volume_from_track_normalization() {
    let album = json!({"peakAmplitude": 0.75, "replayGain": -44.0});
    for (track, album, expected) in [
        (
            json!({"peakAmplitude": 0.5, "replayGain": -24.0}),
            album.clone(),
            0.1,
        ),
        (
            json!({"peakAmplitude": 0.5, "replayGain": -10.0}),
            json!(null),
            0.5011872,
        ),
        (
            json!({"peakAmplitude": 0.5, "replayGain": -4.0}),
            album.clone(),
            1.0,
        ),
        (
            json!({"peakAmplitude": 0.5, "replayGain": 16.0}),
            album.clone(),
            1.0,
        ),
        (
            json!({"peakAmplitude": 2.0, "replayGain": -24.0}),
            album.clone(),
            0.1,
        ),
        (
            json!({"peakAmplitude": 0.0, "replayGain": -24.0}),
            album.clone(),
            0.1,
        ),
        (json!({"replayGain": -24.0}), album.clone(), 1.0),
        (json!({"peakAmplitude": 0.5}), album.clone(), 1.0),
        (json!({}), album.clone(), 1.0),
        (json!(null), album, 1.0),
    ] {
        let fixture = loading_fixture_with_options(None, false);
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "track".into() });
        fixture
            .response
            .send(HttpResponse {
                status_code: 200,
                body: serde_json::to_vec(
                    &json!({"data": {"id": "resolved-product", "attributes": {
                        "uri": "https://media.example/track",
                        "trackAudioNormalizationData": track,
                        "albumAudioNormalizationData": album,
                    }}}),
                )
                .unwrap(),
            })
            .unwrap();

        bounded(fixture.player.play(seed.clone())).await.unwrap();
        completes(fixture.player.set_next(Some(seed))).await;
        let items = fixture.items.lock().unwrap();
        assert_eq!(items.len(), 2);
        for item in items.iter() {
            assert!(
                (item.volume() - expected).abs() < 0.000001,
                "track {track}: expected {expected}, got {}",
                item.volume(),
            );
        }
    }
}

#[tokio::test]
async fn media_input_preserves_all_known_formats_and_independent_normalization() {
    for formats in [
        json!(["FLAC", "FUTURE_FORMAT", "AACLC"]),
        json!(null),
        json!(["FUTURE_FORMAT"]),
    ] {
        let fixture = loading_fixture();
        let seed = fixture
            .player
            .prepare(MediaProduct::Track { id: "track".into() });
        fixture
            .response
            .send(HttpResponse {
                status_code: 200,
                body: serde_json::to_vec(
                    &json!({"data": {"id": "resolved-product", "attributes": {
                        "uri": "https://media.example/track",
                        "formats": formats,
                        "albumAudioNormalizationData": {"peakAmplitude": 0.75, "replayGain": -3.0},
                        "trackAudioNormalizationData": {"peakAmplitude": 0.5, "replayGain": -2.0},
                    }}}),
                )
                .unwrap(),
            })
            .unwrap();
        bounded(fixture.player.play(seed.clone())).await.unwrap();
        let items = fixture.items.lock().unwrap();
        let item = &items[0];
        let input = item.media_input();
        let expected_formats = if formats == json!(["FLAC", "FUTURE_FORMAT", "AACLC"]) {
            vec![player::AudioFormat::Flac, player::AudioFormat::AacLc]
        } else {
            Vec::new()
        };
        assert_eq!(input.formats, expected_formats);
        assert_eq!(
            input.album_normalization_data,
            Some(player::NormalizationData {
                peak_amplitude: Some(0.75),
                replay_gain: Some(-3.0)
            })
        );
        assert_eq!(
            input.track_normalization_data,
            Some(player::NormalizationData {
                peak_amplitude: Some(0.5),
                replay_gain: Some(-2.0)
            })
        );
        assert!(input.drm.is_none());
    }
}

#[tokio::test]
async fn superseded_reset_completes_successfully() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    let first = poll_command(player.reset());
    let second = poll_command(player.reset());
    bounded(first).await;
    bounded(second).await;
    assert_eq!(*calls.lock().unwrap(), ["reset"]);
    assert_eq!(
        bounded(events.recv()).await,
        Some(player::PlayerEvent::CurrentItemChanged {
            item: None,
            state: player::PlayerState::Idle,
        })
    );
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
}

#[tokio::test]
async fn dropping_reset_wait_still_clears_playback_and_reports_idle() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    drop(poll_command(player.reset()));
    assert_eq!(
        bounded(events.recv()).await,
        Some(player::PlayerEvent::CurrentItemChanged {
            item: None,
            state: player::PlayerState::Idle,
        })
    );
    assert_eq!(*calls.lock().unwrap(), ["reset"]);
}

#[tokio::test]
async fn load_superseding_reset_does_not_report_idle() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    let reset = poll_command(player.reset());
    let (_finish, seed) = pending();
    let load = poll_command(player.play(seed.clone()));
    bounded(reset).await;
    wait_for_call(&calls, "reset").await;
    assert!(matches!(
        bounded(events.recv()).await,
        Some(player::PlayerEvent::StateChanged {
            state: player::PlayerState::Preparing,
            ..
        })
    ));
    drop(load);
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
}

#[tokio::test]
async fn superseded_load_does_not_report_preparing() {
    let (delegate, mut events) = player_events::record();
    let (player, calls) = player_with_events(false, delegate);
    let old_seed = ready(Ok(input()));
    let old = poll_command(player.play(old_seed.clone()));
    let (_finish, seed) = pending();
    let product = MediaProduct::Track { id: "track".into() };
    let replacement = poll_command(player.play(seed.clone()));
    bounded(old).await.unwrap();
    assert_eq!(
        bounded(events.recv()).await,
        Some(player::PlayerEvent::StateChanged {
            item: Some(product),
            state: player::PlayerState::Preparing,
        })
    );
    assert_eq!(*calls.lock().unwrap(), ["reset"]);
    drop(replacement);
    drop(player);
    assert!(bounded(events.recv()).await.is_none());
}
