//! `globalThis.Worker` — Web Worker (subset).
//!
//! Each `new Worker(path)` spawns a dedicated OS thread with its own
//! `bun_jsc::Context`. Messages between main and worker travel as JSON
//! strings (structured-clone is too much for an MVP); they're delivered
//! via mpsc channels and `onmessage` callbacks on each side.
//!
//! Supported:
//!   - new Worker("./worker.ts")
//!   - worker.postMessage(value)
//!   - worker.onmessage = (ev) => …          // ev.data = parsed value
//!   - worker.onerror = (e) => …
//!   - worker.terminate()
//!   - Inside the worker:
//!       globalThis.postMessage(value)
//!       globalThis.onmessage = (ev) => …
//!       globalThis.close()                  // exits the worker thread
//!
//! Not supported (yet):
//!   - SharedArrayBuffer / Transferable arguments
//!   - module / classic dichotomy (all workers run as bun-rs modules)
//!   - WorkerGlobalScope event-listener API beyond `on*`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use std::sync::mpsc;

struct WorkerHandle {
    /// main → worker: JSON-encoded message strings.
    to_worker: mpsc::Sender<WorkerMsg>,
    /// worker → main: parent-bound events. Drained by the main thread's
    /// event loop and dispatched to the JS-side on_event callback.
    from_worker: mpsc::Receiver<ParentEvent>,
    /// on_event callback pointer (used on the main thread).
    on_event_id: usize,
    /// Set when terminate() is called.
    stopped: std::sync::Arc<AtomicBool>,
}

enum WorkerMsg {
    Message(String),
    Terminate,
}

enum ParentEvent {
    Message(String),
    Error(String),
}

static WORKERS: OnceLock<Mutex<HashMap<u64, WorkerHandle>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn workers() -> &'static Mutex<HashMap<u64, WorkerHandle>> {
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn any_active() -> bool {
    WORKERS
        .get()
        .map(|m| {
            m.lock()
                .unwrap()
                .values()
                .any(|h| !h.stopped.load(Ordering::SeqCst))
        })
        .unwrap_or(false)
}

pub fn install(ctx: &Context) {
    // __bun_worker_spawn(path, on_event_obj) → id
    let spawn_cb = Callback::new(ctx, "__bun_worker_spawn", |args| {
        let path = args.get(0).to_string();
        let on_event_v = args.get(1);
        if !on_event_v.is_object() {
            return Err("__bun_worker_spawn: on_event must be a function".into());
        }
        let on_event_obj = on_event_v.to_object().map_err(|e| e.to_string())?;
        unsafe {
            sys::JSValueProtect(
                args.context().as_raw(),
                on_event_obj.as_raw() as sys::JSValueRef,
            );
        }
        let on_event_id = on_event_obj.as_raw() as usize;

        let (to_worker_tx, to_worker_rx) = mpsc::channel::<WorkerMsg>();
        let (from_worker_tx, from_worker_rx) = mpsc::channel::<ParentEvent>();
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        workers().lock().unwrap().insert(
            id,
            WorkerHandle {
                to_worker: to_worker_tx,
                from_worker: from_worker_rx,
                on_event_id,
                stopped: stopped.clone(),
            },
        );

        // Spawn the worker thread.
        let path_buf = PathBuf::from(path);
        let stopped_clone = stopped.clone();
        std::thread::spawn(move || {
            run_worker_thread(path_buf, to_worker_rx, from_worker_tx, stopped_clone);
        });

        Ok(Value::new_number(args.context(), id as f64))
    });
    ctx.global_object()
        .set_property("__bun_worker_spawn", &spawn_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(spawn_cb);

    // __bun_worker_post(id, json) — main → worker
    let post_cb = Callback::new(ctx, "__bun_worker_post", |args| {
        let id = args.get(0).to_number() as u64;
        let json = args.get(1).to_string();
        if let Some(h) = workers().lock().unwrap().get(&id) {
            let _ = h.to_worker.send(WorkerMsg::Message(json));
        }
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_worker_post", &post_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(post_cb);

    // __bun_worker_terminate(id)
    let term_cb = Callback::new(ctx, "__bun_worker_terminate", |args| {
        let id = args.get(0).to_number() as u64;
        if let Some(h) = workers().lock().unwrap().get(&id) {
            h.stopped.store(true, Ordering::SeqCst);
            let _ = h.to_worker.send(WorkerMsg::Terminate);
        }
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_worker_terminate", &term_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(term_cb);

    // Install the JS Worker class.
    ctx.eval(JS_POLYFILL, Some("[worker-polyfill]"))
        .expect("install Worker polyfill");
}

/// Runs on the worker thread. Owns its own Context. Parent-bound messages
/// go through `parent_tx` (drained by the main thread's event loop).
fn run_worker_thread(
    path: PathBuf,
    rx: mpsc::Receiver<WorkerMsg>,
    parent_tx: mpsc::Sender<ParentEvent>,
    stopped: std::sync::Arc<AtomicBool>,
) {
    let argv = vec!["worker".to_string(), path.to_string_lossy().into_owned()];
    let runtime = crate::Runtime::new(argv);

    install_worker_self(&runtime.ctx, parent_tx.clone());

    if let Err(e) = crate::modules::run_entry(&runtime.ctx, &path) {
        let _ = parent_tx.send(ParentEvent::Error(e.to_string()));
        stopped.store(true, Ordering::SeqCst);
        return;
    }

    while !stopped.load(Ordering::SeqCst) {
        match rx.try_recv() {
            Ok(WorkerMsg::Message(json)) => {
                deliver_worker_message(&runtime.ctx, &json);
            }
            Ok(WorkerMsg::Terminate) => break,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
        let _ = crate::async_rt::drain_js_tasks(&runtime.ctx);
        let _ = crate::timers::run_one_tick(&runtime.ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn install_worker_self(ctx: &Context, parent_tx: mpsc::Sender<ParentEvent>) {
    let post_cb = Callback::new(ctx, "__bun_worker_self_post", move |args| {
        let json = args.get(0).to_string();
        let _ = parent_tx.send(ParentEvent::Message(json));
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_worker_self_post", &post_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(post_cb);

    ctx.eval(
        r#"
        globalThis.postMessage = function (data) {
            const s = JSON.stringify(data === undefined ? null : data);
            __bun_worker_self_post(s);
        };
        globalThis.onmessage = null;
        globalThis.close = function () {
            // Setting a flag JS-side won't actually stop us; the worker
            // thread reads `stopped` on the main side. As a workaround,
            // throw a sentinel; the main loop ignores.
            throw new Error("__bun_worker_close__");
        };
        globalThis.__bun_worker_dispatch = function (json) {
            if (typeof onmessage === "function") {
                try { onmessage({ data: JSON.parse(json) }); }
                catch (e) { console.error(e); }
            }
        };
        "#,
        Some("[worker-globals]"),
    )
    .unwrap();
}

fn deliver_worker_message(ctx: &Context, json: &str) {
    let global = ctx.global_object();
    let disp = match global
        .get_property("__bun_worker_dispatch")
        .and_then(|v| v.to_object())
    {
        Ok(o) => o,
        Err(_) => return,
    };
    let payload = Value::new_string(ctx, json);
    let _ = disp.call(None, &[payload]);
}

/// Drain parent-bound events on the main JS thread. Called once per event
/// loop iteration. Returns true if any work was delivered.
pub fn pump_parent_events(ctx: &Context) -> bool {
    let snapshot: Vec<(usize, ParentEvent)> = {
        let g = workers().lock().unwrap();
        let mut out = Vec::new();
        for (_, h) in g.iter() {
            while let Ok(ev) = h.from_worker.try_recv() {
                out.push((h.on_event_id, ev));
            }
        }
        out
    };
    let did_work = !snapshot.is_empty();
    for (on_event_id, ev) in snapshot {
        let obj = unsafe {
            bun_jsc::Object::from_raw_for_runtime(ctx, on_event_id as sys::JSObjectRef)
        };
        match ev {
            ParentEvent::Message(json) => {
                let kind = Value::new_string(ctx, "message");
                let payload = Value::new_string(ctx, &json);
                let _ = obj.call(None, &[kind, payload]);
            }
            ParentEvent::Error(msg) => {
                let kind = Value::new_string(ctx, "error");
                let payload = Value::new_string(ctx, &msg);
                let _ = obj.call(None, &[kind, payload]);
            }
        }
    }
    did_work
}

const JS_POLYFILL: &str = r#"
(function (g) {
  class Worker {
    constructor(url) {
      this._url = String(url);
      this.onmessage = null;
      this.onerror = null;
      const handler = (kind, payload) => {
        if (kind === "message") {
          if (typeof this.onmessage === "function") {
            try { this.onmessage({ data: JSON.parse(payload) }); }
            catch (e) { console.error(e); }
          }
        } else if (kind === "error") {
          if (typeof this.onerror === "function") {
            try { this.onerror({ message: payload }); }
            catch (e) { console.error(e); }
          }
        }
      };
      this._id = __bun_worker_spawn(this._url, handler);
    }
    postMessage(data) {
      const s = JSON.stringify(data === undefined ? null : data);
      __bun_worker_post(this._id, s);
    }
    terminate() {
      __bun_worker_terminate(this._id);
    }
  }
  g.Worker = Worker;
})(globalThis);
"#;
