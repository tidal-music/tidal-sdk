use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{mpsc, oneshot, watch};

/// Creates a resettable, unbounded FIFO with one receiver and cloneable senders.
pub fn event_channel<T>() -> (EventSender<T>, EventReceiver<T>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let (reset_sender, reset_receiver) = watch::channel(());
    let pending = Arc::new(Mutex::new(None));
    let shared = Arc::new(Mutex::new(Shared {
        sender,
        reset_sender,
        pending: Arc::downgrade(&pending),
    }));
    (
        EventSender { shared },
        EventReceiver {
            receiver,
            reset_receiver,
            pending,
        },
    )
}

struct Shared<T> {
    sender: mpsc::UnboundedSender<T>,
    reset_sender: watch::Sender<()>,
    pending: Weak<Mutex<Option<Reset<T>>>>,
}

struct Reset<T> {
    receiver: mpsc::UnboundedReceiver<T>,
    reply: oneshot::Sender<()>,
}

pub struct EventSender<T> {
    shared: Arc<Mutex<Shared<T>>>,
}

impl<T> Clone for EventSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T> EventSender<T> {
    /// Enqueues immediately without waiting for processing or a reply.
    /// Returns the event if its receiver has closed.
    pub fn send(&self, event: T) -> Result<(), T> {
        self.shared
            .lock()
            .unwrap()
            .sender
            .send(event)
            .map_err(|error| error.0)
    }

    /// Replaces the FIFO when polled and waits for receiver reset handling.
    /// Returns early if superseded or the receiver closes.
    pub async fn reset(&self) {
        let (reply, completed) = oneshot::channel();
        let discarded = {
            let mut shared = self.shared.lock().unwrap();
            let Some(pending) = shared.pending.upgrade() else {
                return;
            };
            let (sender, receiver) = mpsc::unbounded_channel();
            shared.sender = sender;
            let discarded = pending.lock().unwrap().replace(Reset { receiver, reply });
            let _ = shared.reset_sender.send(());
            discarded
        };
        drop(discarded);
        let _ = completed.await;
    }
}

pub(crate) enum Event<T> {
    Message(T),
    Reset(oneshot::Sender<()>),
}

pub struct EventReceiver<T> {
    receiver: mpsc::UnboundedReceiver<T>,
    reset_receiver: watch::Receiver<()>,
    pending: Arc<Mutex<Option<Reset<T>>>>,
}

impl<T> EventReceiver<T> {
    /// Receives events, acknowledging resets after discarding the old FIFO.
    /// EventRunner additionally waits for its handler's reset hook before acknowledging.
    /// Drains buffered events before returning `None` when all senders are gone.
    pub async fn recv(&mut self) -> Option<T> {
        loop {
            match self.next_event().await? {
                Event::Message(event) => return Some(event),
                Event::Reset(reply) => {
                    let _ = reply.send(());
                }
            }
        }
    }

    /// Discards buffered observations after the producer has stopped.
    /// Use when the receiving task itself stops the producer.
    pub fn clear(&mut self) {
        if let Some(reply) = self.swap_receiver() {
            let _ = reply.send(());
        }
        while self.receiver.try_recv().is_ok() {}
    }

    pub(crate) async fn next_event(&mut self) -> Option<Event<T>> {
        loop {
            if let Some(reply) = self.swap_receiver() {
                return Some(Event::Reset(reply));
            }
            tokio::select! {
                biased;
                changed = self.reset_receiver.changed() => {
                    if changed.is_err() {
                        if let Some(reply) = self.swap_receiver() { return Some(Event::Reset(reply)); }
                        return self.receiver.recv().await.map(Event::Message);
                    }
                }
                event = self.receiver.recv() => {
                    if let Some(reply) = self.swap_receiver() { return Some(Event::Reset(reply)); }
                    return event.map(Event::Message);
                }
            }
        }
    }

    pub(crate) async fn wait_for_reset(&mut self) -> oneshot::Sender<()> {
        loop {
            if let Some(reply) = self.swap_receiver() {
                return reply;
            }
            if self.reset_receiver.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }

    fn swap_receiver(&mut self) -> Option<oneshot::Sender<()>> {
        let reset = self.pending.lock().unwrap().take()?;
        self.reset_receiver.borrow_and_update();
        self.receiver = reset.receiver;
        Some(reset.reply)
    }
}
