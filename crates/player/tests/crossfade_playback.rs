//! Crossfade playback: a player configured with a crossfade overlap plays the end of an item
//! over the start of its successor on two host media players. The host's media players are
//! scripted and the results are observed through the player events and the media calls,
//! including the volume set on each.
#![cfg(not(target_arch = "wasm32"))]

mod support;

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use player::{
    CrossfadeConfig, Error, HttpResponse, MediaError, MediaEvent, MediaItem, MediaPlayerEvent,
    MediaPlayerEvents, MediaTime, PlaybackPosition, PlaybackSeed, Player, PlayerConfig,
    PlayerEvent, PlayerEvent as Event, PlayerState, PlayerState as State,
};
use serde_json::json;
use support::media::{self, Call, MediaCall, Native};
use support::playback::{input, pending, player_config, ready, seed_from_response};
use support::player_events;
use tokio::sync::mpsc;

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("operation timed out")
}

/// Polls once so tests can control submission order without waiting for completion.
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
    tokio::time::timeout(Duration::from_secs(1), future)
        .await
        .unwrap()
        .unwrap();
}

fn track() -> player::MediaProduct {
    player::MediaProduct::Track { id: "track".into() }
}

type Item = Arc<dyn MediaItem>;

struct Fixture {
    overlap: Duration,
    player: Arc<Player>,
    native: mpsc::UnboundedReceiver<Native>,
    events: Arc<Mutex<Vec<Arc<dyn MediaPlayerEvents>>>>,
    gains: Arc<Mutex<Vec<(usize, f32)>>>,
    resets: Vec<usize>,
    items: Vec<Option<Item>>,
    notifications: mpsc::UnboundedReceiver<PlayerEvent>,
}

type Pending = tokio::task::JoinHandle<Result<(), Error>>;

impl Fixture {
    fn new() -> Self {
        Self::with_overlap(Duration::from_secs(5))
    }

    fn with_overlap(overlap: Duration) -> Self {
        Self::with_duration(overlap, Some(Duration::from_secs(50)))
    }

    fn with_duration(overlap: Duration, duration: Option<Duration>) -> Self {
        let (media_factory, media) = media::scripted(media::Script {
            announce_installed: true,
            duration,
        });
        let (player_events, notifications) = player_events::record();
        let player = Arc::new(Player::new(
            PlayerConfig {
                crossfade: Some(CrossfadeConfig { overlap }),
                ..player_config()
            },
            support::playback::dependencies(player_events, media_factory),
        ));
        Self {
            overlap,
            player,
            native: media.native,
            events: media.events,
            gains: media.gains,
            resets: Vec::new(),
            items: vec![None, None],
            notifications,
        }
    }

    fn emit(&self, slot: usize, event: MediaPlayerEvent) {
        let sink = self.events.lock().unwrap()[slot].clone();
        sink.on_event(MediaEvent::new(event));
    }

    async fn command(&mut self, expected_slot: usize) -> MediaCall {
        loop {
            match bounded(self.native.recv()).await.unwrap() {
                Native::Dropped(slot) => panic!("handler {slot} dropped before controller"),
                Native::Reset(slot) => self.resets.push(slot),
                Native::Command(slot, command) => {
                    assert_eq!(slot, expected_slot);
                    return command;
                }
            }
        }
    }

    async fn reset_observed(&mut self, expected: usize) {
        loop {
            if let Some(index) = self.resets.iter().position(|slot| *slot == expected) {
                self.resets.remove(index);
                return;
            }
            match bounded(self.native.recv()).await.unwrap() {
                Native::Reset(slot) => self.resets.push(slot),
                Native::Dropped(slot) => panic!("handler {slot} dropped before controller"),
                Native::Command(_, _) => panic!("unexpected command while waiting for reset"),
            }
        }
    }

    async fn notified(&mut self) {
        bounded(self.notifications.recv()).await.unwrap();
    }

    async fn play(&mut self) -> Item {
        self.play_seed(ready(Ok(input()))).await
    }

    async fn play_seed(&mut self, seed: Arc<PlaybackSeed>) -> Item {
        let slot = 0;
        let player = self.player.clone();
        let pending = tokio::spawn(async move { player.play(seed.clone()).await });
        let MediaCall::SetCurrentAndPlay(command) = self.command(slot).await else {
            panic!("expected Play")
        };
        let item = command.input.clone();
        self.items[slot] = Some(item.clone());
        self.emit(
            slot,
            MediaPlayerEvent::Started(item.clone(), Duration::ZERO),
        );
        command.reply.complete(Ok(()));
        bounded(pending).await.unwrap().unwrap();
        self.notified().await;
        self.resets.clear();
        item
    }

    async fn start_next(&mut self, slot: usize) -> (Call<Item, Result<(), MediaError>>, Pending) {
        self.start_next_seed(slot, ready(Ok(input()))).await
    }

    async fn start_next_seed(
        &mut self,
        slot: usize,
        seed: Arc<PlaybackSeed>,
    ) -> (Call<Item, Result<(), MediaError>>, Pending) {
        let player = self.player.clone();
        let pending = tokio::spawn(async move { player.set_next(Some(seed)).await });
        let MediaCall::Pause(command) = self.command(slot).await else {
            panic!("expected Pause before SetCurrent")
        };
        command.reply.complete(());
        let MediaCall::SetCurrent(command) = self.command(slot).await else {
            panic!("expected SetNext")
        };
        (command, pending)
    }

    async fn next(&mut self, slot: usize) -> Item {
        self.next_seed(slot, ready(Ok(input()))).await
    }

    async fn next_seed(&mut self, slot: usize, seed: Arc<PlaybackSeed>) -> Item {
        let (command, pending) = self.start_next_seed(slot, seed).await;
        let item = command.input.clone();
        self.items[slot] = Some(item.clone());
        command.reply.complete(Ok(()));
        assert_eq!(bounded(pending).await.unwrap(), Ok(()));

        self.resets.clear();
        item
    }

    fn reached(&self, slot: usize) {
        self.emit(
            slot,
            MediaPlayerEvent::Position(
                self.items[slot].as_ref().unwrap().clone(),
                PlaybackPosition {
                    position: Duration::from_secs(45),
                    remaining: Duration::from_secs(5),
                },
            ),
        );
    }

    async fn resume(&mut self, slot: usize) {
        let MediaCall::Resume(command) = self.command(slot).await else {
            panic!("expected Resume")
        };
        command.reply.complete(());
    }

    fn resumed(&self, slot: usize, item: &Item, remaining: Duration) {
        self.emit(
            slot,
            MediaPlayerEvent::Resumed(
                item.clone(),
                PlaybackPosition {
                    position: Duration::ZERO,
                    remaining,
                },
            ),
        );
    }

    async fn paused(&mut self, slot: usize, item: &Item) {
        self.emit(
            slot,
            MediaPlayerEvent::Paused(
                item.clone(),
                PlaybackPosition {
                    position: Duration::from_secs(45),
                    remaining: Duration::from_secs(5),
                },
            ),
        );
        let MediaCall::Pause(command) = self.command(1 - slot).await else {
            panic!("expected next Pause after current Paused");
        };
        command.reply.complete(());
    }

    async fn pause(&mut self, slot: usize) {
        let player = self.player.clone();
        let pending = tokio::spawn(async move { player.pause().await });
        let MediaCall::Pause(command) = self.command(slot).await else {
            panic!("expected current Pause");
        };
        command.reply.complete(());
        bounded(pending).await.unwrap();
    }
}

#[tokio::test]
async fn position_starts_next_but_end_promotes_without_restarting() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    // Preparation and selection notifications can arrive independently of Play's reply.
    while tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
        .await
        .is_ok()
    {}
    f.reached(0);
    f.resume(1).await;
    let event = bounded(f.notifications.recv()).await.unwrap();
    assert!(matches!(event, PlayerEvent::PositionChanged { .. }));
    assert_eq!(f.events.lock().unwrap().len(), 2);
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.pause(1).await;
    assert!(f.resets.is_empty());
}

#[tokio::test]
async fn replacement_reuses_next_before_and_during_overlap() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    let c = f.next(1).await;
    drop(b);
    f.reached(0);
    f.resume(1).await;
    let _d = f.next(1).await;
    drop(c);
    assert_eq!(f.events.lock().unwrap().len(), 2);
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.pause(1).await;
}

#[tokio::test]
async fn current_events_wait_for_pending_set_current_command() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    let (installation, pending) = f.start_next(1).await;
    f.emit(
        0,
        MediaPlayerEvent::Stalled(
            a.clone(),
            PlaybackPosition {
                position: Duration::from_secs(45),
                remaining: Duration::from_secs(5),
            },
        ),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.native.recv())
            .await
            .is_err()
    );
    assert!(!pending.is_finished());
    installation.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
}

#[tokio::test]
async fn replacement_waits_for_pending_resume_and_its_own_completion() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    f.reached(0);
    let MediaCall::Resume(start) = f.command(1).await else {
        panic!("expected Resume")
    };
    let player = f.player.clone();
    let mut replacement = Box::pin(poll_command(player.set_next(Some(ready(Ok(input()))))));
    std::future::poll_fn(|cx| {
        assert!(replacement.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(start);
    let MediaCall::Pause(pause) = f.command(1).await else {
        panic!("expected Pause")
    };
    std::future::poll_fn(|cx| {
        assert!(replacement.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(pause);
    let MediaCall::SetCurrent(command) = f.command(1).await else {
        panic!("expected SetCurrent")
    };
    std::future::poll_fn(|cx| {
        assert!(replacement.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    command.reply.complete(Ok(()));
    completes(replacement).await;
    f.pause(0).await;
}

#[tokio::test]
async fn natural_end_without_position_starts_next() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.pause(1).await;
    assert!(f.resets.is_empty());
}

#[tokio::test]
async fn next_end_records_history_and_clears_the_slot() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.emit(1, MediaPlayerEvent::Started(b.clone(), Duration::ZERO));
    f.emit(1, MediaPlayerEvent::Ended(b.clone(), Duration::ZERO));
    let ended = Arc::downgrade(&b);
    f.items[1] = None;
    drop(b);
    bounded(async {
        while ended.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(f.native.try_recv().is_err());
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.notified().await;
    f.resume(1).await;
    f.pause(1).await;
}

#[tokio::test(start_paused = true)]
async fn seek_before_overlap_pauses_and_rewinds_next() {
    let mut f = Fixture::new();
    let _a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.seek(Duration::from_secs(42)).await });
    let MediaCall::Seek(seek) = f.command(0).await else {
        panic!("expected Seek")
    };
    assert_eq!(seek.input, Duration::from_secs(42));
    f.gains.lock().unwrap().clear();
    seek.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.emit(
        0,
        MediaPlayerEvent::Seeked(
            f.items[0].as_ref().unwrap().clone(),
            PlaybackPosition {
                position: Duration::from_secs(41),
                remaining: Duration::from_secs(8),
            },
        ),
    );
    f.emit(
        0,
        MediaPlayerEvent::Position(
            f.items[0].as_ref().unwrap().clone(),
            PlaybackPosition {
                position: Duration::from_secs(41),
                remaining: Duration::from_secs(8),
            },
        ),
    );
    let MediaCall::Pause(pause) = f.command(1).await else {
        panic!("expected Pause");
    };
    pause.reply.complete(());
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected Seek")
    };
    assert_eq!(seek.input, Duration::ZERO);
    seek.reply.complete(Ok(()));
    wait_for_gain(&f, 0, 1.0).await;
    wait_for_gain(&f, 1, 0.0).await;
    assert_eq!(
        f.gains
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(slot, _)| *slot == 0),
        Some(&(0, 1.0))
    );
    assert_eq!(
        f.gains
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(slot, _)| *slot == 1),
        Some(&(1, 0.0))
    );
    let count = f.gains.lock().unwrap().len();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(f.gains.lock().unwrap().len(), count);
    let player = f.player.clone();
    let resumed = tokio::spawn(async move { player.resume().await });
    f.resume(0).await;
    bounded(resumed).await.unwrap();
    f.reached(0);
    f.resume(1).await;
}

#[tokio::test]
async fn current_paused_orders_next_pause_after_buffered_position_resume() {
    let mut f = Fixture::new();
    let current = f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.pause().await });
    let MediaCall::Pause(pause) = f.command(0).await else {
        panic!("expected current Pause");
    };
    let position = PlaybackPosition {
        position: Duration::from_secs(48),
        remaining: Duration::from_secs(2),
    };

    // Both observations wait while the controller awaits current's Pause reply.
    f.emit(0, MediaPlayerEvent::Position(current.clone(), position));
    f.emit(0, MediaPlayerEvent::Paused(current.clone(), position));
    pause.reply.complete(());
    bounded(pending).await.unwrap();

    f.resume(1).await;
    let MediaCall::Pause(pause) = f.command(1).await else {
        panic!("expected next Pause after the buffered Resume");
    };
    pause.reply.complete(());

    // Completing a paused seek must not restart next.
    f.emit(
        0,
        MediaPlayerEvent::Seeking(current.clone(), position.position),
    );
    f.emit(0, MediaPlayerEvent::Seeked(current, position));
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected next Seek");
    };
    seek.reply.complete(Ok(()));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), f.native.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn resume_waits_for_current_event_before_resuming_next() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.pause().await });
    let MediaCall::Pause(pause) = f.command(0).await else {
        panic!("expected Pause")
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.native.recv())
            .await
            .is_err()
    );
    pause.reply.complete(());
    bounded(pending).await.unwrap();
    f.paused(0, &a).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    let MediaCall::Resume(resume) = f.command(0).await else {
        panic!("expected Resume")
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.native.recv())
            .await
            .is_err()
    );
    resume.reply.complete(());
    bounded(pending).await.unwrap();
    assert!(f.native.try_recv().is_err());
    f.resumed(0, &a, f.overlap);
    f.resume(1).await;
}

#[tokio::test]
async fn clear_next_resets_only_next_and_disables_fading() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    assert_eq!(f.player.set_next(None).await, Ok(()));
    f.reset_observed(1).await;
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Position(
            item,
            PlaybackPosition {
                position: Duration::from_secs(49),
                remaining: Duration::from_secs(1),
            },
        ),
    );
    wait_for_gain(&f, 0, 1.0).await;
    wait_for_gain(&f, 1, 1.0).await;
    f.pause(0).await;
}

#[tokio::test]
async fn reset_reuses_slots_with_pending_playback_command() {
    for replacing_next in [false, true] {
        let mut f = Fixture::new();
        f.play().await;
        f.next(1).await;
        f.reached(0);
        let replacement = if replacing_next {
            f.resume(1).await;
            let player = f.player.clone();
            Some(tokio::spawn(async move {
                player.set_next(Some(ready(Ok(input())))).await
            }))
        } else {
            None
        };
        let reply = match f.command(1).await {
            MediaCall::Pause(command) if replacing_next => command.reply,
            MediaCall::Resume(command) if !replacing_next => command.reply,
            _ => panic!("expected pending playback command"),
        };
        bounded(f.player.reset()).await;
        f.reset_observed(0).await;
        f.reset_observed(1).await;
        assert!(reply.is_closed());
        if let Some(replacement) = replacement {
            assert_eq!(bounded(replacement).await.unwrap(), Ok(()));
        }
        assert!(f.native.try_recv().is_err());
        f.play().await;
    }
}

#[tokio::test]
async fn abandoned_start_reply_leaves_both_slots_selected() {
    let mut f = Fixture::new();
    let _a = f.play().await;
    f.next(1).await;
    f.reached(0);
    let MediaCall::Resume(start) = f.command(1).await else {
        panic!("expected Resume")
    };
    drop(start);
    f.pause(0).await;
}

#[tokio::test]
async fn resume_before_overlap_leaves_next_prepared() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.pause(0).await;
    f.paused(0, &a).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    f.resume(0).await;
    bounded(pending).await.unwrap();
    f.resumed(0, &a, Duration::from_secs(50));
    f.reached(0);
    f.resume(1).await;
    f.pause(0).await;
}

#[tokio::test]
async fn paused_overlap_resumes_a_loaded_replacement() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.pause(0).await;
    f.paused(0, &a).await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    f.resume(0).await;
    f.resumed(0, &a, f.overlap);
    f.resume(1).await;
    bounded(pending).await.unwrap();
}

#[tokio::test]
async fn loading_current_reuses_both_handlers_and_waits_for_installation() {
    let mut f = Fixture::new();
    assert_eq!(f.events.lock().unwrap().len(), 2);
    f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move {
        let seed = ready(Ok(input()));
        player.play(seed.clone()).await
    });
    let MediaCall::SetCurrentAndPlay(command) = f.command(0).await else {
        panic!("expected SetCurrentAndPlay");
    };
    assert!(!pending.is_finished());
    command.reply.complete(Ok(()));
    bounded(pending).await.unwrap().unwrap();
    f.reset_observed(0).await;
    f.reset_observed(1).await;
    assert!(
        f.resets.is_empty(),
        "current loading must reset each slot only once"
    );
    assert!(f.native.try_recv().is_err());
    assert_eq!(f.events.lock().unwrap().len(), 2);
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    f.resume(0).await;
    bounded(pending).await.unwrap();
    f.next(1).await;
}

#[tokio::test]
async fn completed_handler_becomes_reusable_next_after_promotion() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.next(0).await;
    assert_eq!(f.events.lock().unwrap().len(), 2);
    f.pause(1).await;
}

#[tokio::test]
async fn next_position_is_consumed_without_controlling_playback() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let _b = f.next(1).await;
    f.reached(1);
    assert!(f.native.try_recv().is_err());
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.pause(1).await;
}

#[tokio::test]
async fn rejected_next_replacement_keeps_the_accepted_item_paused() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let (command, pending) = f.start_next(1).await;
    let _item = command.input.clone();
    let failure = MediaError::Other {
        code: "set_current_failed".into(),
        description: "setting current failed".into(),
    };
    command.reply.complete(Err(failure.clone()));
    let Err(Error::Other {
        can_retry: false,
        code: actual,
        ..
    }) = bounded(pending).await.unwrap()
    else {
        panic!("expected an other error");
    };
    assert_eq!(actual, "pp:set_current_failed");
    assert!(f.native.try_recv().is_err());
    f.pause(0).await;
}

#[tokio::test]
async fn pause_ignores_missing_reply_and_waits_for_paused_event() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.pause().await });
    let MediaCall::Pause(command) = f.command(0).await else {
        panic!("expected Pause");
    };
    drop(command);
    bounded(pending).await.unwrap();
    assert!(f.native.try_recv().is_err());
    f.paused(0, &item).await;
}

#[tokio::test]
async fn dropping_controller_releases_slots_while_event_command_is_pending() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    f.reached(0);
    let MediaCall::Resume(pending) = f.command(1).await else {
        panic!("expected Resume")
    };
    drop(f.player);
    let mut dropped = Vec::new();
    while dropped.len() < 2 {
        match bounded(f.native.recv()).await.unwrap() {
            Native::Dropped(slot) => dropped.push(slot),
            Native::Reset(_) => {}
            Native::Command(_, _) => panic!("unexpected command after drop"),
        }
    }
    dropped.sort_unstable();
    assert_eq!(dropped, vec![0, 1]);
    drop(pending);
}

#[tokio::test]
async fn ended_is_handled_without_waiting_for_end_position_resume_reply() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    let MediaCall::Resume(pending) = f.command(1).await else {
        panic!("expected Resume")
    };
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.notified().await;
    pending.reply.complete(());
    f.resume(1).await;
    f.pause(1).await;
}

#[tokio::test]
async fn natural_end_resumes_next_regardless_of_last_observed_playing_state() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.pause(0).await;
    f.paused(0, &a).await;
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    f.pause(1).await;
}

#[tokio::test]
async fn position_waits_for_seek_reply_before_starting_next() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.seek(Duration::from_secs(42)).await });
    let MediaCall::Seek(seek) = f.command(0).await else {
        panic!("expected Seek");
    };
    f.emit(
        0,
        MediaPlayerEvent::Seeked(
            f.items[0].as_ref().unwrap().clone(),
            PlaybackPosition {
                position: Duration::from_secs(42),
                remaining: Duration::from_secs(3),
            },
        ),
    );
    f.reached(0);
    tokio::task::yield_now().await;
    assert!(f.native.try_recv().is_err());
    seek.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected Seek before Resume");
    };
    assert_eq!(seek.input, Duration::from_secs(2));
    seek.reply.complete(Ok(()));
    f.resume(1).await;
}

#[tokio::test]
async fn new_play_interrupts_an_unanswered_seek() {
    let mut f = Fixture::new();
    f.play().await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.seek(Duration::from_secs(42)).await });
    let MediaCall::Seek(seek) = f.command(0).await else {
        panic!("expected Seek");
    };
    f.play().await;
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    // The old native reply is no longer needed to make progress.
    seek.reply.complete(Ok(()));
}

#[tokio::test]
async fn replacing_next_discards_its_buffered_end_and_position() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    f.emit(
        1,
        MediaPlayerEvent::Position(
            b.clone(),
            PlaybackPosition {
                position: Duration::from_secs(45),
                remaining: Duration::from_secs(5),
            },
        ),
    );
    f.emit(1, MediaPlayerEvent::Ended(b, Duration::ZERO));
    let _c = f.next(1).await;
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    f.notified().await;
    // A pause reaches both players without a stale position enqueuing Resume on 0.
    f.pause(1).await;
}

#[tokio::test]
async fn new_play_discards_buffered_end_and_position_from_old_current() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let seek = tokio::spawn(async move { player.seek(Duration::from_secs(2)).await });
    let MediaCall::Seek(old_seek) = f.command(0).await else {
        panic!("expected Seek");
    };
    f.emit(
        0,
        MediaPlayerEvent::Position(
            a.clone(),
            PlaybackPosition {
                position: Duration::from_secs(45),
                remaining: Duration::from_secs(5),
            },
        ),
    );
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.play().await;
    assert_eq!(bounded(seek).await.unwrap(), Ok(()));
    drop(old_seek);
    f.pause(0).await;
}

#[tokio::test]
async fn seeking_before_overlap_pauses_and_rewinds_prepared_next() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.seek(Duration::from_secs(42)).await });
    let MediaCall::Seek(seek) = f.command(0).await else {
        panic!("expected Seek");
    };
    seek.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.emit(
        0,
        MediaPlayerEvent::Seeked(
            f.items[0].as_ref().unwrap().clone(),
            PlaybackPosition {
                position: Duration::from_secs(42),
                remaining: Duration::from_secs(8),
            },
        ),
    );
    let MediaCall::Pause(pause) = f.command(1).await else {
        panic!("expected Pause");
    };
    pause.reply.complete(());
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected Seek before Resume");
    };
    assert_eq!(seek.input, Duration::from_secs(0));
    seek.reply.complete(Ok(()));
    f.reached(0);
    f.resume(1).await;
}

#[tokio::test]
async fn resume_ignores_missing_replies_from_both_active_players() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    for slot in [0, 1] {
        let MediaCall::Resume(command) = f.command(slot).await else {
            panic!("expected Resume");
        };
        drop(command);
        if slot == 0 {
            f.resumed(0, &a, f.overlap);
        }
    }
    bounded(pending).await.unwrap();
}

#[tokio::test]
async fn current_failure_is_reported_and_current_load_recovers() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.resets.clear();
    f.emit(
        0,
        MediaPlayerEvent::Failed(
            a.clone(),
            Duration::ZERO,
            MediaError::Other {
                code: "engine_failed".into(),
                description: "media engine failed".into(),
            },
        ),
    );

    let MediaCall::Pause(command) = f.command(1).await else {
        panic!("expected next Pause after current failed");
    };
    command.reply.complete(());
    while !matches!(
        bounded(f.notifications.recv()).await.unwrap(),
        PlayerEvent::StateChanged {
            state: PlayerState::Failed(_),
            ..
        }
    ) {}
    f.play().await;
}

#[tokio::test]
async fn positions_drive_gains_without_clock_progress() {
    let mut f = Fixture::new();
    let item = f.play().await;
    let next = f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let next_item = f.items[1].as_ref().unwrap().clone();
    f.emit(
        1,
        MediaPlayerEvent::Started(next_item.clone(), Duration::ZERO),
    );
    wait_for_gain(&f, 0, 1.0).await;
    f.gains.lock().unwrap().clear();

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(f.gains.lock().unwrap().is_empty());
    for (remaining, outgoing, incoming) in
        [(10, 1.0, 0.0), (5, 1.0, 0.0), (4, 0.8, 0.2), (0, 0.0, 1.0)]
    {
        f.gains.lock().unwrap().clear();
        f.emit(
            0,
            MediaPlayerEvent::Position(
                item.clone(),
                PlaybackPosition {
                    position: Duration::from_secs(50 - remaining),
                    remaining: Duration::from_secs(remaining),
                },
            ),
        );
        wait_for_gain(&f, 0, outgoing).await;
        wait_for_gain(&f, 1, incoming).await;
        assert_eq!(f.gains.lock().unwrap().len(), 2);
        if remaining <= 5 {
            f.resume(1).await;
        }
    }
    assert!(f.native.try_recv().is_err());
    f.gains.lock().unwrap().clear();
    f.emit(
        1,
        MediaPlayerEvent::Position(
            next.clone(),
            PlaybackPosition {
                position: Duration::from_secs(4),
                remaining: Duration::from_secs(46),
            },
        ),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(f.gains.lock().unwrap().is_empty());
}

#[tokio::test]
async fn positions_without_effective_overlap_keep_both_gains_full() {
    for (overlap, duration) in [
        (Duration::ZERO, Some(Duration::from_secs(50))),
        (Duration::from_secs(5), None),
    ] {
        let mut f = Fixture::with_duration(overlap, duration);
        let item = f.play().await;
        f.next(1).await;
        f.gains.lock().unwrap().clear();
        f.emit(
            0,
            MediaPlayerEvent::Position(
                item,
                PlaybackPosition {
                    position: Duration::from_secs(49),
                    remaining: Duration::from_secs(1),
                },
            ),
        );
        wait_for_gain(&f, 0, 1.0).await;
        wait_for_gain(&f, 1, 1.0).await;
    }
}

#[tokio::test]
async fn position_resumes_next_and_starts_gain_updates() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    f.gains.lock().unwrap().clear();
    f.reached(0);
    let MediaCall::Resume(command) = f.command(1).await else {
        panic!("expected Resume");
    };
    wait_for_gain(&f, 1, 0.0).await;
    command.reply.complete(());
}

#[tokio::test]
async fn playback_before_position_does_not_change_gains() {
    let mut f = Fixture::new();
    f.play().await;
    f.next(1).await;
    f.gains.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(f.gains.lock().unwrap().is_empty());
}

#[tokio::test]
async fn zero_overlap_transition_restores_full_gain() {
    let mut f = Fixture::with_overlap(Duration::ZERO);
    let item = f.play().await;
    f.next(1).await;
    f.gains.lock().unwrap().clear();
    f.reached(0);
    let ended_at = web_time::SystemTime::now() - Duration::from_secs(10);
    f.events.lock().unwrap()[0].on_event(MediaEvent {
        event: MediaPlayerEvent::Ended(item, Duration::ZERO),
        emitted_at: ended_at,
    });
    f.resume(1).await;
    wait_for_gain(&f, 1, 1.0).await;
}

#[tokio::test]
async fn crossfade_scales_each_item_volume_and_restores_it_on_promotion() {
    fn normalized_seed(replay_gain: f32) -> Arc<PlaybackSeed> {
        seed_from_response(HttpResponse {
            status_code: 200,
            body: serde_json::to_vec(&json!({"data": {"id": "resolved-product", "attributes": {
                "uri": "https://media.example/track",
                "trackAudioNormalizationData": {"peakAmplitude": 0.5, "replayGain": replay_gain},
            }}}))
            .unwrap(),
        })
    }

    let mut f = Fixture::with_overlap(Duration::from_secs(10));
    let outgoing = f.play_seed(normalized_seed(-6.0)).await;
    let incoming = f.next_seed(1, normalized_seed(-10.0)).await;
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Position(
            outgoing.clone(),
            PlaybackPosition {
                position: Duration::from_secs(45),
                remaining: Duration::from_secs(5),
            },
        ),
    );
    f.resume(1).await;
    for (slot, expected) in [(0, 0.3971641), (1, 0.2505936)] {
        wait_for_gain(&f, slot, expected).await;
        let gains = f.gains.lock().unwrap();
        let (_, volume) = gains.iter().rev().find(|(s, _)| *s == slot).unwrap();
        assert!((volume - expected).abs() < 0.000001);
    }

    f.emit(0, MediaPlayerEvent::Ended(outgoing.clone(), Duration::ZERO));
    f.resume(1).await;
    let gains = f.gains.lock().unwrap();
    let (_, volume) = gains.iter().rev().find(|(slot, _)| *slot == 1).unwrap();
    assert!((volume - 0.5011872).abs() < 0.000001);
    assert!((outgoing.volume() - 0.7943282).abs() < 0.000001);
    assert!((incoming.volume() - 0.5011872).abs() < 0.000001);
}

#[tokio::test]
async fn promotion_restores_full_gain() {
    let mut f = Fixture::new();
    let item = f.play().await;
    let next = f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.emit(0, MediaPlayerEvent::Ended(item, Duration::ZERO));
    f.resume(1).await;
    assert_eq!(
        f.gains
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(slot, _)| *slot == 1),
        Some(&(1, 1.0))
    );
    f.gains.lock().unwrap().clear();
    f.emit(
        1,
        MediaPlayerEvent::Position(
            next,
            PlaybackPosition {
                position: Duration::from_secs(49),
                remaining: Duration::from_secs(1),
            },
        ),
    );
    wait_for_gain(&f, 1, 1.0).await;
    wait_for_gain(&f, 0, 1.0).await;
}

async fn wait_for_gain(f: &Fixture, slot: usize, expected: f32) {
    bounded(async {
        loop {
            if f.gains
                .lock()
                .unwrap()
                .iter()
                .any(|(s, gain)| *s == slot && (*gain - expected).abs() < 0.02)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test]
async fn position_and_resume_use_reported_remaining_without_pausing_next_on_stall() {
    let mut f = Fixture::new();
    let item = f.play().await;
    let next = f.next(1).await;
    let position = PlaybackPosition {
        position: Duration::from_secs(48),
        remaining: Duration::from_secs(2),
    };
    f.gains.lock().unwrap().clear();
    f.emit(0, MediaPlayerEvent::Position(item.clone(), position));
    f.resume(1).await;
    wait_for_gain(&f, 0, 0.4).await;
    wait_for_gain(&f, 1, 0.6).await;
    f.emit(1, MediaPlayerEvent::Started(next.clone(), Duration::ZERO));
    f.emit(0, MediaPlayerEvent::Stalled(item.clone(), position));
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.gains.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(f.gains.lock().unwrap().is_empty());
    assert!(f.native.try_recv().is_err());
    f.emit(0, MediaPlayerEvent::Resumed(item.clone(), position));
    f.resume(1).await;
    wait_for_gain(&f, 0, 0.4).await;
    f.paused(0, &item).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Resumed(
            item,
            PlaybackPosition {
                position: Duration::from_secs(40),
                remaining: Duration::from_secs(10),
            },
        ),
    );
    wait_for_gain(&f, 0, 1.0).await;
    assert!(f.native.try_recv().is_err());
    tokio::time::sleep(Duration::from_millis(60)).await;
    wait_for_gain(&f, 1, 0.0).await;
    assert_eq!(f.gains.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn next_stall_leaves_current_playing() {
    let mut f = Fixture::new();
    let _current = f.play().await;
    let next = f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.emit(
        1,
        MediaPlayerEvent::Stalled(
            next.clone(),
            PlaybackPosition {
                position: Duration::from_secs(2),
                remaining: Duration::from_secs(48),
            },
        ),
    );
    assert!(f.native.try_recv().is_err());
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.gains.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(f.gains.lock().unwrap().is_empty());
    // Current continues driving the fade while next is stalled.
    f.reached(0);
    wait_for_gain(&f, 0, 1.0).await;
    wait_for_gain(&f, 1, 0.0).await;
    f.resume(1).await;
}

#[tokio::test]
async fn promotion_after_pause_restores_gain() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    f.paused(0, &item).await;
    f.emit(0, MediaPlayerEvent::Ended(item, Duration::ZERO));
    f.resume(1).await;
    assert_eq!(
        f.gains
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(slot, _)| *slot == 1),
        Some(&(1, 1.0))
    );
}

#[tokio::test]
async fn position_at_asset_end_restores_full_gain_on_promotion() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.next(1).await;
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Position(
            item.clone(),
            PlaybackPosition {
                position: Duration::from_secs(50),
                remaining: Duration::ZERO,
            },
        ),
    );
    f.emit(0, MediaPlayerEvent::Ended(item, Duration::ZERO));
    f.resume(1).await;
    wait_for_gain(&f, 1, 1.0).await;
}

#[tokio::test]
async fn seeked_repositions_next_and_rendering_updates_gains() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.next(1).await;
    f.reached(0);
    f.resume(1).await;
    let next_item = f.items[1].as_ref().unwrap().clone();
    f.emit(
        1,
        MediaPlayerEvent::Started(next_item.clone(), Duration::ZERO),
    );
    let position = PlaybackPosition {
        position: Duration::from_secs(48),
        remaining: Duration::from_secs(2),
    };
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Seeked(f.items[0].as_ref().unwrap().clone(), position),
    );
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected Seek");
    };
    assert_eq!(seek.input, f.overlap - position.remaining);
    assert!(f.gains.lock().unwrap().is_empty());
    f.emit(0, MediaPlayerEvent::Position(item.clone(), position));
    wait_for_gain(&f, 0, 0.4).await;
    seek.reply.complete(Ok(()));
    wait_for_gain(&f, 1, 0.6).await;
    f.resume(1).await;
    f.paused(0, &item).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.gains.lock().unwrap().clear();
    f.emit(
        0,
        MediaPlayerEvent::Seeked(f.items[0].as_ref().unwrap().clone(), position),
    );
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected Seek");
    };
    seek.reply.complete(Ok(()));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(f.gains.lock().unwrap().is_empty());
    assert!(f.native.try_recv().is_err());
    f.emit(0, MediaPlayerEvent::Resumed(item, position));
    wait_for_gain(&f, 0, 0.4).await;
    wait_for_gain(&f, 1, 0.6).await;
    f.resume(1).await;
}

#[tokio::test]
async fn seek_into_overlap_while_paused_waits_for_current_resumed() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    f.paused(0, &a).await;
    let position = PlaybackPosition {
        position: Duration::from_secs(48),
        remaining: Duration::from_secs(2),
    };
    f.emit(
        0,
        MediaPlayerEvent::Seeked(f.items[0].as_ref().unwrap().clone(), position),
    );
    let MediaCall::Seek(seek) = f.command(1).await else {
        panic!("expected next seek")
    };
    seek.reply.complete(Ok(()));
    // Next's rendering events do not change the controller's playing state.
    f.emit(1, MediaPlayerEvent::Resumed(b, position));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), f.native.recv())
            .await
            .is_err()
    );
    f.emit(0, MediaPlayerEvent::Resumed(a, position));
    f.resume(1).await;
    wait_for_gain(&f, 0, 0.4).await;
}

#[tokio::test]
async fn next_terminal_events_remove_finished_items_but_retain_failures() {
    for terminal in 0..2 {
        let mut f = Fixture::new();
        let a = f.play().await;
        let b = f.next(1).await;
        f.emit(1, MediaPlayerEvent::Started(b.clone(), Duration::ZERO));
        let event = match terminal {
            0 => MediaPlayerEvent::Ended(b.clone(), Duration::ZERO),
            _ => MediaPlayerEvent::Failed(
                b.clone(),
                Duration::ZERO,
                MediaError::Content {
                    code: "decode_failed".into(),
                    description: "decoding failed".into(),
                },
            ),
        };
        f.emit(1, event);
        if terminal == 0 {
            let ended = Arc::downgrade(&b);
            f.items[1] = None;
            drop(b);
            bounded(async {
                while ended.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await;
        }
        assert!(f.native.try_recv().is_err());
        f.emit(0, MediaPlayerEvent::Ended(a.clone(), Duration::ZERO));
        // Finished items are removed; failures remain selected for reporting.
        if terminal == 0 {
            f.resume(1).await;
            f.pause(1).await;
        } else {
            while !matches!(
                bounded(f.notifications.recv()).await.unwrap(),
                PlayerEvent::CurrentItemChanged {
                    state: PlayerState::Failed(_),
                    ..
                }
            ) {}
        }
    }
}

#[tokio::test]
async fn replaced_item_events_update_state_without_changing_slot_placement() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    let (replacement, pending) = f.start_next(1).await;
    let c = replacement.input.clone();
    // Buffer old events while the controller awaits installation of C.
    f.emit(1, MediaPlayerEvent::Started(b.clone(), Duration::ZERO));
    f.emit(
        1,
        MediaPlayerEvent::Current {
            item: Some(b.clone()),
        },
    );
    f.emit(1, MediaPlayerEvent::Current { item: None });
    replacement.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.resume(1).await;
    // The old item's events must not control C after promotion either.
    let position = PlaybackPosition {
        position: Duration::ZERO,
        remaining: Duration::from_secs(50),
    };
    f.emit(1, MediaPlayerEvent::Seeked(b.clone(), position));
    f.emit(1, MediaPlayerEvent::Position(b.clone(), position));
    f.emit(1, MediaPlayerEvent::Ended(b.clone(), Duration::ZERO));
    assert!(f.native.try_recv().is_err());
    f.emit(1, MediaPlayerEvent::Started(c, Duration::ZERO));
    f.next(0).await;
}

#[tokio::test]
async fn next_queued_during_play_does_not_replace_current() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let playing = tokio::spawn(async move { player.play(ready(Ok(input()))).await });
    let MediaCall::SetCurrentAndPlay(play) = f.command(0).await else {
        panic!("expected play")
    };
    let player = f.player.clone();
    let loading = tokio::spawn(async move { player.set_next(Some(ready(Ok(input())))).await });
    tokio::task::yield_now().await;
    play.reply.complete(Ok(()));
    let MediaCall::Pause(pause) = f.command(1).await else {
        panic!("expected Pause before next installation")
    };
    pause.reply.complete(());
    let MediaCall::SetCurrent(next) = f.command(1).await else {
        panic!("expected next in slot 1")
    };
    next.reply.complete(Ok(()));
    bounded(playing).await.unwrap().unwrap();
    assert_eq!(bounded(loading).await.unwrap(), Ok(()));
}

#[tokio::test]
async fn missing_either_duration_skips_crossfade_and_advances_normally() {
    for known_slot in [None, Some(0), Some(1)] {
        let mut f = Fixture::with_duration(Duration::from_secs(5), None);
        let a = f.play().await;
        let b = f.next(1).await;
        if let Some(slot) = known_slot {
            let item = if slot == 0 { a.clone() } else { b.clone() };
            f.emit(
                slot,
                MediaPlayerEvent::Duration(item.clone(), Duration::from_secs(50)),
            );
        }

        f.gains.lock().unwrap().clear();
        f.reached(0);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), f.native.recv())
                .await
                .is_err()
        );
        wait_for_gain(&f, 0, 1.0).await;
        wait_for_gain(&f, 1, 1.0).await;
        f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
        f.resume(1).await;
        wait_for_gain(&f, 1, 1.0).await;
    }
}

#[tokio::test]
async fn duration_changes_update_position_gains() {
    for short_slot in [0, 1] {
        let mut f = Fixture::with_duration(Duration::from_secs(5), None);
        let a = f.play().await;
        let b = f.next(1).await;
        // Exercise either arrival order and either item limiting the overlap.
        for slot in [short_slot, 1 - short_slot] {
            let item = if slot == 0 { a.clone() } else { b.clone() };
            let duration = Duration::from_secs(if slot == short_slot { 4 } else { 50 });
            f.emit(slot, MediaPlayerEvent::Duration(item, duration));
        }

        f.emit(
            0,
            MediaPlayerEvent::Position(
                a.clone(),
                PlaybackPosition {
                    position: Duration::from_secs(if short_slot == 0 { 2 } else { 48 }),
                    remaining: Duration::from_secs(2),
                },
            ),
        );
        f.resume(1).await;
        wait_for_gain(&f, 0, 1.0).await;
        f.emit(
            1,
            MediaPlayerEvent::Duration(b.clone(), Duration::from_secs(2)),
        );
        f.gains.lock().unwrap().clear();
        f.emit(
            0,
            MediaPlayerEvent::Position(
                a,
                PlaybackPosition {
                    position: Duration::from_millis(1500),
                    remaining: Duration::from_millis(500),
                },
            ),
        );
        wait_for_gain(&f, 0, 0.5).await;
        wait_for_gain(&f, 1, 0.5).await;
    }
}

#[tokio::test]
async fn abandoned_installation_replies_complete_successfully() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.play(ready(Ok(input()))).await });
    let MediaCall::SetCurrentAndPlay(command) = f.command(0).await else {
        panic!("expected SetCurrentAndPlay");
    };
    drop(command);
    bounded(pending).await.unwrap().unwrap();

    f.play().await;
    let (command, pending) = f.start_next(1).await;
    drop(command);
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.next(1).await;
}

#[tokio::test]
async fn abandoned_seek_reply_completes_successfully() {
    let mut f = Fixture::new();
    f.play().await;
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.seek(MediaTime::ZERO).await });
    let MediaCall::Seek(command) = f.command(0).await else {
        panic!("expected Seek");
    };
    drop(command);
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.pause(0).await;
}

#[tokio::test]
async fn resume_is_forwarded_when_no_item_is_loaded() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    f.resume(0).await;
    bounded(pending).await.unwrap();
    assert!(f.native.try_recv().is_err());
}

#[tokio::test]
async fn failed_next_becomes_current_after_natural_end() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next(1).await;
    while !matches!(
        bounded(f.notifications.recv()).await.unwrap(),
        Event::CurrentItemChanged {
            state: State::Playing,
            ..
        } | Event::StateChanged {
            state: State::Playing,
            ..
        }
    ) {}
    let failure = MediaError::Content {
        code: "next".into(),
        description: "decode failed".into(),
    };
    f.emit(
        1,
        MediaPlayerEvent::Failed(b.clone(), Duration::ZERO, failure.clone()),
    );
    f.emit(0, MediaPlayerEvent::Ended(a, Duration::ZERO));
    let mut selected = false;
    loop {
        match bounded(f.notifications.recv()).await.unwrap() {
            Event::CurrentItemChanged { item, state } => {
                assert_eq!(item, Some(track()));
                selected = true;
                if let State::Failed(error) = state {
                    assert_eq!(error, failure.into());
                    break;
                }
            }
            Event::StateChanged {
                item,
                state: State::Failed(error),
            } => {
                assert!(selected);
                assert_eq!(item, Some(track()));
                assert_eq!(error, failure.into());
                break;
            }
            Event::PositionChanged { .. } | Event::StateChanged { .. } => {}
        }
    }
}

#[tokio::test]
async fn current_loads_remain_preparing_through_internal_resets() {
    let mut f = Fixture::with_duration(Duration::from_secs(5), None);
    let (finish, seed) = pending();
    let player = f.player.clone();
    let load = tokio::spawn(async move {
        player.play(seed.clone()).await.unwrap();
    });
    let Event::StateChanged {
        item: Some(product),
        state: State::Preparing,
    } = bounded(f.notifications.recv()).await.unwrap()
    else {
        panic!("expected preparation before seed resolves");
    };
    f.reset_observed(0).await;
    f.reset_observed(1).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
            .await
            .is_err()
    );
    assert!(finish.send(Ok(input())).is_ok());
    let MediaCall::SetCurrentAndPlay(command) = f.command(0).await else {
        panic!("expected current installation");
    };
    command.reply.complete(Ok(()));
    bounded(load).await.unwrap();
    assert_eq!(
        bounded(f.notifications.recv()).await.unwrap(),
        Event::CurrentItemChanged {
            item: Some(product),
            state: State::Preparing,
        }
    );
    bounded(f.player.reset()).await;
    assert_eq!(
        bounded(f.notifications.recv()).await.unwrap(),
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle,
        }
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reset_discards_buffered_observations_before_reporting_idle() {
    let mut f = Fixture::new();
    let item = f.play().await;
    // Keep the item alive to verify reset detaches its subscription, independent of drop.
    for second in 0..1024 {
        let event = MediaPlayerEvent::Current {
            item: Some(item.clone()),
        };
        f.emit(0, event);
        let event = MediaPlayerEvent::Position(
            item.clone(),
            PlaybackPosition {
                position: std::time::Duration::from_secs(second),
                remaining: std::time::Duration::from_secs(50),
            },
        );
        f.emit(0, event);
    }
    bounded(f.player.reset()).await;
    loop {
        if matches!(
            bounded(f.notifications.recv()).await.unwrap(),
            Event::CurrentItemChanged {
                item: None,
                state: State::Idle
            }
        ) {
            break;
        }
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), f.notifications.recv())
            .await
            .is_err()
    );
    drop(item);
}
