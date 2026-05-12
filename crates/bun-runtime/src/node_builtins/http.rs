//! `node:http` — minimal compatibility shim.
//!
//! Client (http.get / http.request) wraps `fetch`. Server
//! (http.createServer) wraps `Bun.serve`. The data model approximates
//! Node's IncomingMessage / ServerResponse just enough for common use:
//!
//!   - server.on("request", (req, res) => …) where:
//!       req.method / req.url / req.headers
//!       res.writeHead(status, headers); res.write(chunk); res.end([chunk]);
//!       res.setHeader(name, value)
//!   - http.request(opts | url, cb) where opts may be a URL string or
//!     { hostname, port, path, method, headers }
//!
//! Not implemented:
//!   - Keep-alive / Agent
//!   - Chunked-encoding streaming bodies on the *response* (we buffer)
//!   - http.Server events beyond "request" / "listening" / "close"

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let v = ctx.eval(POLYFILL, Some("[node:http]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("default", &v).unwrap();
    v
}

const POLYFILL: &str = r#"
(() => {
  // ─────────────────────── client ───────────────────────
  function buildRequest(arg, cb) {
    let url, method = "GET", headers = {};
    if (typeof arg === "string") {
      url = arg;
    } else {
      // { protocol, hostname, port, path, method, headers }
      const proto = arg.protocol || "http:";
      const host = arg.hostname || arg.host || "localhost";
      const port = arg.port ? ":" + arg.port : "";
      const path = arg.path || "/";
      url = `${proto}//${host}${port}${path}`;
      if (arg.method) method = arg.method;
      if (arg.headers) headers = arg.headers;
    }
    // Build a thin "ClientRequest" object users can .write to / .end.
    let bodyParts = [];
    const req = {
      _resolved: false,
      _url: url,
      _method: method,
      _headers: headers,
      write(chunk) { bodyParts.push(chunk); return true; },
      end(chunk) {
        if (chunk) bodyParts.push(chunk);
        // Fire fetch. Build body if there are parts.
        let body;
        if (bodyParts.length > 0) {
          if (bodyParts.every(p => p instanceof Uint8Array)) {
            const total = bodyParts.reduce((s, p) => s + p.byteLength, 0);
            body = new Uint8Array(total);
            let off = 0;
            for (const p of bodyParts) { body.set(p, off); off += p.byteLength; }
          } else {
            body = bodyParts.map(p => typeof p === "string" ? p : new TextDecoder().decode(p)).join("");
          }
        }
        fetch(req._url, { method: req._method, headers: req._headers, body }).then(async (r) => {
          // Build IncomingMessage-like response.
          const buf = await r.bytes();
          const msg = {
            statusCode: r.status,
            statusMessage: "",
            headers: (function () {
              const out = {};
              for (const [k, v] of r.headers) out[k.toLowerCase()] = v;
              return out;
            })(),
            _listeners: { data: [], end: [], error: [] },
            _consumed: false,
            on(event, fn) {
              this._listeners[event] && this._listeners[event].push(fn);
              if (!this._consumed) this._consume();
              return this;
            },
            _consume() {
              this._consumed = true;
              queueMicrotask(() => {
                const chunk = Buffer.from(buf);
                for (const fn of this._listeners.data.slice()) fn(chunk);
                for (const fn of this._listeners.end.slice()) fn();
              });
            },
            text() { return new TextDecoder().decode(buf); },
            json() { return JSON.parse(new TextDecoder().decode(buf)); },
          };
          if (cb) cb(msg);
        }, (e) => {
          if (req._errorFn) req._errorFn(e);
        });
      },
      on(event, fn) {
        if (event === "error") req._errorFn = fn;
        return req;
      },
    };
    return req;
  }
  function get(arg, cb) {
    const req = buildRequest(arg, cb);
    req.end();
    return req;
  }

  // ─────────────────────── server ───────────────────────
  function createServer(handler) {
    const server = {
      _handler: handler,
      _bunServer: null,
      _listeners: { request: [], listening: [], close: [], error: [] },
      on(event, fn) {
        if (this._listeners[event]) this._listeners[event].push(fn);
        return this;
      },
      listen(port, hostOrCb, maybeCb) {
        const cb = typeof hostOrCb === "function" ? hostOrCb : maybeCb;
        const self = this;
        this._bunServer = Bun.serve({
          port: port || 0,
          async fetch(req) {
            // Build IncomingMessage + ServerResponse shapes.
            const u = new URL(req.url);
            const incoming = {
              method: req.method,
              url: u.pathname + u.search,
              headers: (function () {
                const out = {};
                for (const [k, v] of req.headers) out[k.toLowerCase()] = v;
                return out;
              })(),
              httpVersion: "1.1",
              on(event, fn) {
                if (event === "data") {
                  req.bytes().then(b => { if (b.length > 0) fn(b); });
                } else if (event === "end") {
                  req.bytes().then(() => fn());
                }
                return this;
              },
              text: () => req.text(),
              json: () => req.json(),
            };
            return new Promise((resolve) => {
              let status = 200;
              let headers = {};
              let parts = [];
              const res = {
                statusCode: 200,
                writeHead(s, hdrs) {
                  status = s;
                  if (hdrs) Object.assign(headers, hdrs);
                  return this;
                },
                setHeader(name, value) {
                  headers[name] = value;
                  return this;
                },
                getHeader(name) { return headers[name]; },
                write(chunk) { parts.push(chunk); return true; },
                end(chunk) {
                  if (chunk) parts.push(chunk);
                  let body;
                  if (parts.length === 0) {
                    body = "";
                  } else if (parts.every(p => p instanceof Uint8Array)) {
                    const total = parts.reduce((s, p) => s + p.byteLength, 0);
                    body = new Uint8Array(total);
                    let off = 0;
                    for (const p of parts) { body.set(p, off); off += p.byteLength; }
                  } else {
                    body = parts.map(p => typeof p === "string" ? p : new TextDecoder().decode(p)).join("");
                  }
                  resolve(new Response(body, { status: status || this.statusCode || 200, headers }));
                },
              };
              // Call request listeners first; if none and there's a constructor handler, use it.
              const listeners = self._listeners.request;
              if (listeners.length > 0) {
                for (const l of listeners) l(incoming, res);
              } else if (typeof self._handler === "function") {
                self._handler(incoming, res);
              } else {
                res.end();
              }
            });
          },
        });
        if (cb) queueMicrotask(cb);
        for (const fn of this._listeners.listening.slice()) {
          queueMicrotask(fn);
        }
        return this;
      },
      address() {
        if (!this._bunServer) return null;
        return { port: this._bunServer.port, family: "IPv4", address: "127.0.0.1" };
      },
      close(cb) {
        if (this._bunServer) this._bunServer.stop();
        for (const fn of this._listeners.close.slice()) fn();
        if (cb) queueMicrotask(cb);
        return this;
      },
    };
    if (typeof handler === "function") server.on("request", handler);
    return server;
  }

  return { get, request: buildRequest, createServer, METHODS: ["GET","POST","PUT","DELETE","HEAD","OPTIONS","PATCH"] };
})()
"#;
