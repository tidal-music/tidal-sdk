//! Browser tasks run on the local executor.

use std::future::Future;
use std::time::Duration;

#[derive(Clone)]
pub struct Runtime;

impl Runtime {
    pub fn current() -> Self {
        Self
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> Task {
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = futures_util::future::Abortable::new(future, registration).await;
        });
        Task(abort)
    }
}

pub struct Task(futures_util::future::AbortHandle);

impl Task {
    pub fn abort(&self) {
        self.0.abort();
    }
}

pub async fn sleep(duration: Duration) {
    let (mut finished, completion) = tokio::sync::oneshot::channel();
    // Create, poll, and drop the JS timer locally; only the completion crosses awaits.
    wasm_bindgen_futures::spawn_local(async move {
        let timer = gloo_timers::future::TimeoutFuture::new(
            duration.as_millis().min(i32::MAX as u128) as u32,
        );
        tokio::select! {
            biased;
            () = finished.closed() => {}
            () = timer => { let _ = finished.send(()); }
        }
    });
    // Dropping the receiver wakes the local task to clear an unfinished timer.
    let _ = completion.await;
}
