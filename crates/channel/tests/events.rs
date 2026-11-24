use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use channel::runtime::BoxFuture;
use channel::{
    event_channel, Command, CommandHandler, CommandRunner, EventHandler, EventReceiver,
    EventRunner, Request,
};
use tokio::sync::{mpsc, oneshot};

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(1), future)
        .await
        .unwrap()
}

#[tokio::test]
async fn events_are_buffered_without_replies_and_drained_after_all_senders_close() {
    // Events do not need to implement Clone or Request.
    struct Event(usize);
    let (sender, mut receiver) = event_channel();
    let other = sender.clone();
    for value in 0..100 {
        assert!(sender.send(Event(value)).is_ok());
    }
    drop(sender);
    assert!(other.send(Event(100)).is_ok());
    drop(other);
    for expected in 0..=100 {
        assert_eq!(bounded(receiver.recv()).await.unwrap().0, expected);
    }
    assert!(bounded(receiver.recv()).await.is_none());
}

#[test]
fn dropping_receiver_releases_queued_payloads_and_rejects_further_events() {
    let (sender, receiver) = event_channel();
    let payload = Arc::new(1);
    let weak = Arc::downgrade(&payload);
    sender.send(payload).unwrap();
    drop(receiver);
    assert!(weak.upgrade().is_none());
    assert_eq!(sender.send(Arc::new(2)), Err(Arc::new(2)));
}

struct Handler {
    observed: mpsc::UnboundedSender<usize>,
    closed: Option<oneshot::Sender<()>>,
}

impl EventHandler for Handler {
    type IncomingEvent = usize;

    fn on_event(&mut self, event: usize) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.observed.send(event).unwrap();
        })
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        let _ = self.closed.take().unwrap().send(());
    }
}

#[tokio::test]
async fn runner_dispatches_in_order_and_drops_handler_on_source_closure() {
    let (sender, receiver) = event_channel();
    let (observed, mut observations) = mpsc::unbounded_channel();
    let (closed, closure) = oneshot::channel();
    let _runner = EventRunner::new(
        Box::new(Handler {
            observed,
            closed: Some(closed),
        }),
        receiver,
    );
    for event in 0..100 {
        sender.send(event).unwrap();
    }
    drop(sender);
    for expected in 0..100 {
        assert_eq!(bounded(observations.recv()).await, Some(expected));
    }
    bounded(closure).await.unwrap();
    assert_eq!(bounded(observations.recv()).await, None);
}

#[tokio::test]
async fn dropping_runner_before_start_releases_handler_and_receiver() {
    let (sender, receiver) = event_channel();
    let (observed, _observations) = mpsc::unbounded_channel();
    let (closed, closure) = oneshot::channel();
    let runner = EventRunner::new(
        Box::new(Handler {
            observed,
            closed: Some(closed),
        }),
        receiver,
    );
    drop(runner);
    bounded(closure).await.unwrap();
    assert_eq!(sender.send(42), Err(42));
}

struct Operation {
    value: usize,
    started: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl Request for Operation {
    type Response = ();
}

struct Commands(Arc<Mutex<Vec<usize>>>);
impl CommandHandler for Commands {
    type Message = Command<Operation>;

    fn on_message(&mut self, command: Self::Message) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().push(command.message.value);
            let _ = command.message.started.send(());
            let _ = command.message.release.await;
            command.reply.complete(());
        })
    }
}

struct Events(Arc<Mutex<Vec<usize>>>);
impl EventHandler for Events {
    type IncomingEvent = Operation;

    fn on_event(&mut self, event: Operation) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().push(event.value);
            let _ = event.started.send(());
            let _ = event.release.await;
        })
    }
}

fn operation(value: usize) -> (Operation, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (started, observed) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    (
        Operation {
            value,
            started,
            release: gate,
        },
        observed,
        release,
    )
}

#[tokio::test]
async fn commands_events_and_reset_progress_independently_with_shared_state() {
    let state = Arc::new(Mutex::new(Vec::new()));
    let (commands, _service) = CommandRunner::new(Box::new(Commands(state.clone())), 4);
    let (events, receiver) = event_channel();
    let _events = EventRunner::new(Box::new(Events(state.clone())), receiver);

    let (command, command_started, command_release) = operation(1);
    let command = tokio::spawn(commands.request(command));
    bounded(command_started).await.unwrap();

    // An active command does not prevent an event from being handled.
    let (event, event_started, event_release) = operation(2);
    assert!(events.send(event).is_ok());
    bounded(event_started).await.unwrap();
    command_release.send(()).unwrap();
    assert_eq!(bounded(command).await.unwrap(), Some(()));

    // A waiting event handler does not prevent another command or reset.
    let (command, command_started, command_release) = operation(3);
    let command = tokio::spawn(commands.request(command));
    bounded(command_started).await.unwrap();
    let (replacement, replacement_started, replacement_release) = operation(4);
    let replacement = tokio::spawn(commands.reset_and_request(replacement));
    bounded(replacement_started).await.unwrap();
    assert_eq!(bounded(command).await.unwrap(), None);
    assert!(command_release.send(()).is_err());
    assert_eq!(*state.lock().unwrap(), vec![1, 2, 3, 4]);

    // Reset did not cancel the event handler or replace its inbox.
    event_release.send(()).unwrap();
    let (event, event_started, event_release) = operation(5);
    assert!(events.send(event).is_ok());
    bounded(event_started).await.unwrap();
    event_release.send(()).unwrap();
    replacement_release.send(()).unwrap();
    assert_eq!(bounded(replacement).await.unwrap(), Some(()));
}

#[tokio::test]
async fn dropping_runner_cancels_an_active_event_handler() {
    let (events, receiver) = event_channel();
    let runner = EventRunner::new(Box::new(Events(Arc::default())), receiver);
    let (event, started, mut release) = operation(1);
    assert!(events.send(event).is_ok());
    bounded(started).await.unwrap();
    drop(runner);
    bounded(release.closed()).await;
    let (event, _, _) = operation(2);
    assert!(events.send(event).is_err());
}

struct IdleEvents {
    secondary: Option<EventReceiver<Operation>>,
    handler: Events,
    closed: Option<oneshot::Sender<()>>,
}

impl Drop for IdleEvents {
    fn drop(&mut self) {
        let _ = self.closed.take().unwrap().send(());
    }
}

impl EventHandler for IdleEvents {
    type IncomingEvent = Operation;

    fn on_event(&mut self, event: Operation) -> BoxFuture<'_, ()> {
        self.handler.on_event(event)
    }

    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(receiver) = &mut self.secondary else {
                return std::future::pending().await;
            };
            match receiver.recv().await {
                Some(event) => self.handler.on_event(event).await,
                None => self.secondary = None,
            }
        })
    }
}

fn idle_runner() -> (
    channel::EventSender<Operation>,
    channel::EventSender<Operation>,
    oneshot::Receiver<()>,
    EventRunner,
) {
    let (primary, receiver) = event_channel();
    let (secondary, secondary_receiver) = event_channel();
    let (closed, closure) = oneshot::channel();
    let runner = EventRunner::new(
        Box::new(IdleEvents {
            secondary: Some(secondary_receiver),
            handler: Events(Arc::default()),
            closed: Some(closed),
        }),
        receiver,
    );
    (primary, secondary, closure, runner)
}

#[tokio::test]
async fn idle_work_waits_during_event_handling_and_optional_source_closure_keeps_reception_alive() {
    let (primary, secondary, closure, _runner) = idle_runner();
    let (first, started, release) = operation(1);
    assert!(primary.send(first).is_ok());
    bounded(started).await.unwrap();
    let (second, mut started, second_release) = operation(2);
    assert!(secondary.send(second).is_ok());
    tokio::task::yield_now().await;
    assert_eq!(started.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    release.send(()).unwrap();
    bounded(started).await.unwrap();
    second_release.send(()).unwrap();
    drop(secondary);
    tokio::task::yield_now().await;
    let (third, started, release) = operation(3);
    assert!(primary.send(third).is_ok());
    bounded(started).await.unwrap();
    release.send(()).unwrap();
    drop(primary);
    // Completion releases the handler even while the runner handle is retained.
    bounded(closure).await.unwrap();
}

#[tokio::test]
async fn incoming_event_cancels_idle_work_before_handling_the_event() {
    let (primary, secondary, _closure, _runner) = idle_runner();
    let (idle, started, idle_release) = operation(1);
    assert!(secondary.send(idle).is_ok());
    bounded(started).await.unwrap();
    let (event, started, release) = operation(2);
    assert!(primary.send(event).is_ok());
    bounded(started).await.unwrap();
    assert!(idle_release.is_closed());
    release.send(()).unwrap();

    let (idle, started, release) = operation(3);
    assert!(secondary.send(idle).is_ok());
    bounded(started).await.unwrap();
    release.send(()).unwrap();
}

#[tokio::test]
async fn source_closure_cancels_idle_work_and_releases_its_receiver_and_buffered_events() {
    let (primary, secondary, closure, _runner) = idle_runner();
    let (idle, started, mut release) = operation(1);
    assert!(secondary.send(idle).is_ok());
    bounded(started).await.unwrap();
    let (buffered, started, mut buffered_release) = operation(2);
    assert!(secondary.send(buffered).is_ok());
    drop(primary);
    bounded(closure).await.unwrap();
    bounded(release.closed()).await;
    assert!(bounded(started).await.is_err());
    bounded(buffered_release.closed()).await;
    let (event, _, _) = operation(3);
    assert!(secondary.send(event).is_err());
}

#[tokio::test]
async fn dropping_runner_cancels_reception_and_releases_all_receivers() {
    let (primary, secondary, closure, runner) = idle_runner();
    tokio::task::yield_now().await;
    drop(runner);
    bounded(closure).await.unwrap();
    let (event, _, _) = operation(1);
    assert!(primary.send(event).is_err());
    let (event, _, _) = operation(2);
    assert!(secondary.send(event).is_err());
}

struct ResetHandler {
    observed: mpsc::UnboundedSender<usize>,
    started: Option<oneshot::Sender<()>>,
    hook_started: Option<oneshot::Sender<()>>,
    hook_release: Option<oneshot::Receiver<()>>,
    block_idle: bool,
}

impl EventHandler for ResetHandler {
    type IncomingEvent = usize;

    fn on_event(&mut self, event: usize) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if event == 0 {
                self.started.take().unwrap().send(()).unwrap();
                std::future::pending::<()>().await;
            }
            self.observed.send(event).unwrap();
        })
    }

    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.block_idle {
                if let Some(started) = self.started.take() {
                    started.send(()).unwrap();
                }
            }
            std::future::pending().await
        })
    }

    fn on_reset(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.hook_started.take().unwrap().send(()).unwrap();
            self.hook_release.take().unwrap().await.unwrap();
        })
    }
}

#[tokio::test]
async fn reset_cancels_active_work_discards_backlog_and_waits_for_handler_cleanup() {
    for block_idle in [false, true] {
        let (sender, receiver) = event_channel();
        let (observed, mut observations) = mpsc::unbounded_channel();
        let (started, start) = oneshot::channel();
        let (hook_started, hook_start) = oneshot::channel();
        let (release, hook_release) = oneshot::channel();
        let _runner = EventRunner::new(
            Box::new(ResetHandler {
                observed,
                started: Some(started),
                hook_started: Some(hook_started),
                hook_release: Some(hook_release),
                block_idle,
            }),
            receiver,
        );
        if !block_idle {
            sender.send(0).unwrap();
        }
        bounded(start).await.unwrap();
        sender.send(1).unwrap();
        let mut reset = Box::pin(sender.reset());
        std::future::poll_fn(|cx| {
            assert!(reset.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        sender.send(2).unwrap();
        bounded(hook_start).await.unwrap();
        std::future::poll_fn(|cx| {
            assert!(reset.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(observations.try_recv().is_err());
        release.send(()).unwrap();
        bounded(reset).await;
        assert_eq!(bounded(observations.recv()).await, Some(2));
        assert!(observations.try_recv().is_err());
    }
}

#[tokio::test]
async fn reset_finishes_if_receiver_is_dropped() {
    let (sender, receiver) = event_channel::<usize>();
    let mut reset = Box::pin(sender.reset());
    std::future::poll_fn(|cx| {
        assert!(reset.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(receiver);
    bounded(reset).await;
    bounded(sender.reset()).await;
    assert!(sender.send(1).is_err());
}
