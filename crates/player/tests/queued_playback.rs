//! Playback with the queued controller: a player without a crossfade configuration plays
//! items in sequence on one host media player. The host's media player is scripted and the
//! results are observed through the player events and the media calls.
#![cfg(not(target_arch = "wasm32"))]

mod support;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use player::{
    Error, MediaError, MediaItem, MediaPlayerEvent, MediaProduct, MediaTime, PlaybackPosition,
    Player, PlayerEvent as Event, PlayerState as State, TransportError,
};
use support::media::{self, Call, MediaCall, Native, ResetGate};
use support::playback::{input, pending, ready};
use support::player_events;
use tokio::sync::{mpsc, oneshot};

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("operation timed out")
}

/// Polls once so tests can control submission order without waiting for completion.
fn poll_command<F: std::future::Future>(future: F) -> impl std::future::Future<Output = F::Output> {
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

fn track() -> MediaProduct {
    MediaProduct::Track { id: "track".into() }
}

type Item = Arc<dyn MediaItem>;
type Request<'a> = std::pin::Pin<Box<dyn Future<Output = Result<(), Error>> + 'a>>;

struct Fixture {
    player: Arc<Player>,
    media: media::Media,
    notifications: mpsc::UnboundedReceiver<Event>,
}

impl Fixture {
    fn new() -> Self {
        let (media_factory, media) = media::scripted(media::Script::default());
        let (player_events, notifications) = player_events::record();
        let player = Arc::new(Player::new(
            support::playback::player_config(),
            support::playback::dependencies(player_events, media_factory),
        ));
        Self {
            player,
            media,
            notifications,
        }
    }

    fn emit(&self, event: MediaPlayerEvent) {
        self.media.emit(0, event);
    }

    /// The next call the player makes on the media player.
    async fn call(&mut self) -> MediaCall {
        loop {
            match bounded(self.media.native.recv()).await.unwrap() {
                Native::Command(_, call) => return call,
                Native::Reset(_) | Native::Dropped(_) => {}
            }
        }
    }

    fn no_call(&mut self) -> bool {
        while let Ok(native) = self.media.native.try_recv() {
            if matches!(native, Native::Command(..)) {
                return false;
            }
        }
        true
    }

    async fn notified(&mut self) {
        bounded(self.notifications.recv()).await.unwrap();
    }

    async fn start_play(
        &mut self,
    ) -> (
        Call<Item, Result<(), MediaError>>,
        tokio::task::JoinHandle<Result<(), Error>>,
    ) {
        let player = self.player.clone();
        let seed = ready(Ok(input()));
        let pending = tokio::spawn(async move { player.play(seed.clone()).await });
        let MediaCall::SetCurrentAndPlay(command) = self.call().await else {
            panic!("expected Play")
        };
        // Loading reports Preparing; internal reset is silent.
        self.notified().await;
        (command, pending)
    }

    async fn play(&mut self) -> Item {
        let (command, pending) = self.start_play().await;
        let item = command.input.clone();
        self.emit(MediaPlayerEvent::Current {
            item: Some(item.clone()),
        });
        command.reply.complete(Ok(()));
        bounded(pending).await.unwrap().unwrap();
        self.notified().await;
        item
    }

    async fn start_next(
        &mut self,
    ) -> (
        Call<Option<Item>, Result<(), MediaError>>,
        tokio::task::JoinHandle<Result<(), Error>>,
    ) {
        let player = self.player.clone();
        let seed = ready(Ok(input()));
        let pending = tokio::spawn(async move { player.set_next(Some(seed)).await });
        let MediaCall::SetNext(command) = self.call().await else {
            panic!("expected SetNext")
        };
        (command, pending)
    }

    async fn next(&mut self) -> Item {
        let (command, pending) = self.start_next().await;
        let item = command.input.as_ref().unwrap().clone();
        command.reply.complete(Ok(()));
        assert_eq!(bounded(pending).await.unwrap(), Ok(()));
        item
    }
}

#[tokio::test]
async fn play_replaces_a_pending_play_without_installing_the_old_seed() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let (mut finish, seed) = pending();
    let load = tokio::spawn(async move { player.play(seed.clone()).await });
    f.notified().await;
    f.play().await;
    bounded(load).await.unwrap().unwrap();
    bounded(finish.closed()).await;
    assert!(finish.send(Ok(input())).is_err());
    assert!(f.no_call());
}

#[tokio::test]
async fn current_reaches_player_before_play_command_completes() {
    let mut f = Fixture::new();
    let (command, pending) = f.start_play().await;
    let a = command.input.clone();
    f.emit(MediaPlayerEvent::Current {
        item: Some(a.clone()),
    });
    f.notified().await;
    assert!(!pending.is_finished());
    command.reply.complete(Ok(()));
    bounded(pending).await.unwrap().unwrap();
    // Only Current clears selection; the command reply must not retain it.
    let weak = Arc::downgrade(&a);
    drop(a);
    drop(command.input);
    f.emit(MediaPlayerEvent::Current { item: None });
    f.notified().await;
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn set_next_reply_cannot_overwrite_native_current() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next().await;
    let (command, pending) = f.start_next().await;
    let c = command.input.as_ref().unwrap().clone();
    f.emit(MediaPlayerEvent::Ended(a, Duration::ZERO));
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    command.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    f.notified().await;
    f.emit(MediaPlayerEvent::Current {
        item: Some(c.clone()),
    });
    f.notified().await;
}

#[tokio::test]
async fn next_observations_are_consumed_without_selecting_next() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next().await;
    let weak = Arc::downgrade(&b);
    f.emit(MediaPlayerEvent::Preparing(b.clone()));
    f.emit(MediaPlayerEvent::Prepared(b.clone()));
    f.emit(MediaPlayerEvent::Failed(
        b.clone(),
        Duration::ZERO,
        MediaError::Content {
            code: "decode_failed".into(),
            description: "failed".into(),
        },
    ));
    drop(b);
    f.emit(MediaPlayerEvent::Current { item: Some(a) });
    bounded(async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        weak.upgrade().is_none(),
        "next must neither be retained nor selected by its observations"
    );
}

#[tokio::test]
async fn clearing_next_and_resuming_are_forwarded() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.set_next(None).await });
    let MediaCall::SetNext(command) = f.call().await else {
        panic!("expected SetNext")
    };
    assert!(command.input.is_none());
    command.reply.complete(Ok(()));
    assert_eq!(bounded(pending).await.unwrap(), Ok(()));
    let player = f.player.clone();
    let pending = tokio::spawn(async move { player.resume().await });
    let MediaCall::Resume(command) = f.call().await else {
        panic!("expected Resume")
    };
    command.reply.complete(());
    bounded(pending).await.unwrap();
}

#[tokio::test]
async fn reset_abandons_pending_play_and_media_clears_selection() {
    let mut f = Fixture::new();
    let (command, pending) = f.start_play().await;
    let a = command.input.clone();
    f.emit(MediaPlayerEvent::Current {
        item: Some(a.clone()),
    });
    f.notified().await;
    bounded(f.player.reset()).await;
    bounded(pending).await.unwrap().unwrap();
    f.notified().await;
    let weak = Arc::downgrade(&a);
    drop(a);
    // An obsolete reply cannot restore selection.
    command.reply.complete(Ok(()));
    drop(command.input);
    assert!(weak.upgrade().is_none());
    f.play().await;
}

#[tokio::test]
async fn dropping_player_releases_media_command_and_event_runners() {
    let f = Fixture::new();
    let mut media = f.media;
    drop(f.player);
    while let Some(native) = bounded(media.native.recv()).await {
        assert!(!matches!(native, Native::Command(..)));
    }
}

#[tokio::test]
async fn media_seek_results_reach_the_caller() {
    let error = MediaError::Other {
        code: "seek_failed".into(),
        description: "seeking failed".into(),
    };
    for result in [Ok(()), Err(error)] {
        let mut f = Fixture::new();
        f.play().await;
        let player = f.player.clone();
        let pending = tokio::spawn(async move { player.seek(MediaTime::ZERO).await });
        let MediaCall::Seek(command) = f.call().await else {
            panic!("expected Seek");
        };
        assert!(!pending.is_finished());
        command.reply.complete(result.clone());
        assert_eq!(bounded(pending).await.unwrap(), result.map_err(Error::from));
    }
}

#[tokio::test]
async fn reset_drops_pending_media_future_and_finishes_cleanup_before_later_commands() {
    let mut f = Fixture::new();
    let (old, pending_play) = f.start_play().await;
    let (started, resetting) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    *f.media.reset_gate.lock().unwrap() = Some(ResetGate {
        started,
        release: cleanup,
    });
    let player = f.player.clone();
    let reset = tokio::spawn(async move { player.reset().await });
    bounded(resetting).await.unwrap();
    assert!(old.reply.is_closed());
    bounded(pending_play).await.unwrap().unwrap();

    let player = f.player.clone();
    let pause = poll_command(player.pause());
    assert!(f.no_call());
    release.send(()).unwrap();
    bounded(reset).await.unwrap();
    let MediaCall::Pause(command) = f.call().await else {
        panic!("expected Pause after cleanup")
    };
    command.reply.complete(());
    bounded(pause).await;
}

#[tokio::test]
async fn current_load_reports_preparing_only_after_cleanup_even_if_caller_stops_waiting() {
    let mut f = Fixture::new();
    let (started, resetting) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    *f.media.reset_gate.lock().unwrap() = Some(ResetGate {
        started,
        release: cleanup,
    });
    let seed = ready(Ok(input()));
    let product = MediaProduct::Track { id: "track".into() };
    let player = f.player.clone();
    let load = poll_command(async {
        player.play(seed.clone()).await.unwrap();
    });
    bounded(resetting).await.unwrap();
    assert!(f.notifications.try_recv().is_err());
    assert!(f.no_call());
    drop(load);
    release.send(()).unwrap();
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(product),
            state: State::Preparing,
        }
    );
    let MediaCall::SetCurrentAndPlay(command) = f.call().await else {
        panic!("expected replacement after cleanup");
    };
    command.reply.complete(Ok(()));
    drop(player);
    drop(f.player);
    assert!(bounded(f.notifications.recv()).await.is_none());
}

#[tokio::test]
async fn replacement_cancels_active_cleanup_without_emitting_the_superseded_state() {
    for reset in [false, true] {
        let mut f = Fixture::new();
        let (started, resetting) = oneshot::channel();
        let (release, cleanup) = oneshot::channel();
        *f.media.reset_gate.lock().unwrap() = Some(ResetGate {
            started,
            release: cleanup,
        });
        let old_seed = ready(Ok(input()));
        let player = f.player.clone();
        let old = poll_command(async {
            if reset {
                player.reset().await;
            } else {
                player.play(old_seed.clone()).await.unwrap();
            }
        });
        bounded(resetting).await.unwrap();
        let seed = ready(Ok(input()));
        let product = MediaProduct::Track { id: "track".into() };
        let replacement = poll_command(player.play(seed.clone()));
        let MediaCall::SetCurrentAndPlay(command) = f.call().await else {
            panic!("expected replacement playback");
        };
        assert!(release.send(()).is_err());
        bounded(old).await;
        assert_eq!(track(), product);
        assert_eq!(
            notification(&mut f).await,
            Event::StateChanged {
                item: Some(product),
                state: State::Preparing,
            }
        );
        command.reply.complete(Ok(()));
        bounded(replacement).await.unwrap();
        drop(player);
        drop(f.player);
        assert!(bounded(f.notifications.recv()).await.is_none());
    }
}

async fn notification(f: &mut Fixture) -> Event {
    bounded(f.notifications.recv()).await.unwrap()
}

#[tokio::test]
async fn public_events_identify_current_state_position_and_successful_exhaustion() {
    let mut f = Fixture::new();
    let (command, pending) = f.start_play().await;
    let item = command.input.clone();
    let product = track();
    f.emit(MediaPlayerEvent::Current {
        item: Some(item.clone()),
    });
    command.reply.complete(Ok(()));
    bounded(pending).await.unwrap().unwrap();
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(product.clone()),
            state: State::Preparing,
        }
    );
    f.emit(MediaPlayerEvent::Started(item.clone(), Duration::ZERO));
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(product.clone()),
            state: State::Playing,
        }
    );
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product.clone(),
            position: Duration::ZERO,
            duration: None,
        }
    );
    let position = PlaybackPosition {
        position: Duration::from_secs(10),
        remaining: Duration::from_secs(40),
    };
    f.emit(MediaPlayerEvent::Position(item.clone(), position));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product.clone(),
            position: position.position,
            duration: None,
        }
    );
    f.emit(MediaPlayerEvent::Duration(
        item.clone(),
        Duration::from_secs(50),
    ));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product.clone(),
            position: position.position,
            duration: Some(Duration::from_secs(50)),
        }
    );
    f.emit(MediaPlayerEvent::Paused(item.clone(), position));
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(product.clone()),
            state: State::Paused,
        }
    );
    let seek = PlaybackPosition {
        position: Duration::from_secs(20),
        remaining: Duration::from_secs(30),
    };
    f.emit(MediaPlayerEvent::Seeking(item.clone(), position.position));
    f.emit(MediaPlayerEvent::Seeked(item.clone(), seek));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product.clone(),
            position: seek.position,
            duration: Some(Duration::from_secs(50)),
        }
    );
    f.emit(MediaPlayerEvent::Ended(item, Duration::from_secs(50)));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product,
            position: Duration::from_secs(50),
            duration: Some(Duration::from_secs(50)),
        }
    );
    f.emit(MediaPlayerEvent::Completed);
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Completed
        }
    );
    bounded(f.player.reset()).await;
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle
        }
    );
    assert!(f.notifications.try_recv().is_err());
}

#[tokio::test]
async fn accepted_next_failure_is_reported_only_when_media_selects_it() {
    let mut f = Fixture::new();
    let a = f.play().await;
    f.emit(MediaPlayerEvent::Started(a.clone(), Duration::ZERO));
    assert!(matches!(
        notification(&mut f).await,
        Event::StateChanged {
            state: State::Playing,
            ..
        }
    ));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: Duration::ZERO,
            duration: None,
        }
    );
    let b = f.next().await;
    let failure = MediaError::Content {
        code: "decode".into(),
        description: "unplayable".into(),
    };
    f.emit(MediaPlayerEvent::Prepared(b.clone()));
    f.emit(MediaPlayerEvent::Failed(
        b.clone(),
        Duration::ZERO,
        failure.clone(),
    ));
    f.emit(MediaPlayerEvent::Preparing(b.clone()));
    f.emit(MediaPlayerEvent::Duration(
        b.clone(),
        Duration::from_secs(123),
    ));
    f.emit(MediaPlayerEvent::Position(
        b.clone(),
        PlaybackPosition {
            position: Duration::from_secs(12),
            remaining: Duration::from_secs(111),
        },
    ));
    // A's progress demonstrates that the media FIFO processed B's failure.
    f.emit(MediaPlayerEvent::Position(
        a.clone(),
        PlaybackPosition {
            position: Duration::from_secs(49),
            remaining: Duration::from_secs(1),
        },
    ));
    assert!(
        matches!(notification(&mut f).await, Event::PositionChanged { item, .. } if item == track())
    );
    assert!(f.notifications.try_recv().is_err());
    let product = track();
    f.emit(MediaPlayerEvent::Ended(a, Duration::from_secs(50)));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: product,
            position: Duration::from_secs(50),
            duration: None,
        }
    );
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(track()),
            state: State::Failed(failure.clone().into()),
        }
    );
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: Duration::ZERO,
            duration: None,
        }
    );
    bounded(f.player.reset()).await;
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle
        }
    );
    assert!(f.notifications.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_of_the_same_product_has_a_new_selection_and_rejects_old_changes() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next().await;
    assert_eq!(track(), track());
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(track()),
            state: State::Preparing,
        }
    );
    f.emit(MediaPlayerEvent::Failed(
        a,
        Duration::ZERO,
        MediaError::Content {
            code: "old".into(),
            description: "old item".into(),
        },
    ));
    f.emit(MediaPlayerEvent::Started(b.clone(), Duration::ZERO));
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(track()),
            state: State::Playing,
        }
    );
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: Duration::ZERO,
            duration: None,
        }
    );
    assert!(f.notifications.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_discards_old_item_changes_buffered_before_the_new_selection() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next().await;
    for millis in 1..=1024 {
        f.emit(MediaPlayerEvent::Position(
            a.clone(),
            PlaybackPosition {
                position: Duration::from_millis(millis),
                remaining: Duration::from_secs(50),
            },
        ));
    }
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    let new_position = Duration::from_secs(20);
    f.emit(MediaPlayerEvent::Position(
        b.clone(),
        PlaybackPosition {
            position: new_position,
            remaining: Duration::from_secs(30),
        },
    ));

    // Either source can win before selection. After selection only B may report progress.
    while !matches!(notification(&mut f).await, Event::CurrentItemChanged { .. }) {}
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: new_position,
            duration: None,
        }
    );
    f.emit(MediaPlayerEvent::Completed);
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Completed,
        }
    );
    assert!(f.notifications.try_recv().is_err());
}

#[tokio::test]
async fn closing_the_selected_item_stream_keeps_controller_reception_alive() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let weak = Arc::downgrade(&a);
    drop(a);
    assert!(weak.upgrade().is_none());
    // Give the runner a turn to observe item-stream closure before controller work.
    tokio::task::yield_now().await;
    f.emit(MediaPlayerEvent::Current { item: None });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle,
        }
    );

    let b = f.next().await;
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(track()),
            state: State::Preparing,
        }
    );
    f.emit(MediaPlayerEvent::Started(b.clone(), Duration::ZERO));
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(track()),
            state: State::Playing,
        }
    );
}

#[tokio::test]
async fn preparing_and_seed_failure_are_reported_without_installing_an_item() {
    let mut f = Fixture::new();
    let (finish, seed) = pending();
    let player = f.player.clone();
    let load = tokio::spawn(async move { player.play(seed.clone()).await });
    let Event::StateChanged {
        item: Some(product),
        state: State::Preparing,
    } = notification(&mut f).await
    else {
        panic!("expected preparation before seed resolves");
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
            .await
            .is_err()
    );
    assert!(finish
        .send(Err(TransportError::OtherError {
            description: "manifest unavailable".into()
        }))
        .is_ok());
    bounded(load).await.unwrap().unwrap_err();
    assert!(matches!(notification(&mut f).await,
        Event::StateChanged { item: Some(item), state: State::Failed(_) } if item == product
    ));
    assert!(f.no_call());
    bounded(f.player.reset()).await;
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle
        }
    );
}

#[tokio::test]
async fn duration_is_reported_before_rendering_and_clearing_does_not_report_completion() {
    let mut f = Fixture::new();
    let item = f.play().await;
    f.emit(MediaPlayerEvent::Duration(
        item.clone(),
        Duration::from_secs(50),
    ));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: Duration::ZERO,
            duration: Some(Duration::from_secs(50)),
        }
    );
    f.emit(MediaPlayerEvent::Current { item: None });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: None,
            state: State::Idle
        }
    );
}

#[tokio::test]
async fn rejected_next_seed_leaves_the_accepted_successor_and_public_state_unchanged() {
    let mut f = Fixture::new();
    let a = f.play().await;
    let b = f.next().await;
    let rejected = ready(Err(TransportError::OtherError {
        description: "unavailable".into(),
    }));
    assert!(bounded(f.player.set_next(Some(rejected))).await.is_err());
    assert!(f.no_call());
    assert!(f.notifications.try_recv().is_err());
    f.emit(MediaPlayerEvent::Ended(a.clone(), Duration::ZERO));
    assert_eq!(
        notification(&mut f).await,
        Event::PositionChanged {
            item: track(),
            position: Duration::ZERO,
            duration: None,
        }
    );
    f.emit(MediaPlayerEvent::Current {
        item: Some(b.clone()),
    });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(track()),
            state: State::Preparing,
        }
    );
}

#[tokio::test]
async fn installation_failure_is_emitted_even_when_the_caller_stops_waiting() {
    let mut f = Fixture::new();
    let (command, pending) = f.start_play().await;
    let product = track();
    pending.abort();
    assert!(bounded(pending).await.unwrap_err().is_cancelled());
    let error = MediaError::Content {
        code: "installation".into(),
        description: "rejected".into(),
    };
    command.reply.complete(Err(error.clone()));
    assert_eq!(
        notification(&mut f).await,
        Event::StateChanged {
            item: Some(product),
            state: State::Failed(error.into()),
        }
    );
    assert!(f.notifications.try_recv().is_err());
}

#[tokio::test]
async fn selecting_an_already_ended_item_reports_a_failure() {
    let mut f = Fixture::new();
    let item = f.next().await;
    f.emit(MediaPlayerEvent::Ended(item.clone(), Duration::ZERO));
    assert!(f.notifications.try_recv().is_err());

    f.emit(MediaPlayerEvent::Current {
        item: Some(item.clone()),
    });
    assert_eq!(
        notification(&mut f).await,
        Event::CurrentItemChanged {
            item: Some(track()),
            state: State::Failed(Error::Other {
                code: "peis".into(),
                description: "The selected playback item has already ended".into(),
                can_retry: false,
            }),
        }
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
        f.emit(event);
        let event = MediaPlayerEvent::Position(
            item.clone(),
            PlaybackPosition {
                position: Duration::from_secs(second),
                remaining: Duration::from_secs(50),
            },
        );
        f.emit(event);
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
        tokio::time::timeout(Duration::from_millis(20), f.notifications.recv())
            .await
            .is_err()
    );
    drop(item);
}

#[tokio::test]
async fn abandoned_media_replies_complete_commands_successfully() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let requests: [Request<'_>; 3] = [
        Box::pin(player.play(ready(Ok(input())))),
        Box::pin(player.set_next(Some(ready(Ok(input()))))),
        Box::pin(player.seek(MediaTime::ZERO)),
    ];

    for request in requests {
        let pending = poll_command(request);
        drop(f.call().await);
        assert_eq!(bounded(pending).await, Ok(()));
    }
}

// Command submission, ordering and cancellation as the media player observes them.

#[tokio::test]
async fn dropping_a_command_wait_does_not_cancel_the_submitted_command() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    drop(poll_command(player.pause()));
    let MediaCall::Pause(command) = f.call().await else {
        panic!("expected Pause")
    };
    command.reply.complete(());
}

#[tokio::test(start_paused = true)]
async fn media_receives_play_and_next_only_after_preparation_completes() {
    for play in [true, false] {
        let mut f = Fixture::new();
        let player = f.player.clone();
        let (finish, seed) = pending();
        let command = if play {
            Box::pin(poll_command(player.play(seed.clone()))) as Request<'_>
        } else {
            Box::pin(poll_command(player.set_next(Some(seed.clone()))))
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(1), f.call())
                .await
                .is_err(),
            "nothing reaches the media player before the seed is prepared"
        );
        let prepared = input();
        let id = prepared.id.to_string();
        assert!(finish.send(Ok(prepared)).is_ok());
        let item = match f.call().await {
            MediaCall::SetCurrentAndPlay(call) if play => {
                call.reply.complete(Ok(()));
                call.input
            }
            MediaCall::SetNext(call) if !play => {
                call.reply.complete(Ok(()));
                call.input.unwrap()
            }
            _ => panic!("expected the prepared item"),
        };
        assert_eq!(support::playback::resource_id(&item.media_input()), id);
        bounded(command).await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn replaced_preparation_cannot_reach_media_when_it_finishes_later() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let (finish, seed) = pending();
    let old = poll_command(player.play(seed.clone()));
    assert!(tokio::time::timeout(Duration::from_millis(1), f.call())
        .await
        .is_err());

    let replacement = input();
    let id = replacement.id.to_string();
    let load = poll_command(player.play(ready(Ok(replacement))));
    let MediaCall::SetCurrentAndPlay(command) = f.call().await else {
        panic!("expected the replacement")
    };
    assert_eq!(
        support::playback::resource_id(&command.input.media_input()),
        id
    );
    command.reply.complete(Ok(()));
    bounded(load).await.unwrap();
    bounded(old).await.unwrap();

    // The retained seed may finish, but its replaced command must stay cancelled.
    assert!(finish.send(Ok(input())).is_ok());
    assert!(tokio::time::timeout(Duration::from_millis(1), f.call())
        .await
        .is_err());
    let pause = poll_command(player.pause());
    let MediaCall::Pause(command) = f.call().await else {
        panic!("expected Pause")
    };
    command.reply.complete(());
    bounded(pause).await;
}

#[tokio::test]
async fn reset_discards_commands_waiting_in_a_full_queue() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let pending: Vec<_> = (0..100).map(|_| poll_command(player.pause())).collect();
    let MediaCall::Pause(in_flight) = f.call().await else {
        panic!("expected Pause")
    };
    bounded(player.reset()).await;
    for command in pending {
        bounded(command).await;
    }
    drop(in_flight);
    // Senders blocked on the full queue must not deliver to the one that replaced it.
    tokio::task::yield_now().await;
    assert!(f.no_call());
}

#[tokio::test]
async fn dropping_the_player_releases_media_while_commands_wait_in_a_full_queue() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let pending: Vec<_> = (0..100).map(|_| poll_command(player.pause())).collect();
    let MediaCall::Pause(in_flight) = f.call().await else {
        panic!("expected Pause")
    };
    drop(pending);
    drop(in_flight);
    drop(player);
    let Fixture {
        player, mut media, ..
    } = f;
    drop(player);
    // Commands already accepted may still reach the media player; then it is released.
    while bounded(media.native.recv()).await.is_some() {}
}

#[tokio::test(start_paused = true)]
async fn media_events_reach_the_player_while_the_next_item_is_preparing() {
    let mut f = Fixture::new();
    let player = f.player.clone();
    let a = f.play().await;
    let (finish, seed) = pending();
    let mut next = Box::pin(poll_command(player.set_next(Some(seed))));
    assert!(tokio::time::timeout(Duration::from_millis(1), f.call())
        .await
        .is_err());

    let position = PlaybackPosition {
        position: Duration::from_secs(1),
        remaining: Duration::from_secs(49),
    };
    f.emit(MediaPlayerEvent::Position(a, position));
    loop {
        match bounded(f.notifications.recv()).await.unwrap() {
            Event::StateChanged { .. } => {}
            event => {
                assert_eq!(
                    event,
                    Event::PositionChanged {
                        item: track(),
                        position: position.position,
                        duration: None,
                    }
                );
                break;
            }
        }
    }
    std::future::poll_fn(|cx| {
        assert!(next.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;

    assert!(finish.send(Ok(input())).is_ok());
    let MediaCall::SetNext(command) = f.call().await else {
        panic!("expected SetNext")
    };
    command.reply.complete(Ok(()));
    bounded(next).await.unwrap();
}
