//! `node:stream` — Readable / Writable / Duplex / PassThrough.
//!
//! Pure JS implementation as EventEmitter subclasses. The API is the
//! flowing-mode subset (push chunks → emit 'data' / 'end'; write chunks →
//! emit 'drain' / 'finish'). Object mode (chunks are non-Uint8Array) is
//! supported. No `_construct` / `_writev` / `_destroy` lifecycle hooks
//! beyond the basics.
//!
//! Web-Streams interop:
//!   `Readable.toWeb(node_readable)` → ReadableStream
//!   `Readable.fromWeb(web_readable)` → Readable
//!   (same for Writable)

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval(POLYFILL, Some("[node:stream]")).unwrap();
    let exports = exports_v.to_object().unwrap();
    exports.set_property("default", &exports_v).unwrap();

    // Expose Readable / Writable on globalThis so the Rust side of
    // fs.createReadStream / fs.createWriteStream can `new` them without
    // re-routing through __bun_require.
    let global = ctx.global_object();
    if let Ok(r) = exports.get_property("Readable") {
        let _ = global.set_property("__bun_NodeReadable", &r);
    }
    if let Ok(w) = exports.get_property("Writable") {
        let _ = global.set_property("__bun_NodeWritable", &w);
    }

    exports_v
}

/// Pre-install the global helpers even if user code hasn't imported
/// node:stream yet, so `fs.createReadStream` works on its own.
pub fn install_globals(ctx: &Context) {
    // Trigger the module build (which sets the globals via build()).
    let _ = build(ctx);
}

const POLYFILL: &str = r#"
(() => {
  const EventEmitter = (() => {
    // Local EE so we don't depend on require("node:events") here.
    class EE {
      constructor() { this._events = Object.create(null); }
      on(e, f) { (this._events[e] || (this._events[e] = [])).push(f); return this; }
      addListener(e, f) { return this.on(e, f); }
      once(e, f) {
        const w = (...a) => { this.off(e, w); f.apply(this, a); };
        w._original = f;
        return this.on(e, w);
      }
      off(e, f) {
        const list = this._events[e];
        if (!list) return this;
        const i = list.findIndex(g => g === f || g._original === f);
        if (i !== -1) list.splice(i, 1);
        if (list.length === 0) delete this._events[e];
        return this;
      }
      removeListener(e, f) { return this.off(e, f); }
      removeAllListeners(e) {
        if (e === undefined) this._events = Object.create(null);
        else delete this._events[e];
        return this;
      }
      emit(e, ...args) {
        const list = this._events[e];
        if (!list || list.length === 0) {
          if (e === "error") {
            // Match Node: throw if no error listener.
            throw args[0] != null ? args[0] : new Error("Unhandled 'error' event");
          }
          return false;
        }
        for (const fn of list.slice()) { try { fn.apply(this, args); } catch (err) { console.error(err); } }
        return true;
      }
      listenerCount(e) { return (this._events[e] || []).length; }
    }
    return EE;
  })();

  class Readable extends EventEmitter {
    constructor(opts) {
      super();
      opts = opts || {};
      this._opts = opts;
      this._buf = [];
      this._reading = false;
      this._ended = false;
      this._closed = false;
      this._objectMode = !!opts.objectMode;
      this._encoding = opts.encoding || null;
      this._flowing = null;        // null = paused, false = paused (explicit), true = flowing
      this._readable = true;
      this.readable = true;
      this.destroyed = false;
      // When user supplies `read()`, route to it. Otherwise expect push() from outside.
      if (typeof opts.read === "function") this._read = opts.read;
    }
    // Override on() so adding a 'data' listener auto-resumes (Node semantics).
    on(event, fn) {
      const r = super.on(event, fn);
      if (event === "data" && this._flowing === null) this.resume();
      return r;
    }
    addListener(event, fn) { return this.on(event, fn); }
    _read(_n) { /* override in opts.read */ }
    push(chunk, encoding) {
      if (chunk === null) {
        this._ended = true;
        if (this._buf.length === 0) this._endStream();
        else if (this._flowing) this._drain();
        return false;
      }
      if (this._objectMode || chunk instanceof Uint8Array || typeof chunk === "string") {
        this._buf.push(chunk);
        if (this._flowing) this._drain();
        return this._buf.length < (this._opts.highWaterMark || 16);
      }
      this.emit("error", new TypeError("Invalid chunk type"));
      return false;
    }
    _drain() {
      while (this._buf.length > 0) {
        const c = this._buf.shift();
        this.emit("data", c);
      }
      if (this._ended) this._endStream();
    }
    _endStream() {
      if (this._closed) return;
      this._closed = true;
      this.readable = false;
      this.emit("end");
      queueMicrotask(() => this.emit("close"));
    }
    resume() {
      this._flowing = true;
      if (this._buf.length > 0) this._drain();
      else if (!this._ended) {
        try { this._read(this._opts.highWaterMark || 16); } catch (e) { this.emit("error", e); }
      } else this._endStream();
      return this;
    }
    pause() { this._flowing = false; return this; }
    read(_n) {
      if (this._buf.length > 0) return this._buf.shift();
      if (!this._ended && !this._reading) {
        this._reading = true;
        try { this._read(this._opts.highWaterMark || 16); } catch (e) { this.emit("error", e); }
        this._reading = false;
      }
      return this._buf.length > 0 ? this._buf.shift() : null;
    }
    pipe(dest, opts) {
      opts = opts || {};
      this.on("data", (c) => {
        if (!dest.write(c) && this.pause) this.pause();
      });
      this.on("end", () => { if (opts.end !== false) dest.end(); });
      this.on("error", (e) => dest.emit && dest.emit("error", e));
      if (dest.on) dest.on("drain", () => this.resume && this.resume());
      this.resume();
      return dest;
    }
    destroy(err) {
      if (this.destroyed) return this;
      this.destroyed = true;
      this._buf = [];
      if (err) this.emit("error", err);
      this.emit("close");
      return this;
    }
    // Native async iteration: drains the buffer one chunk at a time.
    async *[Symbol.asyncIterator]() {
      const self = this;
      let resolveNext, rejectNext;
      let pendingChunks = [];
      let ended = false;
      let errored = null;
      self.on("data", (c) => {
        if (resolveNext) { const r = resolveNext; resolveNext = null; r({ value: c, done: false }); }
        else pendingChunks.push(c);
      });
      self.on("end", () => {
        ended = true;
        if (resolveNext) { resolveNext({ value: undefined, done: true }); resolveNext = null; }
      });
      self.on("error", (e) => {
        errored = e;
        if (rejectNext) { rejectNext(e); rejectNext = null; }
      });
      self.resume();
      while (true) {
        if (errored) throw errored;
        if (pendingChunks.length) { yield pendingChunks.shift(); continue; }
        if (ended) return;
        const next = await new Promise((res, rej) => { resolveNext = res; rejectNext = rej; });
        if (next.done) return;
        yield next.value;
      }
    }
    static from(iterableOrAsync) {
      const r = new Readable({ objectMode: true });
      (async () => {
        try {
          for await (const c of iterableOrAsync) r.push(c);
          r.push(null);
        } catch (e) { r.destroy(e); }
      })();
      return r;
    }
    static toWeb(node_readable) {
      return new ReadableStream({
        start(controller) {
          node_readable.on("data", (c) => controller.enqueue(c));
          node_readable.on("end", () => controller.close());
          node_readable.on("error", (e) => controller.error(e));
        },
        cancel(reason) { node_readable.destroy(reason); },
      });
    }
    static fromWeb(web_stream) {
      const r = new Readable({ objectMode: true });
      (async () => {
        try { for await (const c of web_stream) r.push(c); r.push(null); }
        catch (e) { r.destroy(e); }
      })();
      return r;
    }
  }

  class Writable extends EventEmitter {
    constructor(opts) {
      super();
      opts = opts || {};
      this._opts = opts;
      this._writing = false;
      this._queue = [];
      this._ended = false;
      this._finished = false;
      this.writable = true;
      this.destroyed = false;
      if (typeof opts.write === "function") this._write = opts.write;
    }
    _write(_chunk, _enc, cb) { cb(); }
    write(chunk, encoding, cb) {
      if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
      if (this._ended) {
        const err = new Error("Cannot write after end");
        if (cb) cb(err); else this.emit("error", err);
        return false;
      }
      this._queue.push({ chunk, encoding, cb });
      this._pump();
      return this._queue.length < (this._opts.highWaterMark || 16);
    }
    _pump() {
      if (this._writing) return;
      if (this._queue.length === 0) {
        if (this._ended) this._finish();
        return;
      }
      this._writing = true;
      const next = () => {
        const item = this._queue.shift();
        if (!item) {
          this._writing = false;
          this.emit("drain");
          if (this._ended && this._queue.length === 0) this._finish();
          return;
        }
        try {
          this._write(item.chunk, item.encoding, (err) => {
            if (err) {
              if (item.cb) item.cb(err);
              this.emit("error", err);
              return;
            }
            if (item.cb) item.cb();
            next();
          });
        } catch (e) {
          if (item.cb) item.cb(e);
          this.emit("error", e);
        }
      };
      next();
    }
    end(chunk, encoding, cb) {
      if (typeof chunk === "function") { cb = chunk; chunk = undefined; }
      if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
      if (chunk !== undefined && chunk !== null) {
        if (cb) this.write(chunk, encoding, () => { this._ended = true; this._pump(); cb(); });
        else { this.write(chunk, encoding); this._ended = true; }
      } else {
        this._ended = true;
        if (cb) this.once("finish", cb);
      }
      if (!this._writing) this._pump();
      return this;
    }
    _finish() {
      if (this._finished) return;
      this._finished = true;
      this.writable = false;
      this.emit("finish");
      queueMicrotask(() => this.emit("close"));
    }
    destroy(err) {
      if (this.destroyed) return this;
      this.destroyed = true;
      this._queue = [];
      if (err) this.emit("error", err);
      this.emit("close");
      return this;
    }
    static toWeb(node_writable) {
      return new WritableStream({
        write(chunk) {
          return new Promise((resolve, reject) => {
            if (!node_writable.write(chunk, undefined, (e) => e ? reject(e) : resolve())) {
              node_writable.once("drain", resolve);
            } else { resolve(); }
          });
        },
        close() { return new Promise(r => node_writable.end(r)); },
        abort(reason) { node_writable.destroy(reason); },
      });
    }
    static fromWeb(web_writable) {
      const writer = web_writable.getWriter();
      return new Writable({
        write(chunk, _enc, cb) {
          writer.write(chunk).then(() => cb(), cb);
        },
      });
    }
  }

  // Duplex inherits from Readable and inlines its own writable state, so
  // subclasses (like PassThrough) can override `_write` cleanly on the
  // prototype without the inner Writable's default _write clobbering it.
  class Duplex extends Readable {
    constructor(opts) {
      super(opts);
      this._wQueue = [];
      this._wWriting = false;
      this._wEnded = false;
      this._wFinished = false;
      this.writable = true;
      if (opts && typeof opts.write === "function") this._write = opts.write;
    }
    _write(_chunk, _encoding, cb) { cb(); }
    write(chunk, encoding, cb) {
      if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
      if (this._wEnded) {
        const err = new Error("Cannot write after end");
        if (cb) cb(err); else this.emit("error", err);
        return false;
      }
      this._wQueue.push({ chunk, encoding, cb });
      this._wPump();
      return this._wQueue.length < ((this._opts && this._opts.highWaterMark) || 16);
    }
    _wPump() {
      if (this._wWriting) return;
      if (this._wQueue.length === 0) {
        if (this._wEnded && !this._wFinished) {
          this._wFinished = true;
          this.writable = false;
          this.emit("finish");
        }
        return;
      }
      this._wWriting = true;
      const next = () => {
        const item = this._wQueue.shift();
        if (!item) {
          this._wWriting = false;
          this.emit("drain");
          if (this._wEnded && !this._wFinished) {
            this._wFinished = true;
            this.writable = false;
            this.emit("finish");
          }
          return;
        }
        try {
          this._write(item.chunk, item.encoding, (err) => {
            if (err) {
              if (item.cb) item.cb(err);
              this.emit("error", err);
              return;
            }
            if (item.cb) item.cb();
            next();
          });
        } catch (e) {
          if (item.cb) item.cb(e);
          this.emit("error", e);
        }
      };
      next();
    }
    end(chunk, encoding, cb) {
      if (typeof chunk === "function") { cb = chunk; chunk = undefined; }
      if (typeof encoding === "function") { cb = encoding; encoding = undefined; }
      if (chunk != null) this.write(chunk, encoding);
      this._wEnded = true;
      if (cb) this.once("finish", cb);
      this._wPump();
      return this;
    }
  }

  class PassThrough extends Duplex {
    _write(chunk, _encoding, cb) {
      this.push(chunk);
      cb();
    }
  }

  // Module exports
  const stream = {
    Readable, Writable, Duplex, PassThrough, Transform: Duplex,
    finished(s, cb) {
      // Resolves once the stream finishes / errors / closes.
      const done = (err) => { if (cb) cb(err); };
      let called = false;
      const wrap = (err) => { if (!called) { called = true; done(err); } };
      if (s.on) {
        s.on("end", () => wrap());
        s.on("finish", () => wrap());
        s.on("close", () => wrap());
        s.on("error", wrap);
      }
    },
    pipeline(...streams) {
      // last arg may be callback
      let cb = streams[streams.length - 1];
      if (typeof cb === "function") streams = streams.slice(0, -1);
      else cb = null;
      try {
        for (let i = 0; i < streams.length - 1; i++) streams[i].pipe(streams[i + 1]);
        const last = streams[streams.length - 1];
        if (last.on) {
          last.on("finish", () => cb && cb(null));
          last.on("end", () => cb && cb(null));
          last.on("error", (e) => cb && cb(e));
        }
      } catch (e) { if (cb) cb(e); }
      return streams[streams.length - 1];
    },
  };
  return stream;
})()
"#;
