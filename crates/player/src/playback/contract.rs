use std::sync::Arc;

use channel::{CommandHandler, EventReceiver, Request};

use crate::model::{Error, MediaTime};
use crate::player_item::PlayerItem;

pub(crate) trait CommandHandlerFactory {
    fn create(
        &self,
    ) -> (
        Box<dyn CommandHandler<Message = PlaybackCommand>>,
        EventReceiver<PlaybackControllerEvents>,
    );
}

#[derive(derive_more::From)]
pub(crate) enum PlaybackCommand {
    Reset(channel::Command<Reset>),
    SetCurrentAndPlay(channel::Command<SetCurrentAndPlay>),
    SetNext(channel::Command<SetNext>),
    Resume(channel::Command<Resume>),
    Pause(channel::Command<Pause>),
    Seek(channel::Command<Seek>),
}

#[derive(Clone)]
pub(crate) enum PlaybackControllerEvents {
    CurrentItemChanged(Arc<PlayerItem>),
    Cleared,
    Completed,
}

pub(crate) struct Reset;

impl Request for Reset {
    type Response = ();
}

pub(crate) struct SetCurrentAndPlay(pub(crate) Arc<PlayerItem>);

impl Request for SetCurrentAndPlay {
    type Response = Result<(), Error>;
}

pub(crate) struct SetNext(pub(crate) Option<Arc<PlayerItem>>);

impl Request for SetNext {
    type Response = Result<(), Error>;
}

pub(crate) struct Resume;

impl Request for Resume {
    type Response = ();
}

pub(crate) struct Pause;

impl Request for Pause {
    type Response = ();
}

pub(crate) struct Seek(pub(crate) MediaTime);

impl Request for Seek {
    type Response = Result<(), Error>;
}
