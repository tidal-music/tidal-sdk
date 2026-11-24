#![cfg(target_arch = "wasm32")]

use channel::runtime::BoxFuture;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use channel::runtime::{sleep, Runtime};
use channel::{
    event_channel, Command, CommandHandler, CommandRunner, EventHandler, EventRunner, Request,
};
use tokio::sync::oneshot;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::select! {
        value = future => value,
        () = sleep(Duration::from_secs(2)) => panic!("browser operation timed out"),
    }
}

struct Work(bool);
impl Request for Work {
    type Response = Arc<str>;
}

struct Worker {
    events: channel::EventSender<&'static str>,
    dropped: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl CommandHandler for Worker {
    type Message = Command<Work>;

    fn on_message(&mut self, command: Self::Message) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events.send("started").unwrap();
            if command.message.0 {
                std::future::pending::<()>().await;
            }
            sleep(Duration::from_millis(5)).await;
            command.reply.complete(Arc::from("done"));
        })
    }
}

#[wasm_bindgen_test]
async fn commands_execute_reset_and_drop_on_the_browser_executor() {
    let (events, mut observed) = event_channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let (commands, runner) = CommandRunner::new(
        Box::new(Worker {
            events,
            dropped: dropped.clone(),
        }),
        2,
    );
    assert_eq!(
        &*bounded(commands.request(Work(false))).await.unwrap(),
        "done"
    );
    assert_eq!(bounded(observed.recv()).await, Some("started"));

    let (finished, completion) = oneshot::channel();
    let pending = commands.request(Work(true));
    Runtime::current().spawn(async move {
        let _ = finished.send(pending.await);
    });
    assert_eq!(bounded(observed.recv()).await, Some("started"));
    bounded(commands.reset()).await;
    assert!(bounded(completion).await.unwrap().is_none());
    assert!(bounded(commands.request(Work(false))).await.is_some());
    assert_eq!(bounded(observed.recv()).await, Some("started"));

    let (finished, completion) = oneshot::channel();
    let pending = commands.request(Work(true));
    Runtime::current().spawn(async move {
        let _ = finished.send(pending.await);
    });
    assert_eq!(bounded(observed.recv()).await, Some("started"));
    drop(runner);
    assert!(bounded(completion).await.unwrap().is_none());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(bounded(commands.request(Work(false))).await.is_none());
}

struct Observer {
    started: Option<oneshot::Sender<Arc<str>>>,
    dropped: Arc<AtomicBool>,
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl EventHandler for Observer {
    type IncomingEvent = Arc<str>;
    fn on_event(&mut self, event: Arc<str>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            sleep(Duration::from_millis(5)).await;
            self.started.take().unwrap().send(event).unwrap();
            std::future::pending::<()>().await;
        })
    }
}

#[wasm_bindgen_test]
async fn events_execute_and_drop_cancels_an_active_local_handler() {
    let (events, receiver) = event_channel();
    let (started, observed) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let runner = EventRunner::new(
        Box::new(Observer {
            started: Some(started),
            dropped: dropped.clone(),
        }),
        receiver,
    );
    events.send(Arc::from("event")).unwrap();
    assert_eq!(&*bounded(observed).await.unwrap(), "event");
    drop(runner);
    // Aborts are processed on the next local-executor turn.
    sleep(Duration::from_millis(5)).await;
    assert!(dropped.load(Ordering::SeqCst));
    assert!(events.send(Arc::from("closed")).is_err());
}

#[wasm_bindgen_test]
async fn dropping_runners_before_their_first_poll_releases_handlers() {
    let (events, mut observed) = event_channel();
    let command_dropped = Arc::new(AtomicBool::new(false));
    let (commands, runner) = CommandRunner::new(
        Box::new(Worker {
            events,
            dropped: command_dropped.clone(),
        }),
        1,
    );
    drop(runner);
    let (events, receiver) = event_channel();
    let (started, _observed) = oneshot::channel();
    let event_dropped = Arc::new(AtomicBool::new(false));
    let runner = EventRunner::new(
        Box::new(Observer {
            started: Some(started),
            dropped: event_dropped.clone(),
        }),
        receiver,
    );
    drop(runner);
    sleep(Duration::from_millis(5)).await;
    assert!(command_dropped.load(Ordering::SeqCst));
    assert!(event_dropped.load(Ordering::SeqCst));
    assert!(bounded(observed.recv()).await.is_none());
    assert!(bounded(commands.request(Work(false))).await.is_none());
    assert!(events.send(Arc::from("closed")).is_err());
}
