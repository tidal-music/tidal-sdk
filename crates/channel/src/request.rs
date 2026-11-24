use tokio::sync::oneshot;

/// A domain request and the response its caller expects.
///
/// The receiving message enum implements `From<Command<Q>>` for each supported
/// request `Q`, preserving the typed reply inside the corresponding variant.
pub trait Request: Send + 'static {
    type Response: Send + 'static;
}

/// Pairs a request with its correctly typed reply.
pub struct Command<Q: Request> {
    pub message: Q,
    pub reply: Reply<Q::Response>,
}

pub struct Reply<R> {
    sender: oneshot::Sender<R>,
}

impl<R> Reply<R> {
    pub fn complete(self, response: R) {
        let _ = self.sender.send(response);
    }
}

impl<Q: Request> Command<Q> {
    pub(crate) fn new(message: Q) -> (Self, oneshot::Receiver<Q::Response>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                message,
                reply: Reply { sender },
            },
            receiver,
        )
    }
}
