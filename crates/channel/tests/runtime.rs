use std::cell::Cell;
use std::time::Duration;

use channel::runtime::{sleep, BoxFuture, Runtime, Task};
use channel::{CommandRunner, EventRunner};
use tokio::sync::oneshot;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::*;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test_configure!(run_in_browser);

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn shared_runtime_accepts_send_futures_without_requiring_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Runtime>();
    assert_send_sync::<Task>();
    assert_send_sync::<CommandRunner>();
    assert_send_sync::<EventRunner>();

    let value = Cell::new(1);
    let future: BoxFuture<'static, usize> = Box::pin(async move {
        sleep(Duration::from_millis(1)).await;
        value.set(value.get() + 1);
        value.get()
    });
    let (finished, completion) = oneshot::channel();
    Runtime::current().spawn(async move {
        let _ = finished.send(future.await);
    });
    tokio::select! {
        result = completion => assert_eq!(result.unwrap(), 2),
        () = sleep(Duration::from_secs(2)) => panic!("runtime task timed out"),
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn run_returns_the_output_and_dropping_it_aborts_the_task() {
    let runtime = Runtime::current();
    assert_eq!(runtime.run(async { 2 }).await, 2);

    struct Dropped(Option<oneshot::Sender<()>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let (dropped, aborted) = oneshot::channel();
    let (started, running) = oneshot::channel();
    let guard = Dropped(Some(dropped));
    let mut run = Box::pin(runtime.run(async move {
        let _guard = guard;
        let _ = started.send(());
        std::future::pending::<()>().await;
    }));
    tokio::select! {
        _ = &mut run => unreachable!("the task never finishes"),
        result = running => result.unwrap(),
    }
    drop(run);
    tokio::select! {
        result = aborted => result.unwrap(),
        () = sleep(Duration::from_secs(2)) => panic!("the task was not aborted"),
    }
}

#[cfg(target_arch = "wasm32")]
mod browser_timer {
    use super::*;
    use std::future::Future;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = r#"
let probe;
export function watchTimers(onCreated, onCleared) {
    const set = globalThis.setTimeout;
    const clear = globalThis.clearTimeout;
    probe = { set, clear, active: new Set() };
    globalThis.setTimeout = (callback, delay, ...args) => {
        const id = set(callback, delay, ...args);
        if (delay === 60000) {
            probe.active.add(id);
            onCreated();
        }
        return id;
    };
    globalThis.clearTimeout = id => {
        const tracked = probe.active.delete(id);
        clear(id);
        if (tracked) onCleared();
    };
}
export function activeTimers() { return probe.active.size; }
export function restoreTimers() {
    for (const id of probe.active) probe.clear(id);
    globalThis.setTimeout = probe.set;
    globalThis.clearTimeout = probe.clear;
}
"#)]
    extern "C" {
        #[wasm_bindgen(js_name = watchTimers)]
        fn watch_timers(on_created: &JsValue, on_cleared: &JsValue);
        #[wasm_bindgen(js_name = activeTimers)]
        fn active_timers() -> usize;
        #[wasm_bindgen(js_name = restoreTimers)]
        fn restore_timers();
    }

    struct Probe;
    impl Drop for Probe {
        fn drop(&mut self) {
            restore_timers();
        }
    }

    #[wasm_bindgen_test]
    async fn dropping_sleep_clears_its_browser_timer() {
        let (created, creation) = oneshot::channel();
        let mut created = Some(created);
        let on_created: Closure<dyn FnMut()> = Closure::new(move || {
            if let Some(sender) = created.take() {
                let _ = sender.send(());
            }
        });
        let (cleared, cancellation) = oneshot::channel();
        let mut cleared = Some(cleared);
        let on_cleared: Closure<dyn FnMut()> = Closure::new(move || {
            if let Some(sender) = cleared.take() {
                let _ = sender.send(());
            }
        });
        watch_timers(on_created.as_ref(), on_cleared.as_ref());
        let _probe = Probe;

        let mut timer = Box::pin(sleep(Duration::from_secs(60)));
        std::future::poll_fn(|cx| {
            assert!(timer.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        tokio::select! {
            result = creation => result.unwrap(),
            () = sleep(Duration::from_secs(2)) => panic!("browser timer was not created"),
        }
        assert_eq!(active_timers(), 1);
        drop(timer);
        tokio::select! {
            result = cancellation => result.unwrap(),
            () = sleep(Duration::from_secs(2)) => panic!("browser timer was not cleared"),
        }
        assert_eq!(active_timers(), 0);
    }
}
