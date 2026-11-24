//! Native tasks use the captured Tokio runtime.

use std::future::Future;
use std::time::Duration;

#[derive(Clone)]
pub struct Runtime {
    handle: tokio::runtime::Handle,
}

impl Runtime {
    pub fn current() -> Self {
        Self {
            handle: tokio::runtime::Handle::current(),
        }
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> Task {
        Task(self.handle.spawn(future))
    }
}

pub struct Task(tokio::task::JoinHandle<()>);

impl Task {
    pub fn abort(&self) {
        self.0.abort();
    }
}

pub async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}
