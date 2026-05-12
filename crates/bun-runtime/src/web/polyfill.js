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
    return {
      _body: bodyText == null ? "" : (typeof bodyText === "string" ? bodyText : String(bodyText)),
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
      const bytes = body instanceof Uint8Array ? body : init._bytes || null;
      Object.assign(this, makeBody(body, bytes));
      this.status = init.status != null ? init.status : 200;
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers || {});
      this.url = init.url || "";
      this.ok = this.status >= 200 && this.status < 300;
      this.redirected = false;
      this.type = "default";
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
    const u = typeof url === "string" ? url : url.url;
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
    });
    return new Response(raw.body, {
      status: raw.status,
      headers: raw.headers,
      url: raw.url,
      _bytes: raw.bytes,
    });
  };
})(globalThis);
