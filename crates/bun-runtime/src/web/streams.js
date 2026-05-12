// bun-rs Web Streams polyfill — subset of the WHATWG Streams Standard.
//
// What's covered:
//   - ReadableStream (constructor with start/pull/cancel, getReader,
//     cancel, tee, locked, pipeTo, pipeThrough, ReadableStream.from)
//   - WritableStream (constructor with start/write/close/abort,
//     getWriter, locked, abort, close)
//   - TransformStream (start/transform/flush, .readable, .writable)
//   - ReadableStreamDefaultReader (read, cancel, releaseLock, closed)
//   - WritableStreamDefaultWriter (write, close, abort, releaseLock,
//     ready, closed, desiredSize)
//   - Default queuing strategies (CountQueuingStrategy with high-water-mark)
//
// What's NOT covered:
//   - BYOB readers
//   - Byte streams (UnderlyingByteSource / ReadableByteStreamController)
//   - Adoptive pipe semantics under back-pressure quirks
//   - All error / abort propagation edge cases (we do the common path)

(function (g) {
  // ─── Internal helpers ──────────────────────────────────────────────
  const $ = Symbol("bunrs-stream-internal");

  function deferred() {
    let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }

  function makeError(msg) { return new TypeError(msg); }

  // ─── ReadableStream ────────────────────────────────────────────────
  class ReadableStream {
    constructor(underlying, strategy) {
      underlying = underlying || {};
      strategy = strategy || {};
      const hwm = strategy.highWaterMark == null ? 1 : strategy.highWaterMark;
      const sizeFn = strategy.size;

      this[$] = {
        state: "readable",         // "readable" | "closed" | "errored"
        storedError: null,
        queue: [],                 // pending chunks
        queueSize: 0,
        hwm,
        sizeFn,
        // pendingReads: queue of deferreds for read() when no chunks
        pendingReads: [],
        // closeRequested: writer requested close, drain remaining then signal
        closeRequested: false,
        reader: null,
        underlying,
        pulling: false,
      };

      this[$].controller = new ReadableStreamDefaultController(this);

      try {
        const r = underlying.start && underlying.start(this[$].controller);
        if (r && typeof r.then === "function") {
          r.then(() => this[$].controller._pullIfNeeded(),
                 (e) => this[$].controller.error(e));
        } else {
          this[$].controller._pullIfNeeded();
        }
      } catch (e) {
        this[$].controller.error(e);
      }
    }

    get locked() { return this[$].reader !== null; }

    getReader(opts) {
      if (opts && opts.mode === "byob") throw makeError("BYOB readers not supported");
      if (this[$].reader) throw makeError("ReadableStream is already locked");
      return new ReadableStreamDefaultReader(this);
    }

    cancel(reason) {
      if (this[$].state === "errored") return Promise.reject(this[$].storedError);
      return this[$].controller._cancel(reason);
    }

    tee() {
      // Simple tee: buffer everything and replay to two new streams.
      const buffer = [];
      const reader = this.getReader();
      let done = false;
      let error = null;
      const pumpers = [null, null];

      async function pump() {
        try {
          while (true) {
            const { value, done: d } = await reader.read();
            if (d) {
              done = true;
              for (const p of pumpers) if (p) p.close();
              return;
            }
            buffer.push(value);
            for (const p of pumpers) if (p) p.enqueue(value);
          }
        } catch (e) {
          error = e;
          done = true;
          for (const p of pumpers) if (p) p.error(e);
        }
      }
      pump();

      const make = (idx) => new ReadableStream({
        start(controller) {
          pumpers[idx] = controller;
        },
      });
      return [make(0), make(1)];
    }

    pipeTo(dest, opts) {
      opts = opts || {};
      const reader = this.getReader();
      const writer = dest.getWriter();
      return (async () => {
        try {
          while (true) {
            const { value, done } = await reader.read();
            if (done) break;
            await writer.write(value);
          }
          if (!opts.preventClose) await writer.close();
        } catch (e) {
          if (!opts.preventAbort) await writer.abort(e);
          throw e;
        } finally {
          try { reader.releaseLock(); } catch {}
          try { writer.releaseLock(); } catch {}
        }
      })();
    }

    pipeThrough(transform, opts) {
      // {readable, writable} on the transform.
      this.pipeTo(transform.writable, opts).catch(() => {});
      return transform.readable;
    }

    static from(asyncIterable) {
      let it;
      if (typeof asyncIterable[Symbol.asyncIterator] === "function") {
        it = asyncIterable[Symbol.asyncIterator]();
      } else if (typeof asyncIterable[Symbol.iterator] === "function") {
        const sync = asyncIterable[Symbol.iterator]();
        it = {
          next() { return Promise.resolve(sync.next()); },
          return(v) { return Promise.resolve(sync.return ? sync.return(v) : { value: v, done: true }); },
        };
      } else {
        throw makeError("ReadableStream.from requires an (async) iterable");
      }
      return new ReadableStream({
        async pull(controller) {
          try {
            const { value, done } = await it.next();
            if (done) controller.close();
            else controller.enqueue(value);
          } catch (e) {
            controller.error(e);
          }
        },
        cancel(reason) {
          if (it.return) return it.return(reason);
        },
      });
    }

    // Async-iterator protocol — `for await (const chunk of stream)`.
    [Symbol.asyncIterator]() {
      const reader = this.getReader();
      return {
        next: () => reader.read(),
        return: async (v) => { reader.releaseLock(); return { value: v, done: true }; },
        [Symbol.asyncIterator]() { return this; },
      };
    }
  }

  class ReadableStreamDefaultController {
    constructor(stream) { this._stream = stream; }
    get desiredSize() {
      const s = this._stream[$];
      if (s.state === "errored") return null;
      if (s.state === "closed") return 0;
      return s.hwm - s.queueSize;
    }
    enqueue(chunk) {
      const s = this._stream[$];
      if (s.state !== "readable" || s.closeRequested) throw makeError("controller not active");
      // Hand to a pending reader if there is one.
      if (s.pendingReads.length > 0) {
        const d = s.pendingReads.shift();
        d.resolve({ value: chunk, done: false });
        return;
      }
      const size = s.sizeFn ? s.sizeFn(chunk) : 1;
      s.queue.push(chunk);
      s.queueSize += size;
      this._pullIfNeeded();
    }
    close() {
      const s = this._stream[$];
      if (s.state !== "readable") return;
      s.closeRequested = true;
      // If queue empty AND no pending reads, transition to closed.
      this._finalizeIfDone();
    }
    error(e) {
      const s = this._stream[$];
      if (s.state !== "readable") return;
      s.state = "errored";
      s.storedError = e;
      while (s.pendingReads.length) s.pendingReads.shift().reject(e);
    }
    _finalizeIfDone() {
      const s = this._stream[$];
      if (s.closeRequested && s.queue.length === 0) {
        s.state = "closed";
        while (s.pendingReads.length) {
          s.pendingReads.shift().resolve({ value: undefined, done: true });
        }
      }
    }
    _pullIfNeeded() {
      const s = this._stream[$];
      if (s.state !== "readable" || s.closeRequested || s.pulling) return;
      if (s.queueSize >= s.hwm) return;
      if (!s.underlying.pull) return;
      s.pulling = true;
      try {
        const r = s.underlying.pull(this);
        Promise.resolve(r).then(() => {
          s.pulling = false;
          this._pullIfNeeded();
        }, (e) => {
          s.pulling = false;
          this.error(e);
        });
      } catch (e) {
        s.pulling = false;
        this.error(e);
      }
    }
    _cancel(reason) {
      const s = this._stream[$];
      s.queue = [];
      s.queueSize = 0;
      s.state = "closed";
      while (s.pendingReads.length) {
        s.pendingReads.shift().resolve({ value: undefined, done: true });
      }
      if (s.underlying.cancel) {
        try { return Promise.resolve(s.underlying.cancel(reason)); }
        catch (e) { return Promise.reject(e); }
      }
      return Promise.resolve();
    }
  }

  class ReadableStreamDefaultReader {
    constructor(stream) {
      this._stream = stream;
      stream[$].reader = this;
      this._closedDeferred = deferred();
      // If stream already closed/errored, resolve immediately.
      if (stream[$].state === "closed") this._closedDeferred.resolve();
      else if (stream[$].state === "errored") this._closedDeferred.reject(stream[$].storedError);
    }
    get closed() { return this._closedDeferred.promise; }

    read() {
      if (!this._stream) return Promise.reject(makeError("reader released"));
      const s = this._stream[$];
      if (s.queue.length > 0) {
        const chunk = s.queue.shift();
        s.queueSize -= s.sizeFn ? s.sizeFn(chunk) : 1;
        // Pull more if behind the hwm.
        s.controller._pullIfNeeded();
        s.controller._finalizeIfDone();
        return Promise.resolve({ value: chunk, done: false });
      }
      if (s.state === "closed") return Promise.resolve({ value: undefined, done: true });
      if (s.state === "errored") return Promise.reject(s.storedError);
      // Queue a pending read.
      const d = deferred();
      s.pendingReads.push(d);
      s.controller._pullIfNeeded();
      return d.promise;
    }
    cancel(reason) {
      if (!this._stream) return Promise.reject(makeError("reader released"));
      return this._stream.cancel(reason);
    }
    releaseLock() {
      if (!this._stream) return;
      this._stream[$].reader = null;
      this._stream = null;
      this._closedDeferred.reject(makeError("reader released"));
    }
  }

  // ─── WritableStream ────────────────────────────────────────────────
  class WritableStream {
    constructor(underlying, strategy) {
      underlying = underlying || {};
      strategy = strategy || {};
      const hwm = strategy.highWaterMark == null ? 1 : strategy.highWaterMark;
      const sizeFn = strategy.size;

      this[$] = {
        state: "writable",   // writable | closed | erroring | errored
        storedError: null,
        queue: [],           // [{chunk, deferred}]
        inFlight: false,
        writer: null,
        underlying,
        sizeFn,
        hwm,
        size: 0,
      };
      this[$].controller = { error: (e) => this[$].$error(e) };

      this[$].$error = (e) => {
        if (this[$].state !== "writable") return;
        this[$].state = "errored";
        this[$].storedError = e;
        for (const { deferred: d } of this[$].queue) d.reject(e);
        this[$].queue = [];
      };

      setupWritablePump(this);

      try {
        const r = underlying.start && underlying.start(this[$].controller);
        if (r && typeof r.then === "function") {
          r.catch((e) => this[$].$error(e));
        }
      } catch (e) {
        this[$].$error(e);
      }
    }
    get locked() { return this[$].writer !== null; }
    getWriter() {
      if (this[$].writer) throw makeError("WritableStream is locked");
      return new WritableStreamDefaultWriter(this);
    }
    abort(reason) {
      const s = this[$];
      if (s.state !== "writable") {
        return s.state === "errored" ? Promise.reject(s.storedError) : Promise.resolve();
      }
      s.$error(reason || makeError("aborted"));
      if (s.underlying.abort) {
        return Promise.resolve(s.underlying.abort(reason)).catch(() => {});
      }
      return Promise.resolve();
    }
    close() {
      const s = this[$];
      if (s.state !== "writable") {
        return Promise.reject(makeError("not writable"));
      }
      // Wait for queue to drain, then call underlying.close.
      const drain = deferred();
      const finalize = async () => {
        if (s.underlying.close) {
          try { await s.underlying.close(); } catch (e) { s.$error(e); drain.reject(e); return; }
        }
        s.state = "closed";
        drain.resolve();
      };
      if (s.queue.length === 0 && !s.inFlight) finalize();
      else {
        // Hook drain check: enqueue a sentinel that fires `finalize` when reached.
        s.queue.push({ chunk: undefined, deferred: { resolve: finalize, reject: drain.reject }, _close: true });
        this[$].$pump();
      }
      return drain.promise;
    }

  }

  // The pump function is monkey-patched onto the internal state below.
  function setupWritablePump(ws) {
    ws[$].$pump = async function() {
      const s = ws[$];
      if (s.inFlight) return;
      s.inFlight = true;
      try {
        while (s.queue.length > 0) {
          const item = s.queue[0];
          if (item._close) {
            s.queue.shift();
            await item.deferred.resolve();
            continue;
          }
          if (s.state !== "writable") {
            item.deferred.reject(s.storedError || makeError("not writable"));
            s.queue.shift();
            continue;
          }
          try {
            if (s.underlying.write) await s.underlying.write(item.chunk, s.controller);
            item.deferred.resolve();
          } catch (e) {
            s.$error(e);
            item.deferred.reject(e);
          }
          s.queue.shift();
          s.size -= s.sizeFn ? s.sizeFn(item.chunk) : 1;
        }
      } finally {
        s.inFlight = false;
      }
    };
  }

  class WritableStreamDefaultWriter {
    constructor(ws) {
      this._stream = ws;
      ws[$].writer = this;
      this._closedDeferred = deferred();
      this._readyDeferred = deferred();
      this._readyDeferred.resolve();
      if (ws[$].state === "errored") {
        this._closedDeferred.reject(ws[$].storedError);
      }
    }
    get closed() { return this._closedDeferred.promise; }
    get ready() { return this._readyDeferred.promise; }
    get desiredSize() {
      const s = this._stream && this._stream[$];
      if (!s) return null;
      if (s.state === "errored" || s.state === "erroring") return null;
      if (s.state === "closed") return 0;
      return s.hwm - s.size;
    }
    write(chunk) {
      if (!this._stream) return Promise.reject(makeError("writer released"));
      const s = this._stream[$];
      if (s.state !== "writable") {
        return s.state === "errored" ? Promise.reject(s.storedError) : Promise.reject(makeError("not writable"));
      }
      const d = deferred();
      s.queue.push({ chunk, deferred: d });
      s.size += s.sizeFn ? s.sizeFn(chunk) : 1;
      s.$pump();
      return d.promise;
    }
    close() {
      if (!this._stream) return Promise.reject(makeError("writer released"));
      const p = this._stream.close();
      p.then(() => this._closedDeferred.resolve(), (e) => this._closedDeferred.reject(e));
      return p;
    }
    abort(reason) {
      if (!this._stream) return Promise.reject(makeError("writer released"));
      return this._stream.abort(reason);
    }
    releaseLock() {
      if (!this._stream) return;
      this._stream[$].writer = null;
      this._stream = null;
    }
  }

  // ─── TransformStream ───────────────────────────────────────────────
  class TransformStream {
    constructor(transformer, writableStrategy, readableStrategy) {
      transformer = transformer || {};
      let outController;
      const readable = new ReadableStream({
        start(controller) { outController = controller; },
      }, readableStrategy);

      const writable = new WritableStream({
        async start() {
          if (transformer.start) await transformer.start({ enqueue: (c) => outController.enqueue(c) });
        },
        async write(chunk) {
          const controller = { enqueue: (c) => outController.enqueue(c) };
          if (transformer.transform) await transformer.transform(chunk, controller);
          else outController.enqueue(chunk);
        },
        async close() {
          const controller = { enqueue: (c) => outController.enqueue(c) };
          if (transformer.flush) await transformer.flush(controller);
          outController.close();
        },
        async abort(reason) {
          outController.error(reason);
        },
      }, writableStrategy);

      this.readable = readable;
      this.writable = writable;
    }
  }

  // ─── Queuing strategies ───────────────────────────────────────────
  class CountQueuingStrategy {
    constructor({ highWaterMark }) { this.highWaterMark = highWaterMark; }
    size() { return 1; }
  }
  class ByteLengthQueuingStrategy {
    constructor({ highWaterMark }) { this.highWaterMark = highWaterMark; }
    size(chunk) { return chunk && chunk.byteLength != null ? chunk.byteLength : 0; }
  }

  g.ReadableStream = ReadableStream;
  g.WritableStream = WritableStream;
  g.TransformStream = TransformStream;
  g.ReadableStreamDefaultReader = ReadableStreamDefaultReader;
  g.WritableStreamDefaultWriter = WritableStreamDefaultWriter;
  g.CountQueuingStrategy = CountQueuingStrategy;
  g.ByteLengthQueuingStrategy = ByteLengthQueuingStrategy;
})(globalThis);
