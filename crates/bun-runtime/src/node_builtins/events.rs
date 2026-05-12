//! `node:events` — pure JS implementation of EventEmitter.
//!
//! Faithful enough for `on/once/off/emit/removeListener/removeAllListeners/
//! listenerCount/listeners/eventNames/setMaxListeners/getMaxListeners`.

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    // The polyfill returns the EventEmitter class itself; that's what
    // node:events exports. `import EE from 'node:events'` should yield
    // the class — so default = the class. Named imports work because
    // EventEmitter.EventEmitter = EventEmitter (set inside the polyfill).
    let class_v = ctx.eval(POLYFILL, Some("[node:events]")).unwrap();
    let class_obj = class_v.to_object().unwrap();
    // Attach default = self for ESM default-import.
    class_obj.set_property("default", &class_v).unwrap();
    class_v
}

const POLYFILL: &str = r#"
(() => {
  class EventEmitter {
    constructor() {
      this._events = Object.create(null);
      this._maxListeners = 10;
    }
    on(event, fn) {
      const list = this._events[event] || (this._events[event] = []);
      list.push(fn);
      if (list.length > this._maxListeners) {
        // Match Node's warning shape but route to stderr.
        // (No process.emitWarning yet.)
        console.warn("MaxListenersExceededWarning: " + list.length + " listeners for " + event);
      }
      return this;
    }
    addListener(event, fn) { return this.on(event, fn); }
    once(event, fn) {
      const wrap = (...args) => { this.off(event, wrap); fn.apply(this, args); };
      wrap._original = fn;
      return this.on(event, wrap);
    }
    off(event, fn) {
      const list = this._events[event];
      if (!list) return this;
      const idx = list.findIndex(f => f === fn || f._original === fn);
      if (idx !== -1) list.splice(idx, 1);
      if (list.length === 0) delete this._events[event];
      return this;
    }
    removeListener(event, fn) { return this.off(event, fn); }
    removeAllListeners(event) {
      if (event === undefined) this._events = Object.create(null);
      else delete this._events[event];
      return this;
    }
    emit(event, ...args) {
      const list = this._events[event];
      if (!list || list.length === 0) {
        if (event === "error") {
          throw args[0] != null ? args[0] : new Error("Unhandled 'error' event");
        }
        return false;
      }
      // Copy because handlers may mutate the list (e.g. once removes self).
      for (const fn of list.slice()) {
        try { fn.apply(this, args); }
        catch (e) { console.error("EventEmitter handler threw:", e); }
      }
      return true;
    }
    listeners(event) { return (this._events[event] || []).slice(); }
    listenerCount(event) { return (this._events[event] || []).length; }
    eventNames() { return Object.keys(this._events); }
    setMaxListeners(n) { this._maxListeners = n; return this; }
    getMaxListeners() { return this._maxListeners; }
    prependListener(event, fn) {
      const list = this._events[event] || (this._events[event] = []);
      list.unshift(fn);
      return this;
    }
    prependOnceListener(event, fn) {
      const wrap = (...args) => { this.off(event, wrap); fn.apply(this, args); };
      wrap._original = fn;
      return this.prependListener(event, wrap);
    }
  }
  EventEmitter.EventEmitter = EventEmitter;
  EventEmitter.defaultMaxListeners = 10;
  return EventEmitter;
})()
"#;
