use crate::runtime::{BoxFuture, Runtime, Task};

use crate::event_channel::Event;
use crate::EventReceiver;

/// Business logic for incoming events, independent of command handling.
pub trait EventHandler: Send + 'static {
    type IncomingEvent: Send + 'static;

    /// Events are handled serially. Dropping the runner can cancel this future.
    fn on_event(&mut self, event: Self::IncomingEvent) -> BoxFuture<'_, ()>;

    /// Clear associated state after old event handling has been cancelled.
    fn on_reset(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Run other work while waiting for an event.
    /// Events or source closure cancel this future, so awaits must be cancellation-safe.
    /// Wait for work before returning to avoid spinning the runner.
    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

/// Owns an event task, independently of any command task.
/// Shared business state must not be locked across an await.
pub struct EventRunner {
    task: Task,
}

impl EventRunner {
    /// Starts dispatch on the current executor. Source closure drains the
    /// inbox, then drops the handler. The task does not retain an event sender.
    pub fn new<H>(mut handler: Box<H>, mut receiver: EventReceiver<H::IncomingEvent>) -> Self
    where
        H: EventHandler + ?Sized,
    {
        Self {
            task: Runtime::current().spawn(async move {
                loop {
                    let event = tokio::select! {
                        event = receiver.next_event() => event,
                        () = handler.on_idle() => continue,
                    };
                    let Some(event) = event else {
                        break;
                    };
                    let reply = match event {
                        Event::Reset(reply) => reply,
                        Event::Message(event) => {
                            tokio::select! {
                                biased;
                                reply = receiver.wait_for_reset() => reply,
                                () = handler.on_event(event) => continue,
                            }
                        }
                    };
                    handler.on_reset().await;
                    let _ = reply.send(());
                }
            }),
        }
    }
}

impl Drop for EventRunner {
    fn drop(&mut self) {
        // Cancellation releases the handler and inbox when the executor processes it.
        self.task.abort();
    }
}
