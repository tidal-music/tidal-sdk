//! Shared task contracts with platform-specific executors and timers.

use std::future::Future;
use std::pin::Pin;

#[cfg_attr(target_arch = "wasm32", path = "runtime-web.rs")]
#[cfg_attr(not(target_arch = "wasm32"), path = "runtime-native.rs")]
mod platform;

pub use platform::{sleep, Runtime, Task};

impl Runtime {
    /// Runs the future on this runtime, so it can be awaited from a foreign executor.
    /// Dropping the returned future aborts the task.
    pub async fn run<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> T {
        let (finished, output) = tokio::sync::oneshot::channel();
        let _task = AbortOnDrop(self.spawn(async move {
            let _ = finished.send(future.await);
        }));
        output.await.expect("the runtime outlives its tasks")
    }
}

struct AbortOnDrop(Task);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A borrowed asynchronous operation that may move between runtime threads.
///
/// ```compile_fail
/// use channel::runtime::BoxFuture;
/// use std::rc::Rc;
///
/// fn local_future() -> BoxFuture<'static, usize> {
///     let value = Rc::new(1);
///     Box::pin(async move { *value })
/// }
/// ```
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
