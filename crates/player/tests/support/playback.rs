//! Player setup and playback seeds whose preparation the test controls.
use std::sync::Arc;

use player::{
    Authenticator, AuthenticatorError, HttpRequest, HttpResponse, HttpTransport, MediaInput,
    MediaPlayerFactory, MediaProduct, PlaybackSeed, Player, PlayerConfig, PlayerDependencies,
    PlayerEvents, ReportingEvent, TidalEventSender, TransportError,
};
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{drm_provider, media, player_events};

pub struct Input {
    pub id: Uuid,
}

pub fn input() -> Input {
    Input { id: Uuid::new_v4() }
}

pub fn resource_id(input: &MediaInput) -> &str {
    input.url.strip_prefix("https://media.example/").unwrap()
}

/// Answers the manifest request with the input the test sends.
struct PreparationPlatform {
    response: tokio::sync::Mutex<Option<oneshot::Receiver<Result<Input, TransportError>>>>,
}

#[async_trait::async_trait]
impl HttpTransport for PreparationPlatform {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let response = self
            .response
            .lock()
            .await
            .take()
            .unwrap()
            .await
            .map_err(|_| TransportError::OtherError {
                description: "cancelled".into(),
            })??;
        Ok(HttpResponse {
            status_code: 200,
            body: serde_json::to_vec(
                &json!({"data": {"id": response.id.to_string(), "attributes": {
                    "uri": format!("https://media.example/{}", response.id)
                }}}),
            )
            .unwrap(),
        })
    }
}

#[async_trait::async_trait]
impl Authenticator for PreparationPlatform {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("token".into())
    }
    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("unexpected refresh")
    }
}

struct Ignored;

impl TidalEventSender for Ignored {
    fn send_event(&self, _: ReportingEvent) {}
}

/// Fails if the player needs the network: these tests supply prepared seeds.
struct Offline;

#[async_trait::async_trait]
impl HttpTransport for Offline {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        panic!("playback tests supply prepared seeds");
    }
}

pub fn player_config() -> PlayerConfig {
    PlayerConfig {
        base_url: "https://openapi.tidal.com/v2".into(),
        manifest_configuration: Default::default(),
        crossfade: None,
    }
}

pub fn dependencies(
    player_events: Arc<dyn PlayerEvents>,
    media_factory: Arc<dyn MediaPlayerFactory>,
) -> PlayerDependencies {
    PlayerDependencies {
        player_events,
        event_sender: Arc::new(Ignored),
        http_transport: Arc::new(Offline),
        authenticator: Arc::new(Token),
        drm_provider: Arc::new(drm_provider::NoopDrmProvider),
        media_factory,
    }
}

struct Token;

#[async_trait::async_trait]
impl Authenticator for Token {
    async fn access_token(&self) -> Result<String, AuthenticatorError> {
        Ok("token".into())
    }
    async fn refreshed_access_token(&self) -> Result<String, AuthenticatorError> {
        panic!("unexpected refresh")
    }
}

/// A seed whose preparation completes when the input is sent.
pub fn pending() -> (
    oneshot::Sender<Result<Input, TransportError>>,
    Arc<PlaybackSeed>,
) {
    let (sender, receiver) = oneshot::channel();
    let platform = Arc::new(PreparationPlatform {
        response: tokio::sync::Mutex::new(Some(receiver)),
    });
    let (media_factory, _media) = media::scripted(media::Script::default());
    let source = Player::new(
        player_config(),
        PlayerDependencies {
            player_events: Arc::new(player_events::Ignore),
            event_sender: Arc::new(Ignored),
            http_transport: platform.clone(),
            authenticator: platform,
            drm_provider: Arc::new(drm_provider::NoopDrmProvider),
            media_factory,
        },
    );
    (
        sender,
        source.prepare(MediaProduct::Track { id: "track".into() }),
    )
}

pub fn ready(input: Result<Input, TransportError>) -> Arc<PlaybackSeed> {
    let (sender, seed) = pending();
    assert!(sender.send(input).is_ok());
    seed
}

/// Returns the same manifest response to every request.
struct FixedResponse(HttpResponse);

#[async_trait::async_trait]
impl HttpTransport for FixedResponse {
    async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        Ok(self.0.clone())
    }
}

/// A seed prepared from this manifest response.
pub fn seed_from_response(response: HttpResponse) -> Arc<PlaybackSeed> {
    let (media_factory, _media) = media::scripted(media::Script::default());
    let mut dependencies = dependencies(Arc::new(player_events::Ignore), media_factory);
    dependencies.http_transport = Arc::new(FixedResponse(response));
    let source = Player::new(player_config(), dependencies);
    source.prepare(MediaProduct::Track { id: "track".into() })
}
