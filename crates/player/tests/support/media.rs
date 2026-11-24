//! Scripted host media players. Every call and its reply are handed to the test, which
//! completes them in the order it wants to observe.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use player::{
    MediaError, MediaEvent, MediaItem, MediaPlayer, MediaPlayerEvent, MediaPlayerEvents,
    MediaPlayerFactory, MediaTime,
};
use tokio::sync::{mpsc, oneshot};

/// A media call and the way to complete it.
pub struct Call<T, R> {
    pub input: T,
    pub reply: Reply<R>,
}

pub struct Reply<R>(oneshot::Sender<R>);

impl<R> Reply<R> {
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    pub fn complete(self, value: R) {
        let _ = self.0.send(value);
    }
}

pub enum MediaCall {
    SetCurrentAndPlay(Call<Arc<dyn MediaItem>, Result<(), MediaError>>),
    SetCurrent(Call<Arc<dyn MediaItem>, Result<(), MediaError>>),
    SetNext(Call<Option<Arc<dyn MediaItem>>, Result<(), MediaError>>),
    Pause(Call<(), ()>),
    Resume(Call<(), ()>),
    Seek(Call<MediaTime, Result<(), MediaError>>),
}

/// What the player did to one of the media instances it created.
pub enum Native {
    Command(usize, MediaCall),
    Reset(usize),
    Dropped(usize),
}

/// A reset that stays pending until the test releases it.
pub struct ResetGate {
    pub started: oneshot::Sender<()>,
    pub release: oneshot::Receiver<()>,
}

#[derive(Default)]
pub struct Script {
    /// Reports Duration and Current when an item is installed, as a media engine would.
    pub announce_installed: bool,
    pub duration: Option<Duration>,
}

/// The test's side of the media instances a player creates.
pub struct Media {
    pub native: mpsc::UnboundedReceiver<Native>,
    /// Event sinks, indexed by the order the player created the instances in.
    pub events: Arc<Mutex<Vec<Arc<dyn MediaPlayerEvents>>>>,
    /// Volume set on each instance, in order: item volume on installation, then set_volume
    /// calls, and full volume on reset.
    pub gains: Arc<Mutex<Vec<(usize, f32)>>>,
    pub reset_gate: Arc<Mutex<Option<ResetGate>>>,
}

impl Media {
    pub fn emit(&self, slot: usize, event: MediaPlayerEvent) {
        let sink = self.events.lock().unwrap()[slot].clone();
        sink.on_event(MediaEvent::new(event));
    }
}

pub fn scripted(script: Script) -> (Arc<dyn MediaPlayerFactory>, Media) {
    let (commands, native) = mpsc::unbounded_channel();
    let events = Arc::new(Mutex::new(Vec::new()));
    let gains = Arc::new(Mutex::new(Vec::new()));
    let reset_gate = Arc::new(Mutex::new(None));
    let factory = Factory {
        commands,
        events: events.clone(),
        gains: gains.clone(),
        reset_gate: reset_gate.clone(),
        script,
    };
    (
        Arc::new(factory),
        Media {
            native,
            events,
            gains,
            reset_gate,
        },
    )
}

struct Factory {
    commands: mpsc::UnboundedSender<Native>,
    events: Arc<Mutex<Vec<Arc<dyn MediaPlayerEvents>>>>,
    gains: Arc<Mutex<Vec<(usize, f32)>>>,
    reset_gate: Arc<Mutex<Option<ResetGate>>>,
    script: Script,
}

impl MediaPlayerFactory for Factory {
    fn create(&self, events: Arc<dyn MediaPlayerEvents>) -> Arc<dyn MediaPlayer> {
        let mut sinks = self.events.lock().unwrap();
        let slot = sinks.len();
        sinks.push(events.clone());
        Arc::new(Instance {
            slot,
            commands: self.commands.clone(),
            gains: self.gains.clone(),
            reset_gate: self.reset_gate.clone(),
            announce_installed: self.script.announce_installed,
            duration: self.script.duration,
            events,
        })
    }
}

struct Instance {
    slot: usize,
    commands: mpsc::UnboundedSender<Native>,
    gains: Arc<Mutex<Vec<(usize, f32)>>>,
    reset_gate: Arc<Mutex<Option<ResetGate>>>,
    announce_installed: bool,
    duration: Option<Duration>,
    events: Arc<dyn MediaPlayerEvents>,
}

impl Instance {
    async fn call<T, R>(
        &self,
        input: T,
        abandoned: R,
        command: impl FnOnce(Call<T, R>) -> MediaCall,
    ) -> R {
        let (reply, response) = oneshot::channel();
        let call = command(Call {
            input,
            reply: Reply(reply),
        });
        let _ = self.commands.send(Native::Command(self.slot, call));
        response.await.unwrap_or(abandoned)
    }

    fn install(&self, item: &Arc<dyn MediaItem>) {
        self.gains.lock().unwrap().push((self.slot, item.volume()));
        if !self.announce_installed {
            return;
        }
        if let Some(duration) = self.duration {
            self.events
                .on_event(MediaEvent::new(MediaPlayerEvent::Duration(
                    item.clone(),
                    duration,
                )));
        }
        self.events
            .on_event(MediaEvent::new(MediaPlayerEvent::Current {
                item: Some(item.clone()),
            }));
    }
}

#[async_trait::async_trait]
impl MediaPlayer for Instance {
    async fn set_current_and_play(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.install(&item);
        self.call(item, Ok(()), MediaCall::SetCurrentAndPlay).await
    }

    async fn set_current(&self, item: Arc<dyn MediaItem>) -> Result<(), MediaError> {
        self.install(&item);
        self.call(item, Ok(()), MediaCall::SetCurrent).await
    }

    async fn set_next(&self, item: Option<Arc<dyn MediaItem>>) -> Result<(), MediaError> {
        self.call(item, Ok(()), MediaCall::SetNext).await
    }

    async fn pause(&self) {
        self.call((), (), MediaCall::Pause).await
    }

    async fn resume(&self) {
        self.call((), (), MediaCall::Resume).await
    }

    async fn seek(&self, position: MediaTime) -> Result<(), MediaError> {
        self.call(position, Ok(()), MediaCall::Seek).await
    }

    async fn set_volume(&self, volume: f32) {
        self.gains.lock().unwrap().push((self.slot, volume));
    }

    async fn reset(&self) {
        self.gains.lock().unwrap().push((self.slot, 1.0));
        let _ = self.commands.send(Native::Reset(self.slot));
        let gate = self.reset_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.started.send(());
            let _ = gate.release.await;
        }
    }

    // The scripted instance owns no rendering or networking resources.
    fn dispose(&self) {}
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.commands.send(Native::Dropped(self.slot));
    }
}
