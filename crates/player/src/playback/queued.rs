use std::sync::Arc;

use channel::runtime::BoxFuture;
use channel::{
    event_channel, CommandHandler, EventHandler, EventReceiver, EventRunner, EventSender,
};

use super::contract::{CommandHandlerFactory, PlaybackCommand, PlaybackControllerEvents};
use super::media_player_runner::{self as media, MediaPlayerRunner};
use crate::model::{Error, MediaTime};
use crate::platforms::media::{MediaEvent, MediaPlayerEvent, MediaPlayerFactory};
use crate::player_item::PlayerItem;
use crate::reporting::item_event::PlaybackRequestReason;
use web_time::SystemTime;

pub(crate) struct QueuedPlaybackControllerFactory {
    pub(crate) media_factory: Arc<dyn MediaPlayerFactory>,
}

impl CommandHandlerFactory for QueuedPlaybackControllerFactory {
    fn create(
        &self,
    ) -> (
        Box<dyn CommandHandler<Message = PlaybackCommand>>,
        EventReceiver<PlaybackControllerEvents>,
    ) {
        let (controller, events) = QueuedPlaybackController::new(self.media_factory.as_ref());
        (Box::new(controller), events)
    }
}

impl CommandHandler for QueuedPlaybackController {
    type Message = PlaybackCommand;

    fn on_message(&mut self, message: PlaybackCommand) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match message {
                PlaybackCommand::Reset(command) => command.reply.complete(self.reset().await),
                PlaybackCommand::SetCurrentAndPlay(command) => command
                    .reply
                    .complete(self.set_current_and_play(command.message.0).await),
                PlaybackCommand::SetNext(command) => command
                    .reply
                    .complete(self.set_next(command.message.0).await),
                PlaybackCommand::Resume(command) => command.reply.complete(self.resume().await),
                PlaybackCommand::Pause(command) => command.reply.complete(self.pause().await),
                PlaybackCommand::Seek(command) => {
                    command.reply.complete(self.seek(command.message.0).await)
                }
            }
        })
    }
}

impl EventHandler for MediaEvents {
    type IncomingEvent = MediaEvent;

    fn on_reset(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.previous_item_ended_at = None;
            self.events.reset().await;
        })
    }

    fn on_event(&mut self, event: MediaEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match event.event {
                MediaPlayerEvent::Current { item: Some(item) } => {
                    let Some(item) = PlayerItem::from_media(&item) else {
                        return;
                    };
                    if let Some(timestamp) = self.previous_item_ended_at.take() {
                        item.request_playback(timestamp, PlaybackRequestReason::Implicit);
                    }
                    let _ = self
                        .events
                        .send(PlaybackControllerEvents::CurrentItemChanged(item));
                }
                MediaPlayerEvent::Current { item: None } => {
                    self.previous_item_ended_at = None;
                    let _ = self.events.send(PlaybackControllerEvents::Cleared);
                }
                MediaPlayerEvent::Completed => {
                    self.previous_item_ended_at = None;
                    let _ = self.events.send(PlaybackControllerEvents::Completed);
                }
                MediaPlayerEvent::Ended(_, _) => {
                    self.previous_item_ended_at.get_or_insert(event.emitted_at);
                }
                _ => {}
            }
        })
    }
}

pub(crate) struct QueuedPlaybackController {
    media: MediaPlayerRunner,
    _event_runner: EventRunner,
}

impl QueuedPlaybackController {
    pub(crate) fn new(
        factory: &dyn MediaPlayerFactory,
    ) -> (Self, EventReceiver<PlaybackControllerEvents>) {
        let (media, media_events) = MediaPlayerRunner::new(factory);
        let (events, receiver) = event_channel();
        let event_runner = EventRunner::new(
            Box::new(MediaEvents {
                events,
                previous_item_ended_at: None,
            }),
            media_events,
        );
        (
            Self {
                media,
                _event_runner: event_runner,
            },
            receiver,
        )
    }

    async fn set_current_and_play(&mut self, item: Arc<PlayerItem>) -> Result<(), Error> {
        self.media
            .commands
            .request(media::SetCurrentAndPlay(item))
            .await
            .unwrap_or(Ok(()))
            .map_err(Error::from)
    }

    async fn set_next(&mut self, item: Option<Arc<PlayerItem>>) -> Result<(), Error> {
        self.media
            .commands
            .request(media::SetNext(item))
            .await
            .unwrap_or(Ok(()))
            .map_err(Error::from)
    }

    async fn resume(&mut self) {
        let _ = self.media.commands.request(media::Resume).await;
    }

    async fn pause(&mut self) {
        let _ = self.media.commands.request(media::Pause).await;
    }

    async fn seek(&mut self, position: MediaTime) -> Result<(), Error> {
        self.media
            .commands
            .request(media::Seek(position))
            .await
            .transpose()?;
        Ok(())
    }

    async fn reset(&mut self) {
        let _ = self.media.commands.reset_and_request(media::Reset).await;
        self.media.events.reset().await;
    }
}

struct MediaEvents {
    events: EventSender<PlaybackControllerEvents>,
    // Current identifies the successor after the outgoing item's Ended observation.
    previous_item_ended_at: Option<SystemTime>,
}
