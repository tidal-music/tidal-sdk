use std::time::Duration;

use channel::runtime::BoxFuture;
use channel::{Command, CommandHandler, CommandRunner, Request};
use tokio::sync::{mpsc, oneshot};

enum Message {
    Block,
    Complete,
    Cleanup,
}

impl Request for Message {
    type Response = ();
}

struct TestService {
    events: mpsc::UnboundedSender<&'static str>,
    cleanup_gate: Option<oneshot::Receiver<()>>,
}

impl CommandHandler for TestService {
    type Message = Command<Message>;

    fn on_message(&mut self, command: Command<Message>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if matches!(command.message, Message::Cleanup) {
                self.events.send("cleanup started").unwrap();
                if let Some(gate) = self.cleanup_gate.take() {
                    gate.await.unwrap();
                }
                self.events.send("cleanup finished").unwrap();
                command.reply.complete(());
                return;
            }
            self.events.send("message started").unwrap();
            if matches!(command.message, Message::Block) {
                std::future::pending::<()>().await;
            }
            self.events.send("message finished").unwrap();
            command.reply.complete(());
        })
    }
}

impl Drop for TestService {
    fn drop(&mut self) {
        let _ = self.events.send("closed");
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(1), future)
        .await
        .unwrap()
}

#[tokio::test]
async fn messages_run_in_order_and_closure_runs_cleanup() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    sender.request(Message::Complete).await.unwrap();
    sender.request(Message::Complete).await.unwrap();
    for expected in [
        "message started",
        "message finished",
        "message started",
        "message finished",
    ] {
        assert_eq!(bounded(observed.recv()).await, Some(expected));
    }
    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert_eq!(bounded(observed.recv()).await, None);
}

#[tokio::test]
async fn replacement_cleanup_finishes_before_the_next_message() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (release, gate) = oneshot::channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: Some(gate),
        }),
        4,
    );
    let _blocked = started(sender.request(Message::Block)).await;
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    let resetting = started(sender.reset_and_request(Message::Cleanup)).await;
    assert_eq!(bounded(observed.recv()).await, Some("cleanup started"));
    let completion = started(sender.request(Message::Complete)).await;
    tokio::task::yield_now().await;
    assert!(observed.try_recv().is_err());
    release.send(()).unwrap();
    bounded(resetting).await;
    assert_eq!(bounded(completion).await, Some(()));
    for expected in ["cleanup finished", "message started", "message finished"] {
        assert_eq!(bounded(observed.recv()).await, Some(expected));
    }
    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
}

#[tokio::test]
async fn idle_reset_does_not_call_the_handler() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    sender.reset().await;
    assert!(observed.try_recv().is_err());
    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
}

#[tokio::test]
async fn closure_interrupts_a_message_without_calling_reset() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    let _blocked = started(sender.request(Message::Block)).await;
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert_eq!(bounded(observed.recv()).await, None);
}

#[tokio::test]
async fn dropping_runner_interrupts_an_active_message_and_drops_the_service() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    let _blocked = started(sender.request(Message::Block)).await;
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    drop(runner);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert_eq!(bounded(observed.recv()).await, None);
    assert!(sender.request(Message::Complete).await.is_none());
    sender.reset().await;
}

#[tokio::test]
async fn dropping_runner_interrupts_explicit_cleanup() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (release, gate) = oneshot::channel();
    let (sender, runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: Some(gate),
        }),
        4,
    );
    let resetting = started(sender.reset_and_request(Message::Cleanup)).await;
    assert_eq!(bounded(observed.recv()).await, Some("cleanup started"));
    drop(runner);
    bounded(resetting).await;
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert!(release.send(()).is_err());
    assert_eq!(bounded(observed.recv()).await, None);
}

#[tokio::test]
async fn dropping_runner_before_it_starts_still_drops_the_service() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    drop(runner);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert_eq!(bounded(observed.recv()).await, None);
    sender.reset().await;
}

async fn started<F: std::future::Future>(future: F) -> std::pin::Pin<Box<F>> {
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    future
}

#[tokio::test]
async fn sender_closure_releases_active_queued_and_blocked_requests_with_runner_retained() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        1,
    );
    let active = started(sender.request(Message::Block)).await;
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    let queued = started(sender.request(Message::Complete)).await;
    let blocked = started(sender.request(Message::Complete)).await;

    drop(sender);
    assert_eq!(bounded(observed.recv()).await, Some("closed"));
    assert_eq!(bounded(active).await, None);
    assert_eq!(bounded(queued).await, None);
    assert_eq!(bounded(blocked).await, None);
}

struct IdleService {
    commands: TestService,
    work: mpsc::UnboundedReceiver<()>,
}

impl CommandHandler for IdleService {
    type Message = Command<Message>;

    fn on_message(&mut self, command: Self::Message) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.commands.on_message(command).await;
        })
    }

    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.work.recv().await.is_some() {
                self.commands.events.send("idle work").unwrap();
            } else {
                std::future::pending::<()>().await;
            }
        })
    }
}

#[tokio::test]
async fn idle_work_runs_between_commands_with_ready_commands_taking_priority() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (work, receiver) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(IdleService {
            commands: TestService {
                events,
                cleanup_gate: None,
            },
            work: receiver,
        }),
        4,
    );

    // Both inputs are ready before the runner's first poll.
    work.send(()).unwrap();
    let command = sender.enqueue(Message::Complete).ok().unwrap();
    assert_eq!(bounded(command).await, Some(()));
    for expected in ["message started", "message finished", "idle work"] {
        assert_eq!(bounded(observed.recv()).await, Some(expected));
    }

    // Another command interrupts the idle wait; idle work resumes afterwards.
    assert_eq!(bounded(sender.request(Message::Complete)).await, Some(()));
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    assert_eq!(bounded(observed.recv()).await, Some("message finished"));
    work.send(()).unwrap();
    assert_eq!(bounded(observed.recv()).await, Some("idle work"));
}

#[tokio::test]
async fn idle_work_waits_during_a_command_and_reset_still_interrupts_it() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (work, receiver) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(IdleService {
            commands: TestService {
                events,
                cleanup_gate: None,
            },
            work: receiver,
        }),
        4,
    );
    let blocked = sender.enqueue(Message::Block).ok().unwrap();
    assert_eq!(bounded(observed.recv()).await, Some("message started"));
    work.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(observed.try_recv().is_err());

    sender.reset().await;
    assert_eq!(bounded(blocked).await, None);
    assert_eq!(bounded(observed.recv()).await, Some("idle work"));
}

#[tokio::test]
async fn superseded_resets_complete_without_calling_the_handler() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: None,
        }),
        4,
    );
    let first = started(sender.reset()).await;
    let second = started(sender.reset()).await;
    bounded(first).await;
    bounded(second).await;
    assert!(observed.try_recv().is_err());
    assert_eq!(bounded(sender.request(Message::Complete)).await, Some(()));
}

#[tokio::test]
async fn reset_can_interrupt_explicit_cleanup() {
    let (events, mut observed) = mpsc::unbounded_channel();
    let (release, gate) = oneshot::channel();
    let (sender, _runner) = CommandRunner::new(
        Box::new(TestService {
            events,
            cleanup_gate: Some(gate),
        }),
        4,
    );
    let cleanup = started(sender.reset_and_request(Message::Cleanup)).await;
    assert_eq!(bounded(observed.recv()).await, Some("cleanup started"));
    assert_eq!(
        bounded(sender.reset_and_request(Message::Complete)).await,
        Some(())
    );
    assert_eq!(bounded(cleanup).await, None);
    assert!(release.send(()).is_err());
    for expected in ["message started", "message finished"] {
        assert_eq!(bounded(observed.recv()).await, Some(expected));
    }
    assert!(observed.try_recv().is_err());
}
