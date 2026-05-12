//! `queueMicrotask` / `setTimeout` / `clearTimeout` / `setInterval` / `clearInterval`.
//!
//! MVP event loop — single-threaded, runs after the main script finishes.
//! All timer state lives in a thread-local registry; callbacks are stored as
//! protected `JSValueRef`s so they survive GC until they fire.
//!
//! The loop is driven by [`run_event_loop`], which the CLI calls after
//! `eval_*` returns. It blocks until the timer set is empty.

use std::cell::RefCell;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bun_jsc::{Callback, Context, JsString, Value};
use bun_jsc_sys as sys;

/// Public: install `queueMicrotask` / `setTimeout` / `clearTimeout` /
/// `setInterval` / `clearInterval` onto `globalThis`.
pub fn install_timers(ctx: &Context) {
    let global = ctx.global_object();

    // queueMicrotask: polyfill via `Promise.resolve().then(cb)`.
    // This is the standard implementation and matches HTML semantics.
    ctx.eval(
        r#"
        globalThis.queueMicrotask = function queueMicrotask(cb) {
            if (typeof cb !== 'function') {
                throw new TypeError('queueMicrotask requires a function');
            }
            Promise.resolve().then(cb);
        };
        "#,
        Some("[queueMicrotask]"),
    )
    .expect("install queueMicrotask polyfill");

    let set_timeout = Callback::new(ctx, "setTimeout", move |args| {
        let cb = args.get(0);
        if !cb.is_object() {
            return Err("setTimeout callback must be a function".to_string());
        }
        let cb_obj = cb.to_object().map_err(|e| e.to_string())?;
        let delay_ms = if args.len() >= 2 { args.get(1).to_number() } else { 0.0 };
        let delay = if delay_ms.is_nan() || delay_ms < 0.0 {
            Duration::from_millis(0)
        } else {
            Duration::from_millis(delay_ms as u64)
        };
        let id = register_timer(args.context(), cb_obj.as_raw(), delay, false);
        Ok(Value::new_number(args.context(), id as f64))
    });
    global
        .set_property("setTimeout", &set_timeout.value_in(ctx))
        .unwrap();
    std::mem::forget(set_timeout);

    let set_interval = Callback::new(ctx, "setInterval", move |args| {
        let cb = args.get(0);
        if !cb.is_object() {
            return Err("setInterval callback must be a function".to_string());
        }
        let cb_obj = cb.to_object().map_err(|e| e.to_string())?;
        let delay_ms = if args.len() >= 2 { args.get(1).to_number() } else { 0.0 };
        let delay = if delay_ms.is_nan() || delay_ms <= 0.0 {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(delay_ms as u64)
        };
        let id = register_timer(args.context(), cb_obj.as_raw(), delay, true);
        Ok(Value::new_number(args.context(), id as f64))
    });
    global
        .set_property("setInterval", &set_interval.value_in(ctx))
        .unwrap();
    std::mem::forget(set_interval);

    let clear_timer = Callback::new(ctx, "clearTimeout", |args| {
        if args.len() >= 1 {
            let id = args.get(0).to_number() as u64;
            cancel_timer(id);
        }
        Ok(Value::new_undefined(args.context()))
    });
    let cv = clear_timer.value_in(ctx);
    global.set_property("clearTimeout", &cv).unwrap();
    global.set_property("clearInterval", &cv).unwrap();
    std::mem::forget(clear_timer);
}

// ── Timer registry ───────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct TimerEntry {
    id: u64,
    deadline: Instant,
    period: Option<Duration>,
    callback: sys::JSObjectRef,
    ctx: sys::JSContextRef,
}

// BinaryHeap pops the *largest*; we want smallest deadline first.
impl Eq for TimerEntry {}
impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed so the heap behaves as a min-heap by deadline.
        other.deadline.cmp(&self.deadline).then(other.id.cmp(&self.id))
    }
}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

thread_local! {
    static TIMERS: RefCell<BinaryHeap<TimerEntry>> = RefCell::new(BinaryHeap::new());
    static CANCELED: RefCell<Vec<u64>> = RefCell::new(Vec::new());
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn register_timer(
    ctx: &Context,
    callback: sys::JSObjectRef,
    delay: Duration,
    repeating: bool,
) -> u64 {
    // Protect the callback so JSC's GC won't collect it before it fires.
    unsafe {
        sys::JSValueProtect(ctx.as_raw(), callback as sys::JSValueRef);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let entry = TimerEntry {
        id,
        deadline: Instant::now() + delay,
        period: if repeating { Some(delay) } else { None },
        callback,
        ctx: ctx.as_raw(),
    };
    TIMERS.with(|t| t.borrow_mut().push(entry));
    id
}

fn cancel_timer(id: u64) {
    CANCELED.with(|c| c.borrow_mut().push(id));
}

fn is_canceled(id: u64) -> bool {
    CANCELED.with(|c| c.borrow().contains(&id))
}

fn has_pending_timers() -> bool {
    TIMERS.with(|t| !t.borrow().is_empty())
}

/// Drive timer firings + drain async-runtime → JS task queue + service
/// Bun.serve requests, until nothing is in flight.
pub fn run_event_loop(ctx: &Context) {
    loop {
        let timer_did_work = run_one_tick(ctx);
        let async_did_work = crate::async_rt::drain_js_tasks(ctx) > 0;
        let server_did_work =
            crate::bun_api::serve::poll_one(ctx, std::time::Duration::from_millis(20));
        let servers_active = crate::bun_api::serve::any_active();
        let async_pending = crate::async_rt::has_pending_async();
        if !timer_did_work
            && !async_did_work
            && !server_did_work
            && !servers_active
            && !async_pending
        {
            return;
        }
    }
}

/// Fire the next due timer (sleeping until its deadline if needed). Returns
/// `false` when there are no pending timers.
///
/// This is the building block both [`run_event_loop`] and
/// [`crate::modules::await_promise`] sit on top of.
pub fn run_one_tick(ctx: &Context) -> bool {
    loop {
        let next = TIMERS.with(|t| t.borrow_mut().pop());
        let Some(entry) = next else { return false };

        if is_canceled(entry.id) {
            unsafe {
                sys::JSValueUnprotect(entry.ctx, entry.callback as sys::JSValueRef);
            }
            continue;
        }

        let now = Instant::now();
        if entry.deadline > now {
            std::thread::sleep(entry.deadline - now);
        }

        unsafe {
            let mut exc: sys::JSValueRef = std::ptr::null();
            let _ = sys::JSObjectCallAsFunction(
                entry.ctx,
                entry.callback,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                &mut exc,
            );
            if !exc.is_null() {
                let s = sys::JSValueToStringCopy(entry.ctx, exc, std::ptr::null_mut());
                if !s.is_null() {
                    let msg = JsString::adopt(s).to_string();
                    eprintln!("Uncaught (in timer) {msg}");
                } else {
                    eprintln!("Uncaught (in timer) <unstringifiable>");
                }
            }
            let _ = ctx;
        }

        if let Some(period) = entry.period {
            let next_entry = TimerEntry {
                deadline: Instant::now() + period,
                ..entry
            };
            TIMERS.with(|t| t.borrow_mut().push(next_entry));
        } else {
            unsafe {
                sys::JSValueUnprotect(entry.ctx, entry.callback as sys::JSValueRef);
            }
        }
        return true;
    }
}

#[allow(dead_code)]
fn _retain_has_pending() -> bool {
    has_pending_timers()
}
