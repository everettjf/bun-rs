//! `node:readline` — line-based stdin reader.
//!
//! Implemented:
//!   - createInterface({ input, output, terminal })
//!     - interface.question(query, cb)
//!     - interface.on("line", fn) / on("close", fn)
//!     - interface.close()
//!     - interface.write(data)
//!
//! Strategy: a tokio task on the input fd reads lines from stdin and posts
//! each one to the JS thread. Per-process there's only one stdin reader,
//! so we keep a single global task and let user code add listeners.
//!
//! `readline/promises` provides the `await rl.question(...)` form.

use std::io::BufRead;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;

struct Listener {
    on_event: usize, // sys::JSObjectRef as usize (never deref'd off-thread)
    ctx: usize,      // sys::JSGlobalContextRef as usize
}

// SAFETY: the pointers in Listener are only ever cast back and
// dereferenced on the JS thread, after passing through async_rt::post_to_js.
// Storing them as `usize` makes the wrapper Send/Sync without lying about
// the raw types.

static LISTENERS: OnceLock<Mutex<Vec<(u64, Listener)>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static STARTED: OnceLock<()> = OnceLock::new();

fn listeners() -> &'static Mutex<Vec<(u64, Listener)>> {
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// True while at least one readline.Interface is alive — the event loop
/// uses this to stay running so we can deliver lines.
pub fn any_active() -> bool {
    LISTENERS
        .get()
        .map(|m| !m.lock().unwrap().is_empty())
        .unwrap_or(false)
}

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    install_globals(ctx);
    let v = ctx.eval(POLYFILL, Some("[node:readline]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("default", &v).unwrap();
    v
}

fn install_globals(ctx: &Context) {
    let global = ctx.global_object();
    if global
        .get_property("__bun_rl_register")
        .map(|v| !v.is_undefined())
        .unwrap_or(false)
    {
        return;
    }

    let register_cb = Callback::new(ctx, "__bun_rl_register", |args| {
        let on_event_v = args.get(0);
        if !on_event_v.is_object() {
            return Err("__bun_rl_register: missing on_event".into());
        }
        let on_event_obj = on_event_v.to_object().map_err(|e| e.to_string())?;
        unsafe {
            sys::JSValueProtect(args.context().as_raw(), on_event_obj.as_raw() as sys::JSValueRef);
        }
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        listeners().lock().unwrap().push((
            id,
            Listener {
                on_event: on_event_obj.as_raw() as usize,
                ctx: args.context().as_global_raw() as usize,
            },
        ));
        // Start the global stdin pump on first registration.
        start_stdin_pump();
        Ok(Value::new_number(args.context(), id as f64))
    });
    global
        .set_property("__bun_rl_register", &register_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(register_cb);

    let unreg_cb = Callback::new(ctx, "__bun_rl_unregister", |args| {
        let id = args.get(0).to_number() as u64;
        let mut g = listeners().lock().unwrap();
        if let Some(pos) = g.iter().position(|(i, _)| *i == id) {
            let (_, lst) = g.remove(pos);
            unsafe {
                sys::JSValueUnprotect(
                    lst.ctx as sys::JSContextRef,
                    lst.on_event as sys::JSValueRef,
                );
            }
        }
        Ok(Value::new_undefined(args.context()))
    });
    global
        .set_property("__bun_rl_unregister", &unreg_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(unreg_cb);
}

fn start_stdin_pump() {
    STARTED.get_or_init(|| {
        // The stdin reader runs in its own thread (not tokio) because we want
        // a blocking BufRead. Each line is posted to the JS thread.
        std::thread::spawn(|| {
            let stdin = std::io::stdin();
            let mut lock = stdin.lock();
            let mut buf = String::new();
            loop {
                buf.clear();
                match lock.read_line(&mut buf) {
                    Ok(0) => {
                        broadcast(None);
                        return;
                    }
                    Ok(_) => {
                        // Strip trailing newline.
                        let mut line = buf.clone();
                        if line.ends_with('\n') { line.pop(); }
                        if line.ends_with('\r') { line.pop(); }
                        broadcast(Some(line));
                    }
                    Err(_) => {
                        broadcast(None);
                        return;
                    }
                }
            }
        });
    });
}

fn broadcast(line: Option<String>) {
    // Cross-thread transit uses usize so the closure type stays Send.
    let entries: Vec<usize> = listeners()
        .lock()
        .unwrap()
        .iter()
        .map(|(_, l)| l.on_event)
        .collect();
    for on_event_id in entries {
        let line = line.clone();
        crate::async_rt::post_to_js(move |ctx| {
            let raw = on_event_id as sys::JSObjectRef;
            let obj = unsafe { bun_jsc::Object::from_raw_for_runtime(ctx, raw) };
            match line {
                Some(s) => {
                    let kind = Value::new_string(ctx, "line");
                    let payload = Value::new_string(ctx, &s);
                    let _ = obj.call(None, &[kind, payload]);
                }
                None => {
                    let kind = Value::new_string(ctx, "close");
                    let _ = obj.call(None, &[kind, Value::new_undefined(ctx)]);
                }
            }
        });
    }
}

const POLYFILL: &str = r#"
(() => {
  class Interface {
    constructor(opts) {
      opts = opts || {};
      this._output = opts.output || (typeof process !== "undefined" ? process.stdout : null);
      this._listeners = { line: [], close: [] };
      this._questions = [];
      this._closed = false;
      const onEvent = (kind, payload) => {
        if (kind === "line") {
          if (this._questions.length > 0) {
            const q = this._questions.shift();
            q(payload);
            return;
          }
          for (const fn of this._listeners.line.slice()) {
            try { fn(payload); } catch (e) { console.error(e); }
          }
        } else if (kind === "close") {
          this._closed = true;
          for (const fn of this._listeners.close.slice()) {
            try { fn(); } catch (e) { console.error(e); }
          }
        }
      };
      this._id = __bun_rl_register(onEvent);
    }
    on(event, fn) {
      if (this._listeners[event]) this._listeners[event].push(fn);
      return this;
    }
    once(event, fn) {
      const wrap = (...a) => { this.off(event, wrap); fn(...a); };
      return this.on(event, wrap);
    }
    off(event, fn) {
      const list = this._listeners[event];
      if (!list) return this;
      const i = list.indexOf(fn);
      if (i !== -1) list.splice(i, 1);
      return this;
    }
    removeListener(event, fn) { return this.off(event, fn); }
    write(data) {
      if (this._output && typeof this._output.write === "function") {
        this._output.write(data);
      }
    }
    question(query, cb) {
      this.write(query);
      this._questions.push(cb);
    }
    close() {
      if (this._closed) return;
      this._closed = true;
      __bun_rl_unregister(this._id);
      for (const fn of this._listeners.close.slice()) {
        try { fn(); } catch (e) { console.error(e); }
      }
    }
    // Promise-aware question for `readline/promises`.
    questionAsync(query) {
      return new Promise((resolve) => this.question(query, resolve));
    }
  }
  const readline = {
    createInterface(opts) { return new Interface(opts); },
    Interface,
  };
  // Bonus: a "promises" namespace that returns a slightly different
  // interface whose .question returns a Promise.
  readline.promises = {
    createInterface(opts) {
      const i = new Interface(opts);
      // Capture the original cb-style .question, then replace with a
      // Promise-returning version.
      const cbQuestion = i.question.bind(i);
      i.question = (query) => new Promise((resolve) => cbQuestion(query, resolve));
      return i;
    },
  };
  return readline;
})()
"#;
