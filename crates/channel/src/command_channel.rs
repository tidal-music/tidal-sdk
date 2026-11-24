use crate::{Command, Request};

use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{mpsc, oneshot, watch};

pub(super) enum ChannelEvent<T> {
    Message(T),
    Reset(oneshot::Sender<()>),
}

pub(super) fn command_channel<T>(buffer: usize) -> (CommandSender<T>, CommandReceiver<T>) {
    let (tx, rx) = mpsc::channel(buffer);
    let (reset_tx, reset_rx) = watch::channel(());
    let pending_rx = Arc::new(Mutex::new(None));
    let shared = Arc::new(Mutex::new(Shared {
        tx,
        reset_tx,
        buffer,
        pending_rx: Arc::downgrade(&pending_rx),
    }));
    (
        CommandSender { shared },
        CommandReceiver {
            rx,
            reset_rx,
            pending_rx,
        },
    )
}

struct Reset<T> {
    receiver: mpsc::Receiver<T>,
    reply: oneshot::Sender<()>,
}

struct Shared<T> {
    tx: mpsc::Sender<T>,
    reset_tx: watch::Sender<()>,
    buffer: usize,
    pending_rx: Weak<Mutex<Option<Reset<T>>>>,
}

/// Sends typed requests through a bounded FIFO that can be reset.
pub struct CommandSender<T> {
    shared: Arc<Mutex<Shared<T>>>,
}

impl<T> Clone for CommandSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T> CommandSender<T> {
    /// Submits a request when polled and waits for its typed reply.
    /// `None` means the command was abandoned without a reply.
    /// The returned future does not keep the sender alive.
    pub fn request<Q>(
        &self,
        request: Q,
    ) -> impl std::future::Future<Output = Option<Q::Response>> + use<T, Q>
    where
        Q: Request,
        T: From<Command<Q>>,
    {
        send_request(Arc::downgrade(&self.shared), request)
    }

    /// Sends an existing message, preserving any reply sender it contains.
    /// Waits for queue capacity only; does not wait for execution or a reply.
    pub async fn send(&self, message: T) -> Result<(), mpsc::error::SendError<T>> {
        let tx = self.shared.lock().unwrap().tx.clone();
        tx.send(message).await
    }

    /// Enqueues immediately without waiting for capacity. The returned future only
    /// waits for the reply; dropping it does not withdraw the queued command.
    pub fn enqueue<Q>(
        &self,
        request: Q,
    ) -> Result<
        impl std::future::Future<Output = Option<Q::Response>> + use<T, Q>,
        mpsc::error::TrySendError<T>,
    >
    where
        Q: Request,
        T: From<Command<Q>>,
    {
        let (command, response) = Command::new(request);
        self.shared.lock().unwrap().tx.try_send(command.into())?;
        Ok(async move { response.await.ok() })
    }

    /// Resets before submitting a request, then waits for cancellation and the typed reply.
    /// Reset and submission are separate operations, performed when polled.
    pub fn reset_and_request<Q>(
        &self,
        request: Q,
    ) -> impl std::future::Future<Output = Option<Q::Response>> + use<T, Q>
    where
        Q: Request,
        T: From<Command<Q>>,
    {
        let reset = self.reset();
        let send = self.request(request);
        async move {
            // Submit into the replacement FIFO before yielding; the runner still
            // cancels the active command before executing that queued request.
            let ((), response) = tokio::join!(biased; reset, send);
            response
        }
    }

    /// When polled, replaces the FIFO and waits for the runner to cancel active work.
    /// Returns early if superseded or the channel closes.
    pub fn reset(&self) -> impl std::future::Future<Output = ()> + use<T> {
        let shared = Arc::downgrade(&self.shared);
        async move {
            let (reply, completed) = oneshot::channel();
            let Some(shared) = shared.upgrade() else {
                return;
            };

            let _discarded = {
                let mut shared = shared.lock().unwrap();
                let Some(pending_rx) = shared.pending_rx.upgrade() else {
                    return;
                };

                let (tx, rx) = mpsc::channel(shared.buffer);
                let mut pending = pending_rx.lock().unwrap();
                shared.tx = tx;
                let discarded = pending.replace(Reset {
                    receiver: rx,
                    reply,
                });
                let _ = shared.reset_tx.send(());
                discarded
            };
            drop(_discarded);
            drop(shared);
            let _ = completed.await;
        }
    }
}

async fn send_request<T, Q>(shared: Weak<Mutex<Shared<T>>>, request: Q) -> Option<Q::Response>
where
    Q: Request,
    T: From<Command<Q>>,
{
    let tx = {
        let shared = shared.upgrade()?;
        let tx = shared.lock().unwrap().tx.clone();
        tx
    };
    let (command, response) = Command::new(request);
    // Neither the lock nor reset-signal ownership is held while awaiting capacity.
    tx.send(command.into()).await.ok()?;
    response.await.ok()
}

pub(super) struct CommandReceiver<T> {
    rx: mpsc::Receiver<T>,
    reset_rx: watch::Receiver<()>,
    pending_rx: Arc<Mutex<Option<Reset<T>>>>,
}

impl<T> CommandReceiver<T> {
    pub(super) async fn next_event(&mut self) -> Option<ChannelEvent<T>> {
        tokio::select! {
            biased;
            changed = self.reset_rx.changed() => {
                if changed.is_err() {
                    return None;
                }

                self.swap_receiver()
            }
            message = self.rx.recv() => {
                // Closure of the old FIFO must not hide a pending replacement.
                match message {
                    Some(message) => Some(ChannelEvent::Message(message)),
                    None => self.swap_receiver(),
                }
            }
        }
    }

    pub(super) async fn wait_for_reset(&mut self) -> Option<oneshot::Sender<()>> {
        loop {
            let changed = self.reset_rx.changed().await;
            if let Some(ChannelEvent::Reset(reply)) = self.swap_receiver() {
                return Some(reply);
            }
            if changed.is_err() {
                return None;
            }
        }
    }

    fn swap_receiver(&mut self) -> Option<ChannelEvent<T>> {
        let rx = {
            let mut pending = self.pending_rx.lock().unwrap();
            let rx = pending.take()?;
            self.reset_rx.borrow_and_update();
            rx
        };
        self.rx = rx.receiver;
        Some(ChannelEvent::Reset(rx.reply))
    }
}
