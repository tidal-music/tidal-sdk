use crate::model::MediaTime;
use crate::platforms::media::{
    Error, MediaEvent, MediaItem, MediaPlayer, MediaPlayerEvents, MediaPlayerFactory,
};
use crate::player_item::PlayerItem;
use channel::runtime::BoxFuture;
use channel::{
    event_channel, CommandHandler, CommandRunner, CommandSender, EventReceiver, EventSender,
};
use std::sync::Arc;
pub(crate) struct MediaPlayerRunner {
    pub(crate) commands: CommandSender<MediaCommand>,
    pub(crate) events: EventSender<MediaEvent>,
    _runner: CommandRunner,
}

impl MediaPlayerRunner {
    pub(crate) fn new(factory: &dyn MediaPlayerFactory) -> (Self, EventReceiver<MediaEvent>) {
        let (events, receiver) = event_channel();
        let player = factory.create(Arc::new(MediaEvents(events.clone())));
        let (commands, runner) = CommandRunner::new(Box::new(MediaCommands(player)), 32);
        (
            Self {
                commands,
                events,
                _runner: runner,
            },
            receiver,
        )
    }
}

struct MediaEvents(EventSender<MediaEvent>);
impl MediaPlayerEvents for MediaEvents {
    fn on_event(&self, event: MediaEvent) {
        PlayerItem::on_media_player_event(&event);
        let _ = self.0.send(event);
    }
}

struct MediaCommands(Arc<dyn MediaPlayer>);

impl Drop for MediaCommands {
    fn drop(&mut self) {
        self.0.dispose();
    }
}
impl CommandHandler for MediaCommands {
    type Message = MediaCommand;
    fn on_message(&mut self, message: MediaCommand) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match message {
                MediaCommand::Reset(c) => c.reply.complete(self.0.reset().await),
                MediaCommand::SetCurrentAndPlay(c) => c
                    .reply
                    .complete(self.0.set_current_and_play(c.message.0).await),
                MediaCommand::SetCurrent(c) => {
                    c.reply.complete(self.0.set_current(c.message.0).await)
                }
                MediaCommand::SetNext(c) => c.reply.complete(
                    self.0
                        .set_next(c.message.0.map(|item| item as Arc<dyn MediaItem>))
                        .await,
                ),
                MediaCommand::Pause(c) => {
                    self.0.pause().await;
                    c.reply.complete(());
                }
                MediaCommand::Resume(c) => {
                    self.0.resume().await;
                    c.reply.complete(());
                }
                MediaCommand::Seek(c) => c.reply.complete(self.0.seek(c.message.0).await),
                MediaCommand::SetVolume(c) => {
                    self.0.set_volume(c.message.0).await;
                    c.reply.complete(());
                }
            }
        })
    }
}

#[derive(derive_more::From)]
pub(crate) enum MediaCommand {
    Reset(channel::Command<Reset>),
    SetCurrentAndPlay(channel::Command<SetCurrentAndPlay>),
    SetCurrent(channel::Command<SetCurrent>),
    SetNext(channel::Command<SetNext>),
    Resume(channel::Command<Resume>),
    Pause(channel::Command<Pause>),
    Seek(channel::Command<Seek>),
    SetVolume(channel::Command<SetVolume>),
}

pub(crate) struct Reset;

impl channel::Request for Reset {
    type Response = ();
}

pub(crate) struct SetCurrentAndPlay(pub Arc<PlayerItem>);

impl channel::Request for SetCurrentAndPlay {
    type Response = Result<(), Error>;
}

pub(crate) struct SetCurrent(pub Arc<PlayerItem>);

impl channel::Request for SetCurrent {
    type Response = Result<(), Error>;
}

pub(crate) struct SetNext(pub Option<Arc<PlayerItem>>);

impl channel::Request for SetNext {
    type Response = Result<(), Error>;
}

pub(crate) struct Resume;

impl channel::Request for Resume {
    type Response = ();
}

pub(crate) struct Pause;

impl channel::Request for Pause {
    type Response = ();
}

pub(crate) struct Seek(pub MediaTime);

impl channel::Request for Seek {
    type Response = Result<(), Error>;
}

pub(crate) struct SetVolume(pub f32);

impl channel::Request for SetVolume {
    type Response = ();
}
