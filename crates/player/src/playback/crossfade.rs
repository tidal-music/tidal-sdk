use channel::runtime::BoxFuture;
use std::sync::Arc;
use std::time::Duration;

use channel::{event_channel, CommandHandler, EventReceiver, EventSender};

use super::contract::{CommandHandlerFactory, PlaybackCommand, PlaybackControllerEvents};
use super::media_player_runner::{self as media, MediaPlayerRunner};
use crate::model::{Error, MediaTime};
use crate::platforms::media::{
    MediaEvent, MediaItem, MediaPlayerEvent, MediaPlayerFactory, PlaybackPosition,
};
use crate::player_item::PlayerItem;
use crate::reporting::item_event::PlaybackRequestReason;
use web_time::SystemTime;

type Result = std::result::Result<(), Error>;

pub(crate) struct CrossfadePlaybackControllerFactory {
    pub(crate) media_factory: Arc<dyn MediaPlayerFactory>,
    pub(crate) overlap: Duration,
}

impl CommandHandlerFactory for CrossfadePlaybackControllerFactory {
    fn create(
        &self,
    ) -> (
        Box<dyn CommandHandler<Message = PlaybackCommand>>,
        EventReceiver<PlaybackControllerEvents>,
    ) {
        let (controller, events) =
            CrossfadePlaybackController::new(self.media_factory.clone(), self.overlap);

        (Box::new(controller), events)
    }
}

pub(crate) struct CrossfadePlaybackController {
    current: PlayerSlot,
    next: PlayerSlot,
    ramp: VolumeRamp,
    events: EventSender<PlaybackControllerEvents>,
}

impl CrossfadePlaybackController {
    pub(crate) fn new(
        factory: Arc<dyn MediaPlayerFactory>,
        overlap: Duration,
    ) -> (Self, EventReceiver<PlaybackControllerEvents>) {
        let (events, receiver) = event_channel();
        (
            Self {
                current: PlayerSlot::new(factory.as_ref()),
                next: PlayerSlot::new(factory.as_ref()),
                ramp: VolumeRamp::new(overlap),
                events,
            },
            receiver,
        )
    }

    async fn set_current_and_play(&mut self, item: Arc<PlayerItem>) -> Result {
        self.current.set_current_and_play(item).await
    }

    async fn set_next(&mut self, item: Option<Arc<PlayerItem>>) -> Result {
        match item {
            None => self.next.reset().await,
            Some(item) if self.current.item.is_none() => self.current.set_current(item).await?,
            Some(item) => {
                self.next.pause().await;
                self.next.set_current(item).await?;
            }
        }

        Ok(())
    }

    async fn pause(&mut self) {
        self.current.pause().await;
    }

    async fn resume(&mut self) {
        self.current.resume().await;
    }

    async fn seek(&mut self, position: MediaTime) -> Result {
        self.current
            .media
            .commands
            .request(media::Seek(position))
            .await
            .transpose()?;
        Ok(())
    }

    async fn reset(&mut self) {
        self.current.reset().await;
        self.next.reset().await;
        self.events.reset().await;
    }
}

impl CommandHandler for CrossfadePlaybackController {
    type Message = PlaybackCommand;

    fn on_message(&mut self, command: PlaybackCommand) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match command {
                PlaybackCommand::Reset(command) => command.reply.complete(self.reset().await),
                PlaybackCommand::SetCurrentAndPlay(command) => command
                    .reply
                    .complete(self.set_current_and_play(command.message.0).await),

                PlaybackCommand::SetNext(command) => command
                    .reply
                    .complete(self.set_next(command.message.0).await),
                PlaybackCommand::Pause(command) => command.reply.complete(self.pause().await),
                PlaybackCommand::Resume(command) => command.reply.complete(self.resume().await),
                PlaybackCommand::Seek(command) => {
                    command.reply.complete(self.seek(command.message.0).await)
                }
            }
        })
    }

    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            tokio::select! {
                Some(event) = self.next.events.recv() => self.on_next_event(event),
                event = self.current.events.recv() => match event {
                    Some(event) => self.on_current_event(event),
                    None => std::future::pending::<()>().await,
                },
            }
        })
    }
}

struct PlayerSlot {
    media: MediaPlayerRunner,
    events: EventReceiver<MediaEvent>,
    item: Option<Arc<PlayerItem>>,
}

impl PlayerSlot {
    fn new(factory: &dyn MediaPlayerFactory) -> Self {
        let (media, events) = MediaPlayerRunner::new(factory);
        Self {
            media,
            events,
            item: None,
        }
    }

    async fn reset(&mut self) {
        let _ = self.media.commands.reset_and_request(media::Reset).await;
        self.events.clear();
        self.item = None;
    }

    async fn set_current_and_play(&mut self, item: Arc<PlayerItem>) -> Result {
        self.media
            .commands
            .request(media::SetCurrentAndPlay(item.clone()))
            .await
            .transpose()?
            .inspect(|_| self.item = Some(item));

        Ok(())
    }

    async fn set_current(&mut self, item: Arc<PlayerItem>) -> Result {
        self.media
            .commands
            .request(media::SetCurrent(item.clone()))
            .await
            .transpose()?
            .inspect(|_| self.item = Some(item));

        Ok(())
    }

    async fn pause(&self) {
        let _ = self.media.commands.request(media::Pause).await;
    }

    fn set_volume(&self, ramp_gain: f32) {
        let volume = self.item.as_ref().map_or(1.0, |item| item.volume()) * ramp_gain;
        let _ = self.media.commands.enqueue(media::SetVolume(volume));
    }

    async fn resume(&self) {
        let _ = self.media.commands.request(media::Resume).await;
    }
}

impl CrossfadePlaybackController {
    fn swap_player_slots(&mut self) {
        std::mem::swap(&mut self.current, &mut self.next);
        self.current.set_volume(1.0);

        if let Some(item) = &self.current.item {
            let _ = self
                .events
                .send(PlaybackControllerEvents::CurrentItemChanged(item.clone()));
        }
    }

    fn on_current_event(&mut self, event: MediaEvent) {
        let event_is_for_current_item = event
            .event
            .item()
            .and_then(PlayerItem::from_media)
            .zip(self.current.item.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(&a, b));

        if !event_is_for_current_item {
            return;
        }

        match &event.event {
            MediaPlayerEvent::Position(_, position) => {
                self.on_position(*position, event.emitted_at)
            }
            MediaPlayerEvent::Seeked(_, position) => self.on_seeked(*position),
            MediaPlayerEvent::Paused(_, _) | MediaPlayerEvent::Failed(_, _, _) => {
                let _ = self.next.media.commands.enqueue(media::Pause);
            }
            MediaPlayerEvent::Current { item } => self.on_current(item.clone()),
            MediaPlayerEvent::Resumed(_, position) => self.on_position(*position, event.emitted_at),
            MediaPlayerEvent::Ended(_, _) => self.on_ended(event.emitted_at),
            _ => {}
        }
    }

    fn on_next_event(&mut self, event: MediaEvent) {
        let event_is_for_current_item = event
            .event
            .item()
            .and_then(PlayerItem::from_media)
            .zip(self.next.item.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(&a, b));

        if !event_is_for_current_item {
            return;
        }

        if let MediaPlayerEvent::Ended(_, _) = event.event {
            self.next.item = None;
        }
    }

    fn on_position(&mut self, position: PlaybackPosition, timestamp: SystemTime) {
        let (current, next) = self.ramp.gains(
            self.current.item.as_deref(),
            self.next.item.as_deref(),
            position.remaining,
        );

        self.current.set_volume(current);
        self.next.set_volume(next);

        let is_within_fade = self.ramp.is_within_fade(
            self.current.item.as_deref(),
            self.next.item.as_deref(),
            position.remaining,
        );

        if is_within_fade {
            let _ = self.next.item.as_ref().inspect(|item| {
                item.request_playback(timestamp, PlaybackRequestReason::Implicit);
            });

            let _ = self.next.media.commands.enqueue(media::Resume);
        }
    }

    fn on_seeked(&mut self, position: PlaybackPosition) {
        let is_within_fade = self.ramp.is_within_fade(
            self.current.item.as_deref(),
            self.next.item.as_deref(),
            position.remaining,
        );

        if !is_within_fade {
            let _ = self.next.media.commands.enqueue(media::Pause);
            let _ = self
                .next
                .media
                .commands
                .enqueue(media::Seek(Duration::ZERO));
            return;
        }

        let overlap = self
            .ramp
            .effective_overlap(self.current.item.as_deref(), self.next.item.as_deref())
            .unwrap();

        let _ = self
            .next
            .media
            .commands
            .enqueue(media::Seek(overlap - position.remaining));
    }

    fn on_current(&mut self, item: Option<Arc<dyn MediaItem>>) {
        let event = match item {
            Some(item) => match PlayerItem::from_media(&item) {
                Some(item) => PlaybackControllerEvents::CurrentItemChanged(item),
                None => return,
            },
            None => PlaybackControllerEvents::Cleared,
        };
        let _ = self.events.send(event);
    }

    fn on_ended(&mut self, timestamp: SystemTime) {
        if let Some(item) = &self.next.item {
            item.request_playback(timestamp, PlaybackRequestReason::Implicit);
        }
        self.current.item = None;
        self.swap_player_slots();

        if self.current.item.is_none() {
            let _ = self.events.send(PlaybackControllerEvents::Completed);
        }

        let _ = self.current.media.commands.enqueue(media::Resume);
    }
}

struct VolumeRamp {
    desired_overlap: Duration,
}

impl VolumeRamp {
    fn new(desired_overlap: Duration) -> Self {
        Self { desired_overlap }
    }

    fn effective_overlap(
        &self,
        current: Option<&PlayerItem>,
        next: Option<&PlayerItem>,
    ) -> Option<Duration> {
        let current = current?.duration()?;
        let next = next?.duration()?;
        let overlap = self.desired_overlap.min(current.min(next) / 2);
        (!overlap.is_zero()).then_some(overlap)
    }

    fn is_within_fade(
        &self,
        current: Option<&PlayerItem>,
        next: Option<&PlayerItem>,
        remaining: Duration,
    ) -> bool {
        self.effective_overlap(current, next)
            .is_some_and(|overlap| remaining <= overlap)
    }

    fn gains(
        &self,
        current: Option<&PlayerItem>,
        next: Option<&PlayerItem>,
        remaining: Duration,
    ) -> (f32, f32) {
        let Some(overlap) = self.effective_overlap(current, next) else {
            return (1.0, 1.0);
        };

        let current = (remaining.as_secs_f64() / overlap.as_secs_f64()).clamp(0.0, 1.0) as f32;
        (current, 1.0 - current)
    }
}
