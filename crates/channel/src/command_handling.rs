use crate::runtime::{BoxFuture, Runtime, Task};

use crate::command_channel::{command_channel, ChannelEvent, CommandReceiver};
use crate::CommandSender;

/// Domain behaviour driven by a resettable channel.
pub trait CommandHandler: Send + 'static {
    type Message: Send + 'static;

    /// Handle a message. Reset or closure can drop this future before completion.
    fn on_message(&mut self, message: Self::Message) -> BoxFuture<'_, ()>;

    /// Run other work while waiting for a command.
    /// Commands cancel this future, so awaits must be cancellation-safe.
    /// Wait for work before returning to avoid spinning the runner.
    fn on_idle(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            std::future::pending::<()>().await;
        })
    }
}

/// Owns a command task and requests cancellation when dropped.
/// The executor drops the handler when cancellation is processed; cleanup belongs in `Drop`.
pub struct CommandRunner {
    task: Task,
}

impl CommandRunner {
    /// Creates a bounded channel and starts its sole consumer on the current executor.
    pub fn new<S: CommandHandler + ?Sized>(
        handler: Box<S>,
        capacity: usize,
    ) -> (CommandSender<S::Message>, Self) {
        let (sender, receiver) = command_channel(capacity);
        let runner = Self {
            task: Runtime::current().spawn(run_commands(handler, receiver)),
        };
        (sender, runner)
    }
}

impl Drop for CommandRunner {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run_commands<S: CommandHandler + ?Sized>(
    mut handler: Box<S>,
    mut receiver: CommandReceiver<S::Message>,
) {
    loop {
        let event = tokio::select! {
            biased;
            event = receiver.next_event() => event,
            () = handler.on_idle() => continue,
        };
        let Some(event) = event else {
            break;
        };

        match event {
            ChannelEvent::Message(message) => {
                let reset = tokio::select! {
                    biased;
                    reset = receiver.wait_for_reset() => reset,
                    () = handler.on_message(message) => continue,
                };
                let Some(reply) = reset else {
                    break;
                };
                let _ = reply.send(());
            }
            ChannelEvent::Reset(reply) => {
                let _ = reply.send(());
            }
        }
    }
}
