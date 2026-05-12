//! Background tokio runtime + a JS task queue.
//!
//! Architecture: a tokio multi-thread runtime runs on background workers and
//! is the home for async I/O (HTTP, file, sockets when added). When a task
//! completes it needs to deliver its result back to JS — JSC is strictly
//! single-threaded, so the task posts a closure
//! (`Box<dyn FnOnce(&Context) + Send>`) onto a shared mpsc::UnboundedSender
//! whose receiver the JS thread drains in its event loop.
//!
//! Public entrypoints:
//!   - [`init`] — start the tokio runtime once, at runtime startup.
//!   - [`spawn`] — Rust-side: send a future to tokio.
//!   - [`post_to_js`] — Rust-side: deliver work back to the JS thread.
//!   - [`drain_js_tasks`] — JS thread: pull any pending tasks and run them.
//!
//! All shared state lives in a `OnceLock<AsyncRuntime>` so callers don't
//! pass handles around.

use std::sync::OnceLock;

use bun_jsc::Context;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

pub type JsTask = Box<dyn FnOnce(&Context) + Send>;

pub struct AsyncRuntime {
    pub rt: Runtime,
    pub tx: mpsc::UnboundedSender<JsTask>,
    pub rx: parking_lot_lite::Mutex<mpsc::UnboundedReceiver<JsTask>>,
}

mod parking_lot_lite {
    /// Tiny shim so we don't pull in parking_lot. std::sync::Mutex is fine
    /// here; the only reason this module exists is so the Mutex is name-spaced.
    pub use std::sync::Mutex;
}

static RUNTIME: OnceLock<AsyncRuntime> = OnceLock::new();

pub fn init() {
    let _ = RUNTIME.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let (tx, rx) = mpsc::unbounded_channel();
        AsyncRuntime {
            rt,
            tx,
            rx: parking_lot_lite::Mutex::new(rx),
        }
    });
}

fn rt() -> &'static AsyncRuntime {
    RUNTIME.get().expect("async_rt not initialized")
}

/// Spawn a future on the tokio runtime. The future has no access to JSC;
/// pair it with `post_to_js` to deliver results back.
pub fn spawn<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    rt().rt.spawn(fut);
}

/// Schedule a callback to run on the JS thread the next time the event
/// loop drains tasks.
pub fn post_to_js<F>(task: F)
where
    F: FnOnce(&Context) + Send + 'static,
{
    let _ = rt().tx.send(Box::new(task));
}

/// Drain everything currently queued. Returns the number of tasks run.
pub fn drain_js_tasks(ctx: &Context) -> usize {
    let r = rt();
    let mut guard = match r.rx.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let mut count = 0;
    while let Ok(task) = guard.try_recv() {
        drop(guard);
        task(ctx);
        guard = match r.rx.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        count += 1;
    }
    count
}

use std::sync::atomic::{AtomicUsize, Ordering};

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Bump on `post_to_js` (already wrapped) — but to avoid changing the public
/// signature we instead track via a sibling atomic kept in sync with sends.
/// Callers that need precision can use [`has_pending_async`].
pub fn has_pending_async() -> bool {
    IN_FLIGHT.load(Ordering::SeqCst) > 0
}

pub fn note_started() {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
}

pub fn note_finished() {
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
