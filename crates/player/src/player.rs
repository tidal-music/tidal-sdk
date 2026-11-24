use std::sync::{Arc, Mutex, Weak};
use web_time::SystemTime;

use crate::backend::http_client::HttpClient;
use crate::backend::license_fetcher::LicenseFetcher;
use crate::backend::manifest_fetcher::ManifestFetcher;
use crate::backend::tidal_api_client::TidalApiClient;
use crate::loading::loader::{self, Loader};
use crate::loading::PlaybackSeed;
use crate::model::error::{other, ErrorSegment};
use crate::model::{
    CrossfadeConfig, Error, ManifestConfiguration, MediaProduct, MediaTime, PlayerConfig,
    PlayerEvent, PlayerState,
};
use crate::platforms::player_events::PlayerEvents;
use crate::platforms::{
    Authenticator, DrmProvider, HttpTransport, MediaPlayerFactory, TidalEventSender,
};
use crate::playback::{
    CommandHandlerFactory, CrossfadePlaybackControllerFactory, Pause, PlaybackCommand,
    PlaybackControllerEvents, QueuedPlaybackControllerFactory, Reset, Resume, Seek,
    SetCurrentAndPlay, SetNext,
};
use crate::player_item::{PlayerItem, PlayerItemEvents, PlayerItemState};
use crate::reporting::item_event;
use channel::runtime::BoxFuture;
use channel::{
    event_channel, Command, CommandHandler, CommandRunner, CommandSender, EventRunner, EventSender,
    Request,
};
use channel::{EventHandler, EventReceiver};

#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct Player {
    loader: Loader,
    manifest_fetcher: Arc<ManifestFetcher>,
    commands: CommandSender<InternalPlaybackCommand>,
    _events: EventRunner,
    _command_runner: CommandRunner,
    _event_dispatch: EventRunner,
}

impl Player {
    /// Plain Rust callers construct the player within a Tokio runtime.
    pub fn new(config: PlayerConfig, dependencies: PlayerDependencies) -> Self {
        let media_factory = dependencies.media_factory.clone();
        let controller_factory: Box<dyn CommandHandlerFactory> = match config.crossfade {
            None => Box::new(QueuedPlaybackControllerFactory { media_factory }),
            Some(CrossfadeConfig { overlap }) => Box::new(CrossfadePlaybackControllerFactory {
                media_factory,
                overlap,
            }),
        };

        let manifest_fetcher = Arc::new(ManifestFetcher::new(
            Arc::new(TidalApiClient::new(
                HttpClient::new(
                    dependencies.http_transport.clone(),
                    Some(dependencies.authenticator.clone()),
                ),
                config.base_url,
            )),
            config.manifest_configuration,
        ));
        let license_fetcher = Arc::new(LicenseFetcher::new(
            dependencies.http_transport,
            dependencies.authenticator,
        ));

        let (controller_handler, playback_events) = controller_factory.create();
        let (event_sender, events) = event_channel();
        let current_item = Arc::new(Mutex::new(None));
        let processor = PlaybackControllerEventHandler {
            current_item: current_item.clone(),
            player_event_sender: event_sender.clone(),
            item_events: None,
        };
        let event_runner = EventRunner::new(Box::new(processor), playback_events);
        let event_dispatch = EventRunner::new(
            Box::new(PlayerEventDispatcher {
                events: dependencies.player_events,
            }),
            events,
        );

        let (controller_commands, controller_runner) = CommandRunner::new(controller_handler, 32);
        let handler = InternalCommandHandler {
            current_item,
            player_events: event_sender,
            event_sender: dependencies.event_sender.clone(),
            license_fetcher: license_fetcher.clone(),
            controller_commands,
            _controller_runner: controller_runner,
        };
        let (commands, command_runner) = CommandRunner::new(Box::new(handler), 32);

        Self {
            loader: Loader::new(
                manifest_fetcher.clone(),
                license_fetcher,
                dependencies.drm_provider,
                dependencies.event_sender,
            ),
            manifest_fetcher,
            commands,
            _events: event_runner,
            _command_runner: command_runner,
            _event_dispatch: event_dispatch,
        }
    }
}

#[cfg_attr(feature = "uniffi", uniffi::export)]
impl Player {
    pub fn get_manifest_configuration(&self) -> ManifestConfiguration {
        self.manifest_fetcher.get_configuration()
    }

    pub fn set_manifest_configuration(&self, configuration: ManifestConfiguration) {
        self.manifest_fetcher.set_configuration(configuration);
    }

    pub fn prepare(&self, product: MediaProduct) -> Arc<PlaybackSeed> {
        self.loader.prepare(product)
    }

    /// Return loading failures and report them through StateChanged(Failed).
    pub async fn play(&self, seed: Arc<PlaybackSeed>) -> Result<(), Error> {
        let requested_at = SystemTime::now();
        self.commands
            .reset_and_request(Play { seed, requested_at })
            .await
            .unwrap_or(Ok(()))
    }

    pub async fn pause(&self) {
        let _ = self.commands.request(Pause).await;
    }

    pub async fn resume(&self) {
        let requested_at = SystemTime::now();
        let _ = self.commands.request(ResumePlayback(requested_at)).await;
    }

    pub async fn seek(&self, position: MediaTime) -> Result<(), Error> {
        self.commands
            .request(Seek(position))
            .await
            .unwrap_or(Ok(()))
    }

    pub async fn reset(&self) {
        let _ = self.commands.reset_and_request(Reset).await;
    }

    pub async fn set_next(&self, seed: Option<Arc<PlaybackSeed>>) -> Result<(), Error> {
        self.commands
            .request(LoadSeedAndSetNext(seed))
            .await
            .unwrap_or(Ok(()))
    }
}

struct PlayerEventDispatcher {
    events: Arc<dyn PlayerEvents>,
}

impl EventHandler for PlayerEventDispatcher {
    type IncomingEvent = PlayerEvent;

    fn on_event(&mut self, event: PlayerEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events.on_event(event);
        })
    }
}

pub struct PlayerDependencies {
    pub player_events: Arc<dyn PlayerEvents>,
    pub media_factory: Arc<dyn MediaPlayerFactory>,
    pub http_transport: Arc<dyn HttpTransport>,
    pub authenticator: Arc<dyn Authenticator>,
    /// License preloading; the media player acquires licenses when it does not preload.
    pub drm_provider: Arc<dyn DrmProvider>,
    pub event_sender: Arc<dyn TidalEventSender>,
}
struct PlaybackControllerEventHandler {
    current_item: Arc<Mutex<Option<Weak<PlayerItem>>>>,
    player_event_sender: EventSender<PlayerEvent>,
    item_events: Option<(MediaProduct, EventReceiver<PlayerItemEvents>)>,
}

impl EventHandler for PlaybackControllerEventHandler {
    type IncomingEvent = PlaybackControllerEvents;

    fn on_reset(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            *self.current_item.lock().unwrap() = None;
            self.item_events = None;
        })
    }

    fn on_event(&mut self, event: PlaybackControllerEvents) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match event {
                PlaybackControllerEvents::CurrentItemChanged(item) => {
                    self.handle_current_item_changed(item)
                }
                PlaybackControllerEvents::Cleared => self.handle_playback_cleared(),
                PlaybackControllerEvents::Completed => self.handle_playback_completed(),
            }
        })
    }

    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some((product, receiver)) = &mut self.item_events else {
                return std::future::pending().await;
            };
            match receiver.recv().await {
                Some(change) => {
                    if let Some(event) = change.into_player_event(product.clone()) {
                        let _ = self.player_event_sender.send(event);
                    }
                }
                None => self.item_events = None,
            }
        })
    }
}

impl PlaybackControllerEventHandler {
    fn handle_current_item_changed(&mut self, item: Arc<PlayerItem>) {
        *self.current_item.lock().unwrap() = Some(Arc::downgrade(&item));
        self.item_events = None;
        let product = item.media_product();
        let receiver = item.subscribe();
        let _ = self
            .player_event_sender
            .send(PlayerEvent::CurrentItemChanged {
                item: Some(product.clone()),
                state: PlayerState::from(item.state()),
            });
        self.item_events = Some((product, receiver));
    }

    fn handle_playback_cleared(&mut self) {
        *self.current_item.lock().unwrap() = None;
        self.item_events = None;
        let _ = self
            .player_event_sender
            .send(PlayerEvent::CurrentItemChanged {
                item: None,
                state: PlayerState::Idle,
            });
    }

    fn handle_playback_completed(&mut self) {
        *self.current_item.lock().unwrap() = None;
        self.item_events = None;
        let _ = self
            .player_event_sender
            .send(PlayerEvent::CurrentItemChanged {
                item: None,
                state: PlayerState::Completed,
            });
    }
}

#[derive(Debug, thiserror::Error)]
#[error("The selected playback item has already ended")]
struct EndedItemSelected;

impl From<EndedItemSelected> for Error {
    fn from(error: EndedItemSelected) -> Self {
        ErrorSegment::PlaybackError.wrap(other(error, "eis"))
    }
}

impl From<PlayerItemState> for PlayerState {
    fn from(state: PlayerItemState) -> Self {
        use PlayerItemState as State;
        match state {
            State::Preparing => Self::Preparing,
            State::Playing => Self::Playing,
            State::Paused => Self::Paused,
            State::Stalled => Self::Stalled,
            State::Failed(error) => Self::Failed(error),
            State::Ended => Self::Failed(EndedItemSelected.into()),
        }
    }
}

impl PlayerItemEvents {
    fn into_player_event(self, item: MediaProduct) -> Option<PlayerEvent> {
        Some(match self {
            // Natural completion is coordinated by the controller, not a selection failure.
            Self::StateChanged(PlayerItemState::Ended) => return None,
            Self::StateChanged(state) => PlayerEvent::StateChanged {
                item: Some(item),
                state: PlayerState::from(state),
            },
            Self::PositionChanged { position, duration } => PlayerEvent::PositionChanged {
                item,
                position,
                duration,
            },
        })
    }
}

struct InternalCommandHandler {
    // Shared with observations; Weak preserves media-owned item lifetimes.
    current_item: Arc<Mutex<Option<Weak<PlayerItem>>>>,
    event_sender: Arc<dyn TidalEventSender>,
    license_fetcher: Arc<LicenseFetcher>,
    player_events: EventSender<PlayerEvent>,
    controller_commands: CommandSender<PlaybackCommand>,
    _controller_runner: CommandRunner,
}

#[derive(derive_more::From)]
enum InternalPlaybackCommand {
    #[from]
    Reset(Command<Reset>),
    #[from]
    Play(Command<Play>),
    #[from]
    SetNext(Command<LoadSeedAndSetNext>),
    #[from]
    Resume(Command<ResumePlayback>),
    #[from(Command<Pause>, Command<Seek>)]
    Passthrough(PlaybackCommand),
}

struct ResumePlayback(SystemTime);

impl Request for ResumePlayback {
    type Response = ();
}

struct Play {
    seed: Arc<PlaybackSeed>,
    requested_at: SystemTime,
}

impl Request for Play {
    type Response = Result<(), Error>;
}

struct LoadSeedAndSetNext(Option<Arc<PlaybackSeed>>);

impl Request for LoadSeedAndSetNext {
    type Response = Result<(), Error>;
}

impl CommandHandler for InternalCommandHandler {
    type Message = InternalPlaybackCommand;

    fn on_message(&mut self, command: InternalPlaybackCommand) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let command = match command {
                InternalPlaybackCommand::Reset(command) => {
                    self.internal_reset(PlayerState::Idle, None).await;
                    command.reply.complete(());
                    None
                }
                InternalPlaybackCommand::Play(command) => {
                    command.reply.complete(
                        self.play(command.message.seed, command.message.requested_at)
                            .await,
                    );
                    None
                }
                InternalPlaybackCommand::SetNext(command) => {
                    self.set_next(command).await.map(PlaybackCommand::SetNext)
                }
                InternalPlaybackCommand::Resume(command) => self
                    .create_resume_command(command)
                    .map(PlaybackCommand::Resume),
                InternalPlaybackCommand::Passthrough(command) => Some(command),
            };

            if let Some(command) = command {
                let _ = self.controller_commands.send(command).await;
            }
        })
    }
}

impl InternalCommandHandler {
    async fn internal_reset(&mut self, state: PlayerState, item: Option<MediaProduct>) {
        let _ = self.controller_commands.reset_and_request(Reset).await;
        *self.current_item.lock().unwrap() = None;
        let event = match state {
            PlayerState::Idle => PlayerEvent::CurrentItemChanged { item, state },
            _ => PlayerEvent::StateChanged { item, state },
        };
        let _ = self.player_events.send(event);
    }

    async fn play(
        &mut self,
        seed: Arc<PlaybackSeed>,
        requested_at: SystemTime,
    ) -> Result<(), Error> {
        let product = seed.product.clone();
        self.internal_reset(PlayerState::Preparing, Some(product.clone()))
            .await;
        let result = match self.item(seed, Some(requested_at)).await {
            Ok(Some(item)) => {
                *self.current_item.lock().unwrap() = Some(Arc::downgrade(&item));
                let result = self
                    .controller_commands
                    .request(SetCurrentAndPlay(item.clone()))
                    .await
                    .unwrap_or(Ok(()));
                if let Err(error) = &result {
                    item.record_error(error.clone());
                }
                result
            }
            Ok(None) => return Ok(()),
            Err(error) => Err(error),
        };

        if let Err(error) = &result {
            let _ = self.player_events.send(PlayerEvent::StateChanged {
                item: Some(product),
                state: PlayerState::Failed(error.clone()),
            });
        }
        result
    }

    async fn set_next(&mut self, command: Command<LoadSeedAndSetNext>) -> Option<Command<SetNext>> {
        let Some(seed) = command.message.0 else {
            return Some(Command {
                message: SetNext(None),
                reply: command.reply,
            });
        };

        let result = match self.item(seed, None).await {
            Ok(Some(item)) => {
                let result = self
                    .controller_commands
                    .request(SetNext(Some(item.clone())))
                    .await
                    .unwrap_or(Ok(()));
                if let Err(error) = &result {
                    item.record_error(error.clone());
                }
                result
            }
            Ok(None) => return None,
            Err(error) => Err(error),
        };
        command.reply.complete(result);
        None
    }

    fn create_resume_command(&self, command: Command<ResumePlayback>) -> Option<Command<Resume>> {
        let item = self
            .current_item
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(item) = item {
            item.request_playback(
                command.message.0,
                item_event::PlaybackRequestReason::Explicit,
            );
        }
        Some(Command {
            message: Resume,
            reply: command.reply,
        })
    }

    async fn item(
        &mut self,
        seed: Arc<PlaybackSeed>,
        requested_at: Option<SystemTime>,
    ) -> Result<Option<Arc<PlayerItem>>, Error> {
        let product = seed.product.clone();
        let data = match seed.resolve().await {
            Ok(data) => data,
            Err(loader::Error::Cancelled) => return Ok(None),
            Err(loader::Error::ManifestFetcher(error)) => return Err(Error::from(error.as_ref())),
            Err(loader::Error::License(error)) => return Err(error),
        };
        let item = Arc::new(PlayerItem::new(
            product,
            data.input,
            data.drm,
            self.license_fetcher.clone(),
            data.license_session,
            data.events,
            self.event_sender.clone(),
        ));
        if let Some(timestamp) = requested_at {
            item.request_playback(timestamp, item_event::PlaybackRequestReason::Explicit);
        }
        Ok(Some(item))
    }
}
