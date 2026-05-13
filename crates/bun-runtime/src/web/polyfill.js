// bun-rs Web Platform polyfill (subset of WHATWG fetch + URL).
// Installed once at runtime startup. Pure JS — no module loader involvement.
//
// Surface:
//   URL, URLSearchParams (using Rust-side __bun_parse_url for parsing)
//   Headers (case-insensitive map)
//   Request, Response (data containers + body methods)
//   fetch(url, init?) → Promise<Response> (Rust __bun_fetch under the hood)

(function (g) {
  // ────────────────────────── URLSearchParams ──────────────────────────
  class URLSearchParams {
    constructor(init) {
      this._params = [];
      if (init == null) return;
      if (typeof init === "string") {
        this._parseString(init.replace(/^\?/, ""));
      } else if (Array.isArray(init)) {
        for (const [k, v] of init) this.append(k, v);
      } else if (init instanceof URLSearchParams) {
        for (const [k, v] of init._params) this.append(k, v);
      } else if (typeof init === "object") {
        for (const k of Object.keys(init)) this.append(k, init[k]);
      }
    }
    _parseString(s) {
      if (!s) return;
      for (const pair of s.split("&")) {
        const eq = pair.indexOf("=");
        if (eq === -1) this.append(decodeURIComponent(pair), "");
        else this.append(decodeURIComponent(pair.slice(0, eq).replace(/\+/g, " ")),
                          decodeURIComponent(pair.slice(eq + 1).replace(/\+/g, " ")));
      }
    }
    append(k, v) { this._params.push([String(k), String(v)]); }
    set(k, v) { this.delete(k); this.append(k, v); }
    delete(k) { this._params = this._params.filter(p => p[0] !== k); }
    get(k) { const p = this._params.find(p => p[0] === k); return p ? p[1] : null; }
    getAll(k) { return this._params.filter(p => p[0] === k).map(p => p[1]); }
    has(k) { return this._params.some(p => p[0] === k); }
    *entries() { for (const p of this._params) yield p; }
    *keys() { for (const p of this._params) yield p[0]; }
    *values() { for (const p of this._params) yield p[1]; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(cb, thisArg) { for (const [k, v] of this._params) cb.call(thisArg, v, k, this); }
    toString() {
      return this._params
        .map(([k, v]) => encodeURIComponent(k) + "=" + encodeURIComponent(v))
        .join("&");
    }
    get size() { return this._params.length; }
  }
  g.URLSearchParams = URLSearchParams;

  // ───────────────────────────── URL ─────────────────────────────
  class URL {
    constructor(input, base) {
      const r = __bun_parse_url(String(input), base != null ? String(base) : undefined);
      this._r = r;
    }
    get href() { return this._r.href; }
    set href(v) {
      const r = __bun_parse_url(String(v));
      this._r = r;
    }
    get origin() { return this._r.origin; }
    get protocol() { return this._r.protocol; }
    set protocol(v) { /* ignored: re-parse not implemented */ }
    get host() { return this._r.host; }
    set host(v) { /* ignored */ }
    get hostname() { return this._r.hostname; }
    set hostname(v) { /* ignored */ }
    get port() { return this._r.port; }
    set port(v) { /* ignored */ }
    get pathname() { return this._r.pathname; }
    set pathname(v) { /* ignored */ }
    get search() { return this._r.search; }
    set search(v) { /* ignored */ }
    get hash() { return this._r.hash; }
    set hash(v) { /* ignored */ }
    get username() { return this._r.username; }
    get password() { return this._r.password; }
    get searchParams() {
      if (!this._sp) {
        this._sp = new URLSearchParams(this._r.search);
      }
      return this._sp;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }
  g.URL = URL;

  // ─────────────────────────── Headers ───────────────────────────
  class Headers {
    constructor(init) {
      this._map = new Map();
      if (init == null) return;
      if (init instanceof Headers) {
        for (const [k, v] of init._map) this._map.set(k, v);
      } else if (Array.isArray(init)) {
        for (const [k, v] of init) this.append(k, v);
      } else if (typeof init === "object") {
        for (const k of Object.keys(init)) this.append(k, init[k]);
      }
    }
    _normName(k) { return String(k).toLowerCase(); }
    append(k, v) {
      const key = this._normName(k);
      const existing = this._map.get(key);
      this._map.set(key, existing == null ? String(v) : existing + ", " + v);
    }
    delete(k) { this._map.delete(this._normName(k)); }
    get(k) {
      const v = this._map.get(this._normName(k));
      return v === undefined ? null : v;
    }
    has(k) { return this._map.has(this._normName(k)); }
    set(k, v) { this._map.set(this._normName(k), String(v)); }
    *entries() { for (const e of this._map.entries()) yield e; }
    *keys() { for (const k of this._map.keys()) yield k; }
    *values() { for (const v of this._map.values()) yield v; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(cb, thisArg) { for (const [k, v] of this._map) cb.call(thisArg, v, k, this); }
  }
  g.Headers = Headers;

  // ─────────────────────────── Body mixin ───────────────────────────
  // `bodyText` is the textified body; `bodyBytes` (optional) is a
  // Uint8Array view over the raw bytes (used by fetch for binary).
  function makeBody(bodyText, bodyBytes) {
    // Normalize: if the caller passed a Uint8Array (or Buffer) as the body,
    // decode it to a UTF-8 string for .text(). Pass the original bytes
    // along separately via bodyBytes so .bytes() / .arrayBuffer() stay
    // zero-copy.
    let textForm;
    if (bodyText == null) {
      textForm = "";
    } else if (typeof bodyText === "string") {
      textForm = bodyText;
    } else if (bodyText instanceof Uint8Array) {
      textForm = new TextDecoder("utf-8").decode(bodyText);
    } else {
      textForm = String(bodyText);
    }
    return {
      _body: textForm,
      _bodyBytes: bodyBytes || null,
      _bodyUsed: false,
      get bodyUsed() { return this._bodyUsed; },
      text() {
        if (this._bodyUsed) return Promise.reject(new TypeError("Body already consumed"));
        this._bodyUsed = true;
        return Promise.resolve(this._body);
      },
      json() {
        if (this._bodyUsed) return Promise.reject(new TypeError("Body already consumed"));
        this._bodyUsed = true;
        try { return Promise.resolve(JSON.parse(this._body)); }
        catch (e) { return Promise.reject(e); }
      },
      bytes() {
        if (this._bodyUsed) return Promise.reject(new TypeError("Body already consumed"));
        this._bodyUsed = true;
        if (this._bodyBytes) return Promise.resolve(this._bodyBytes);
        return Promise.resolve(new __bun_te().encode(this._body));
      },
      arrayBuffer() {
        if (this._bodyUsed) return Promise.reject(new TypeError("Body already consumed"));
        this._bodyUsed = true;
        const u8 = this._bodyBytes || new __bun_te().encode(this._body);
        // Slice into a fresh ArrayBuffer so callers can mutate freely.
        return Promise.resolve(u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength));
      },
    };
  }

  // Minimal TextEncoder/TextDecoder shim. JSC has neither.
  class __bun_te {
    encode(s) {
      s = String(s);
      const out = [];
      for (let i = 0; i < s.length; i++) {
        const c = s.charCodeAt(i);
        if (c < 0x80) out.push(c);
        else if (c < 0x800) { out.push(0xc0 | (c >> 6)); out.push(0x80 | (c & 0x3f)); }
        else if (c < 0xd800 || c >= 0xe000) {
          out.push(0xe0 | (c >> 12));
          out.push(0x80 | ((c >> 6) & 0x3f));
          out.push(0x80 | (c & 0x3f));
        } else {
          // surrogate pair → codepoint
          const c2 = s.charCodeAt(++i);
          const cp = 0x10000 + (((c & 0x3ff) << 10) | (c2 & 0x3ff));
          out.push(0xf0 | (cp >> 18));
          out.push(0x80 | ((cp >> 12) & 0x3f));
          out.push(0x80 | ((cp >> 6) & 0x3f));
          out.push(0x80 | (cp & 0x3f));
        }
      }
      return new Uint8Array(out);
    }
  }
  g.TextEncoder = __bun_te;
  g.TextDecoder = class {
    constructor(label) { this._label = label || "utf-8"; }
    decode(buf) {
      const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
      let out = "";
      let i = 0;
      while (i < u8.length) {
        const b = u8[i++];
        if (b < 0x80) out += String.fromCharCode(b);
        else if (b < 0xc0) continue;
        else if (b < 0xe0) {
          const b2 = u8[i++] & 0x3f;
          out += String.fromCharCode(((b & 0x1f) << 6) | b2);
        } else if (b < 0xf0) {
          const b2 = u8[i++] & 0x3f;
          const b3 = u8[i++] & 0x3f;
          out += String.fromCharCode(((b & 0x0f) << 12) | (b2 << 6) | b3);
        } else {
          const b2 = u8[i++] & 0x3f, b3 = u8[i++] & 0x3f, b4 = u8[i++] & 0x3f;
          const cp = ((b & 0x07) << 18) | (b2 << 12) | (b3 << 6) | b4;
          const adj = cp - 0x10000;
          out += String.fromCharCode(0xd800 + (adj >> 10), 0xdc00 + (adj & 0x3ff));
        }
      }
      return out;
    }
  };

  // ─────────────────────────── Request ───────────────────────────
  class Request {
    constructor(input, init) {
      init = init || {};
      this.url = typeof input === "string" ? input : input.url;
      this.method = (init.method || "GET").toUpperCase();
      this.headers = new Headers(init.headers || {});
      const bytes = init.body instanceof Uint8Array ? init.body : null;
      Object.assign(this, makeBody(init.body, bytes));
    }
  }
  g.Request = Request;

  // ─────────────────────────── Response ───────────────────────────
  class Response {
    constructor(body, init) {
      init = init || {};
      // If body is itself a ReadableStream, consume it lazily on demand.
      const isStream = body && typeof body === "object" && typeof body.getReader === "function";
      const bytes = body instanceof Uint8Array ? body : init._bytes || null;
      if (isStream) {
        // Drain the stream lazily to build text/bytes on demand.
        Object.assign(this, makeBody("", null));
        this._sourceStream = body;
      } else {
        Object.assign(this, makeBody(body, bytes));
      }
      this.status = init.status != null ? init.status : 200;
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers || {});
      this.url = init.url || "";
      this.ok = this.status >= 200 && this.status < 300;
      this.redirected = false;
      this.type = "default";
    }
    get body() {
      if (this._sourceStream) return this._sourceStream;
      if (this._streamBody) return this._streamBody;
      // Wrap the existing string/bytes body in a one-shot ReadableStream.
      const bytes = this._bodyBytes || (this._body ? new TextEncoder().encode(this._body) : new Uint8Array(0));
      let yielded = false;
      this._streamBody = new ReadableStream({
        pull(controller) {
          if (!yielded) {
            yielded = true;
            if (bytes.byteLength > 0) controller.enqueue(bytes);
          }
          controller.close();
        },
      });
      return this._streamBody;
    }
    static json(data, init) {
      const body = JSON.stringify(data);
      const r = new Response(body, init);
      if (!r.headers.has("content-type")) r.headers.set("content-type", "application/json;charset=UTF-8");
      return r;
    }
    static error() {
      return new Response("", { status: 0 });
    }
  }
  g.Response = Response;

  // ─────────────────────────── fetch ───────────────────────────
  // __bun_fetch is async (returns a Promise) on the Rust side, so wrap.
  g.fetch = async function (url, init) {
    // Accept (string | URL | Request). Request carries its own headers /
    // method / body so we merge those into init.
    let u;
    if (typeof url === "string") {
      u = url;
    } else if (url && typeof url.href === "string") {
      u = url.href;             // URL object
    } else if (url && typeof url.url === "string") {
      u = url.url;              // Request
      if (!init) init = {
        method: url.method,
        headers: url.headers,
        body: url._bodyText !== undefined ? url._bodyText : url._bodyBytes,
        signal: url.signal,
      };
    } else {
      u = String(url);
    }
    const i = init || {};
    const raw = await __bun_fetch(u, {
      method: (i.method || "GET").toUpperCase(),
      headers: (function () {
        if (!i.headers) return {};
        if (i.headers instanceof Headers) {
          const o = {};
          for (const [k, v] of i.headers) o[k] = v;
          return o;
        }
        return i.headers;
      })(),
      body: i.body,
      signal: i.signal,
    });
    return new Response(raw.body, {
      status: raw.status,
      headers: raw.headers,
      url: raw.url,
      _bytes: raw.bytes,
    });
  };
  // ─────────────────────── AbortController ──────────────────────
  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this._listeners = { abort: [] };
    }
    addEventListener(type, fn) {
      if (type !== "abort") return;
      if (this.aborted) {
        try { fn({ type: "abort", target: this }); } catch {}
        return;
      }
      this._listeners.abort.push(fn);
    }
    removeEventListener(type, fn) {
      if (type !== "abort") return;
      const i = this._listeners.abort.indexOf(fn);
      if (i !== -1) this._listeners.abort.splice(i, 1);
    }
    dispatchEvent(event) {
      if (event.type !== "abort") return false;
      for (const fn of this._listeners.abort.slice()) {
        try { fn(event); } catch (e) { console.error(e); }
      }
      if (typeof this.onabort === "function") {
        try { this.onabort(event); } catch (e) { console.error(e); }
      }
      return true;
    }
    throwIfAborted() {
      if (this.aborted) throw this.reason || new Error("Aborted");
    }
    static abort(reason) {
      const s = new AbortSignal();
      s.aborted = true;
      s.reason = reason !== undefined ? reason : new DOMException_("AbortError", "AbortError");
      return s;
    }
    static timeout(ms) {
      const s = new AbortSignal();
      setTimeout(() => {
        if (!s.aborted) {
          s.aborted = true;
          s.reason = new DOMException_("TimeoutError", "TimeoutError");
          s.dispatchEvent({ type: "abort", target: s });
        }
      }, ms);
      return s;
    }
    static any(signals) {
      const s = new AbortSignal();
      for (const sig of signals) {
        if (sig.aborted) {
          s.aborted = true;
          s.reason = sig.reason;
          return s;
        }
      }
      for (const sig of signals) {
        sig.addEventListener("abort", () => {
          if (!s.aborted) {
            s.aborted = true;
            s.reason = sig.reason;
            s.dispatchEvent({ type: "abort", target: s });
          }
        });
      }
      return s;
    }
  }
  // Lightweight DOMException stub.
  class DOMException_ extends Error {
    constructor(message, name) {
      super(message);
      this.name = name || "Error";
    }
  }
  if (typeof g.DOMException === "undefined") g.DOMException = DOMException_;

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }
    abort(reason) {
      if (this.signal.aborted) return;
      this.signal.aborted = true;
      this.signal.reason = reason !== undefined ? reason : new g.DOMException("AbortError", "AbortError");
      this.signal.dispatchEvent({ type: "abort", target: this.signal });
    }
  }
  g.AbortController = AbortController;
  g.AbortSignal = AbortSignal;

  // ── Blob ───────────────────────────────────────────────────────────
  if (typeof g.Blob === "undefined") {
    class Blob {
      constructor(parts, opts) {
        const chunks = [];
        if (parts && parts[Symbol.iterator]) {
          for (const p of parts) {
            if (p instanceof Blob) chunks.push(p._bytes);
            else if (p instanceof ArrayBuffer) chunks.push(new Uint8Array(p));
            else if (ArrayBuffer.isView(p)) chunks.push(new Uint8Array(p.buffer, p.byteOffset, p.byteLength));
            else if (typeof p === "string") chunks.push(new TextEncoder().encode(p));
          }
        }
        let total = 0;
        for (const c of chunks) total += c.byteLength;
        const out = new Uint8Array(total);
        let off = 0;
        for (const c of chunks) { out.set(c, off); off += c.byteLength; }
        this._bytes = out;
        this.size = total;
        this.type = (opts && opts.type) ? String(opts.type).toLowerCase() : "";
      }
      slice(start, end, type) {
        const b = new Blob([]);
        b._bytes = this._bytes.slice(start, end);
        b.size = b._bytes.byteLength;
        b.type = type || "";
        return b;
      }
      async text() { return new TextDecoder("utf-8").decode(this._bytes); }
      async arrayBuffer() { return this._bytes.buffer.slice(this._bytes.byteOffset, this._bytes.byteOffset + this._bytes.byteLength); }
      async bytes() { return new Uint8Array(this._bytes); }
      stream() {
        const data = this._bytes;
        return new ReadableStream({ start(c) { c.enqueue(data); c.close(); } });
      }
    }
    g.Blob = Blob;
  }
  if (typeof g.File === "undefined") {
    class File extends g.Blob {
      constructor(parts, name, opts) {
        super(parts, opts);
        this.name = String(name || "");
        this.lastModified = (opts && opts.lastModified) || Date.now();
      }
    }
    g.File = File;
  }

  // ── FormData ───────────────────────────────────────────────────────
  if (typeof g.FormData === "undefined") {
    class FormData {
      constructor() { this._entries = []; }
      append(name, value, _filename) {
        this._entries.push([String(name), value]);
      }
      set(name, value, _filename) { this.delete(name); this.append(name, value); }
      get(name) { const e = this._entries.find(e => e[0] === name); return e ? e[1] : null; }
      getAll(name) { return this._entries.filter(e => e[0] === name).map(e => e[1]); }
      has(name) { return this._entries.some(e => e[0] === name); }
      delete(name) { this._entries = this._entries.filter(e => e[0] !== name); }
      *entries() { for (const e of this._entries) yield [e[0], e[1]]; }
      *keys() { for (const e of this._entries) yield e[0]; }
      *values() { for (const e of this._entries) yield e[1]; }
      forEach(cb, thisArg) { for (const e of this._entries) cb.call(thisArg, e[1], e[0], this); }
      [Symbol.iterator]() { return this.entries(); }
    }
    g.FormData = FormData;
  }

  // ── performance ────────────────────────────────────────────────────
  if (typeof g.performance === "undefined") {
    const startMs = Date.now();
    const startHr = (typeof process !== "undefined" && process && process.hrtime) ? process.hrtime() : null;
    g.performance = {
      timeOrigin: startMs,
      now() {
        if (startHr && typeof process !== "undefined" && process.hrtime) {
          const d = process.hrtime(startHr);
          return d[0] * 1000 + d[1] / 1e6;
        }
        return Date.now() - startMs;
      },
      mark() {}, measure() {}, clearMarks() {}, clearMeasures() {},
      getEntries() { return []; }, getEntriesByName() { return []; }, getEntriesByType() { return []; },
      toJSON() { return { timeOrigin: this.timeOrigin }; },
    };
  }
  if (typeof g.PerformanceObserver === "undefined") {
    g.PerformanceObserver = class { constructor(){}; observe(){}; disconnect(){}; takeRecords(){return [];} };
  }

  if (typeof g.reportError === "undefined") {
    g.reportError = (err) => { console.error(err); };
  }

  // ── Event / CustomEvent / EventTarget — minimal DOM-shape stubs ────
  if (typeof g.Event === "undefined") {
    class Event {
      constructor(type, init) {
        init = init || {};
        this.type = String(type);
        this.bubbles = !!init.bubbles;
        this.cancelable = !!init.cancelable;
        this.composed = !!init.composed;
        this.defaultPrevented = false;
        this.timeStamp = Date.now();
        this.target = null;
        this.currentTarget = null;
        this.isTrusted = false;
        this.eventPhase = 0;
      }
      preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
      stopPropagation() {}
      stopImmediatePropagation() {}
    }
    g.Event = Event;
  }
  if (typeof g.CustomEvent === "undefined") {
    class CustomEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        this.detail = (init && init.detail) ?? null;
      }
    }
    g.CustomEvent = CustomEvent;
  }
  if (typeof g.ErrorEvent === "undefined") {
    class ErrorEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        init = init || {};
        this.error = init.error || null;
        this.message = init.message || "";
        this.filename = init.filename || "";
        this.lineno = init.lineno || 0;
        this.colno = init.colno || 0;
      }
    }
    g.ErrorEvent = ErrorEvent;
  }
  if (typeof g.MessageEvent === "undefined") {
    class MessageEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        init = init || {};
        this.data = init.data;
        this.origin = init.origin || "";
        this.lastEventId = init.lastEventId || "";
        this.source = init.source || null;
        this.ports = init.ports || [];
      }
    }
    g.MessageEvent = MessageEvent;
  }
  if (typeof g.CloseEvent === "undefined") {
    class CloseEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        init = init || {};
        this.code = init.code || 0;
        this.reason = init.reason || "";
        this.wasClean = !!init.wasClean;
      }
    }
    g.CloseEvent = CloseEvent;
  }
  if (typeof g.PromiseRejectionEvent === "undefined") {
    class PromiseRejectionEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        init = init || {};
        this.promise = init.promise;
        this.reason = init.reason;
      }
    }
    g.PromiseRejectionEvent = PromiseRejectionEvent;
  }
  if (typeof g.EventTarget === "undefined") {
    class EventTarget {
      constructor() { this._listeners = {}; }
      addEventListener(type, fn, _opts) {
        (this._listeners[type] = this._listeners[type] || []).push(fn);
      }
      removeEventListener(type, fn) {
        const a = this._listeners[type];
        if (!a) return;
        const i = a.indexOf(fn);
        if (i >= 0) a.splice(i, 1);
      }
      dispatchEvent(ev) {
        const a = this._listeners[ev.type] || [];
        ev.target = this;
        ev.currentTarget = this;
        for (const fn of a.slice()) {
          try {
            if (typeof fn === "function") fn.call(this, ev);
            else if (fn && typeof fn.handleEvent === "function") fn.handleEvent(ev);
          } catch (e) { console.error(e); }
        }
        return !ev.defaultPrevented;
      }
    }
    g.EventTarget = EventTarget;
  }

  // self getter — alias for globalThis. Bun's tests check this is a getter
  // on the global, so use defineProperty.
  if (!Object.getOwnPropertyDescriptor(g, "self") || !Object.getOwnPropertyDescriptor(g, "self").get) {
    Object.defineProperty(g, "self", { get() { return g; }, configurable: true });
  }
  if (!Object.getOwnPropertyDescriptor(g, "window") || !Object.getOwnPropertyDescriptor(g, "window").get) {
    Object.defineProperty(g, "window", { get() { return g; }, configurable: true });
  }
  if (typeof g.frames === "undefined") g.frames = g;

  // ── crypto (Web Crypto API global) ─────────────────────────────────
  // Bun's tests expect a `crypto` global with .randomUUID(),
  // .getRandomValues(), and .subtle. Node has it; tests do `crypto.xxx`.
  if (typeof g.crypto === "undefined") {
    g.crypto = {
      randomUUID() {
        const b = new Uint8Array(16);
        for (let i = 0; i < 16; i++) b[i] = (Math.random() * 256) & 0xff;
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        const h = Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
        return h.slice(0, 8) + "-" + h.slice(8, 12) + "-" + h.slice(12, 16) + "-" + h.slice(16, 20) + "-" + h.slice(20, 32);
      },
      getRandomValues(arr) {
        if (!ArrayBuffer.isView(arr)) throw new TypeError("getRandomValues requires a TypedArray");
        const v = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
        for (let i = 0; i < v.length; i++) v[i] = (Math.random() * 256) & 0xff;
        return arr;
      },
      subtle: {
        // Real WebCrypto algorithms need real crypto; pass through to
        // node:crypto when possible.
        async digest(algo, data) {
          const c = require("node:crypto");
          const name = (typeof algo === "string" ? algo : algo.name).toLowerCase().replace("sha-", "sha");
          const buf = ArrayBuffer.isView(data)
            ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
            : new Uint8Array(data);
          const h = c.createHash(name).update(buf).digest();
          return new Uint8Array(h).buffer;
        },
        async importKey() { throw new Error("subtle.importKey not implemented"); },
        async exportKey() { throw new Error("subtle.exportKey not implemented"); },
        async encrypt() { throw new Error("subtle.encrypt not implemented"); },
        async decrypt() { throw new Error("subtle.decrypt not implemented"); },
        async sign() { throw new Error("subtle.sign not implemented"); },
        async verify() { throw new Error("subtle.verify not implemented"); },
        async generateKey() { throw new Error("subtle.generateKey not implemented"); },
        async deriveBits() { throw new Error("subtle.deriveBits not implemented"); },
        async deriveKey() { throw new Error("subtle.deriveKey not implemented"); },
        async wrapKey() { throw new Error("subtle.wrapKey not implemented"); },
        async unwrapKey() { throw new Error("subtle.unwrapKey not implemented"); },
      },
    };
  }

  // ── HTMLRewriter (stub) — Bun-specific streaming HTML transformer ──
  if (typeof g.HTMLRewriter === "undefined") {
    class HTMLRewriter {
      constructor() { this._handlers = []; }
      on(_sel, _h) { return this; }
      onDocument(_h) { return this; }
      transform(input) { return input; }
    }
    g.HTMLRewriter = HTMLRewriter;
  }
  if (typeof g.structuredClone === "undefined") {
    g.structuredClone = function (v) {
      try { return JSON.parse(JSON.stringify(v)); } catch { return v; }
    };
  }

})(globalThis);
