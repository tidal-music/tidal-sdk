use crate::model::PlayerEvent;

/// Receives buffered events serially on an independent task, without player or item locks.
/// Callbacks may call synchronous Player methods and schedule asynchronous commands.
/// Return promptly without blocking the executor on commands or future events.
/// When retaining a reference to the player, use `Weak` to avoid an ownership cycle.
/// Dropping Player requests cancellation; callbacks may run until cleanup completes.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait PlayerEvents: Send + Sync {
    fn on_event(&self, event: PlayerEvent);
}
