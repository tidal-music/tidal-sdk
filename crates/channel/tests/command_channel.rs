use std::{future::Future, pin::Pin, time::Duration};

use channel::runtime::BoxFuture;
use channel::{Command, CommandHandler, CommandRunner, CommandSender, Request};
use tokio::sync::{mpsc, oneshot};

struct Work {
    id: usize,
    gate: Option<oneshot::Receiver<()>>,
}

impl Work {
    fn ready(id: usize) -> Self {
        Self { id, gate: None }
    }
}

impl Request for Work {
    type Response = usize;
}

#[derive(Debug, PartialEq)]
enum Event {
    Started(usize),
    Dropped,
}

struct Worker(mpsc::UnboundedSender<Event>);
impl CommandHandler for Worker {
    type Message = Command<Work>;

    fn on_message(&mut self, command: Self::Message) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.0.send(Event::Started(command.message.id)).unwrap();
            if let Some(gate) = command.message.gate {
                let _ = gate.await;
            }
            command.reply.complete(command.message.id);
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.send(Event::Dropped);
    }
}

fn worker() -> (
    CommandSender<Command<Work>>,
    CommandRunner,
    mpsc::UnboundedReceiver<Event>,
) {
    let (events, observed) = mpsc::unbounded_channel();
    let (sender, runner) = CommandRunner::new(Box::new(Worker(events)), 1);
    (sender, runner, observed)
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .unwrap()
}

async fn started<F: Future>(future: F) -> Pin<Box<F>> {
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    future
}

#[tokio::test]
async fn bounded_submission_preserves_order_and_cancels_sends_waiting_for_capacity() {
    let (sender, _runner, mut observed) = worker();
    let other = sender.clone();
    let (release, gate) = oneshot::channel();
    let first = started(sender.request(Work {
        id: 1,
        gate: Some(gate),
    }))
    .await;
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(1)));

    let second = started(other.request(Work::ready(2))).await;
    let third = started(sender.request(Work::ready(3))).await;
    // The active handler is gated and the sole buffered slot holds request 2.
    drop(third);
    release.send(()).unwrap();
    assert_eq!(bounded(first).await, Some(1));
    assert_eq!(bounded(second).await, Some(2));
    assert_eq!(bounded(sender.request(Work::ready(4))).await, Some(4));
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(2)));
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(4)));
    assert!(observed.try_recv().is_err());
}

#[tokio::test]
async fn reset_abandons_active_buffered_and_capacity_blocked_requests() {
    let (sender, _runner, mut observed) = worker();
    let (release, gate) = oneshot::channel();
    let active = started(sender.request(Work {
        id: 1,
        gate: Some(gate),
    }))
    .await;
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(1)));
    let queued = started(sender.request(Work::ready(2))).await;
    let blocked = started(sender.request(Work::ready(3))).await;
    let replacement = started(sender.reset_and_request(Work::ready(4))).await;

    assert_eq!(bounded(replacement).await, Some(4));
    assert_eq!(bounded(active).await, None);
    assert_eq!(bounded(queued).await, None);
    assert_eq!(bounded(blocked).await, None);
    assert!(release.send(()).is_err());
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(4)));
    assert!(observed.try_recv().is_err());
}

#[tokio::test]
async fn resets_before_service_runs_preserve_only_the_latest_request() {
    let (sender, _runner, mut observed) = worker();
    let mut replies = Vec::new();
    // Poll submissions without yielding to the service task.
    for id in 0..10 {
        replies.push(started(sender.reset_and_request(Work::ready(id))).await);
    }
    assert_eq!(bounded(replies.pop().unwrap()).await, Some(9));
    for reply in replies {
        assert_eq!(bounded(reply).await, None);
    }
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(9)));
    assert!(observed.try_recv().is_err());
}

#[tokio::test]
async fn unpolled_requests_neither_reset_nor_keep_the_service_alive() {
    let (sender, _runner, mut observed) = worker();
    let send = sender.request(Work::ready(1));
    let reset = sender.reset_and_request(Work::ready(2));
    assert_eq!(bounded(sender.request(Work::ready(3))).await, Some(3));
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(3)));
    assert!(observed.try_recv().is_err());

    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some(Event::Dropped));
    assert_eq!(bounded(send).await, None);
    assert_eq!(bounded(reset).await, None);
    assert_eq!(bounded(observed.recv()).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_resets_leave_the_service_usable() {
    let (sender, _runner, _observed) = worker();
    let first = sender.clone();
    let second = sender.clone();
    let a = tokio::spawn(async move {
        for _ in 0..1_000 {
            first.reset().await;
        }
    });
    let b = tokio::spawn(async move {
        for _ in 0..1_000 {
            second.reset().await;
        }
    });
    bounded(a).await.unwrap();
    bounded(b).await.unwrap();
    assert_eq!(bounded(sender.request(Work::ready(1))).await, Some(1));
}

#[tokio::test]
async fn immediate_submission_does_not_require_polling_the_reply() {
    let (sender, _runner, mut observed) = worker();
    let response = sender.enqueue(Work::ready(1)).ok().unwrap();
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(1)));
    assert_eq!(bounded(response).await, Some(1));
    let response = sender.enqueue(Work::ready(2)).ok().unwrap();
    drop(response);
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(2)));
}

#[tokio::test]
async fn immediate_submission_reports_capacity_and_reset_abandons_queued_replies() {
    let (sender, _runner, mut observed) = worker();
    let (_release, gate) = oneshot::channel();
    let active = sender
        .enqueue(Work {
            id: 1,
            gate: Some(gate),
        })
        .ok()
        .unwrap();
    assert_eq!(bounded(observed.recv()).await, Some(Event::Started(1)));
    let queued = sender.enqueue(Work::ready(2)).ok().unwrap();
    assert!(matches!(
        sender.enqueue(Work::ready(3)),
        Err(mpsc::error::TrySendError::Full(_))
    ));
    sender.reset().await;
    assert_eq!(bounded(active).await, None);
    assert_eq!(bounded(queued).await, None);
    let fresh = sender.enqueue(Work::ready(4)).ok().unwrap();
    assert_eq!(bounded(fresh).await, Some(4));
}
