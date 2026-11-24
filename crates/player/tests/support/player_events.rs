use player::{PlayerEvent, PlayerEvents};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Callback<F>(pub F);

impl<F: Fn(PlayerEvent) + Send + Sync> PlayerEvents for Callback<F> {
    fn on_event(&self, event: PlayerEvent) {
        (self.0)(event);
    }
}

pub fn record() -> (Arc<dyn PlayerEvents>, mpsc::UnboundedReceiver<PlayerEvent>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        Arc::new(Callback(move |event| {
            let _ = sender.send(event);
        })),
        receiver,
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Ignore;

#[cfg(not(target_arch = "wasm32"))]
impl PlayerEvents for Ignore {
    fn on_event(&self, _: PlayerEvent) {}
}
