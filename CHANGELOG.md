# Changelog

All notable changes to bun-rs are documented here. Versioning follows
[SemVer](https://semver.org/) (with the customary "anything goes pre-1.0" caveat).

## [1.0.1] – 2026-05-13

### Fixed
- `req.text()` and `Response.text()` returned the comma-separated
  `String(uint8array)` form when the body was constructed from a
  Uint8Array. Now decoded as UTF-8. Manifested in `Bun.serve`
  handlers reading POST bodies. Surfaced building the
  `examples/shortener` demo.

### Added
- `examples/shortener/` — a working URL shortener (Bun.serve +
  bun:sqlite + JSON + 302 redirect, ~80 lines).

## [1.0.0] – 2026-05-12

The 1.0 release. The public surface documented in `docs/guide.md` is
considered stable; anything we'd break would be 2.0.

This release consolidates everything done since 0.3.0 plus the
foundations for production use.

### Added since 0.3.0

**Native databases / FFI**
- `bun:sqlite` — full Database / query / prepare API (positional + named
  params, blob round-trip)
- `bun:ffi` — `dlopen(path, { sym: { args, returns } })` with i8…u64 /
  f32 / f64 / pointer / cstring types

**HTTP stack**
- `Bun.serve` over HTTPS — `tls: { key, cert }` option, PEM strings
  or paths; rustls + tokio-rustls
- HTTP/2 via ALPN negotiation (`h2` preferred, falls back to http/1.1)
- `node:http` — `createServer(handler)` + `http.get/request(cb)`,
  wrapping `Bun.serve` and `fetch` under the hood

**Concurrency / parallelism**
- `Worker` (WHATWG) — std::thread + per-worker JSC Context + JSON
  message passing, `postMessage` / `onmessage` / `terminate`

**npm / packaging**
- `bun-rs install` — fetches from registry.npmjs.org, extracts
  tarballs to `node_modules/`, writes `bun-rs.lock.json`. Supports
  scoped packages, `--production`. Env override: `BUN_REGISTRY`
- **CJS interop** — `require("./y")` style modules (`module.exports = …`)
  load correctly, and `import x from "cjs-pkg"` follows the
  esModuleInterop convention so common npm packages just work

**Compression**
- `node:zlib` — deflate / inflate / gzip / gunzip / deflateRaw /
  inflateRaw, sync + callback-async forms

### Changed
- Module wrapper now passes a `__module` indirection so CJS and ESM
  bodies share the same `exports` storage. ESM modules are stamped
  `__esModule = true` so the default-import shim distinguishes them
- Sourcemap remapper updated for the new 4-line wrapper prefix
- Cleaned up dead `#[warn]` paths

### Known limitations preserved
- No live ESM bindings (default-imports are still value snapshots)
- Worker doesn't support SharedArrayBuffer / transferables / nested workers
- `fetch` doesn't honor `AbortSignal` mid-stream — only at request setup
- `bun-rs install` resolves loose semver (^/~ stripped to exact) —
  pinned versions and `latest` are reliable
- macOS + Linux only (no Windows JSC build)
- Sourcemap remap is line-only (no column), and may drift for
  JSX-heavy files where oxc's transpile shifts lines

## [0.3.0] – 2026-05-12

The theme: workflow — test runner, bundler, REPL polish.

### Added
- **`bun-rs test`** — Jest-compatible runner with `describe` / `test` /
  `it` / `expect` (16+ matchers, `.not`, async `.resolves` / `.rejects`),
  `beforeAll` / `afterAll` / `beforeEach` / `afterEach`. Auto-discovers
  `*.test.{ts,tsx,js,jsx}` / `*.spec.{ts,tsx,js}` files.
- **`bun-rs build <entry> [--outfile path]`** — single-file bundler.
  Walks the import graph via the existing loader, emits all reachable
  modules as numbered factories in one self-contained JS file.
  `node:*` imports stay as host-resolved externals. ~1.7KB for a
  3-module hello-world.
- **`WebSocket`** — client, text + binary frames, `addEventListener` +
  `on*`, custom close codes / reasons. tokio_tungstenite + rustls.
- **`fetch` honors `AbortSignal`** — `init.signal` wires into the
  tokio task via a cancel channel; aborting collapses the request
  immediately instead of waiting for the full response.
- **`node:readline`** — `createInterface`, `question(query, cb)`,
  `on('line')` / `on('close')`, `readline.promises.createInterface`
  with `await iface.question(query)`.

### Fixed
- **Event-loop bug**: `run_one_tick` used to `std::thread::sleep` until
  the next timer's deadline, which starved async-runtime task delivery
  (WS messages, fetch completions, fs.promises) by up to seconds.
  Replaced with a "fire only due timers; caller naps in short
  chunks" design. All existing tests stayed green; readline /
  WebSocket tests would have surfaced this on every run.

### Known still-missing
- Live ESM bindings (deferred to 0.4 — needs symbol-level rewriter).
- HTTPS server, HTTP/2 (deferred to 0.5).
- `bun install` (deferred to 0.4).

## [0.2.0] – 2026-05-12

The theme: streams, concurrency, better error reporting.

### Added

- **Web Streams**: `ReadableStream`, `WritableStream`, `TransformStream`,
  default readers/writers, `pipeTo` / `pipeThrough` / `tee`,
  `ReadableStream.from(iter)` (sync + async), `Symbol.asyncIterator` for
  `for await (const chunk of stream)`. `Response.body` is now a real
  ReadableStream.
- **`node:stream`** — `Readable` / `Writable` / `Duplex` / `PassThrough`
  / `Transform` (alias) as EventEmitter subclasses; auto-flow on
  `'data'` listener; `pipeline()` + `finished()`; Web Streams interop
  via `Readable.toWeb` / `Readable.fromWeb`.
- **`fs.createReadStream(path)`** — streams a file in `highWaterMark`-
  sized chunks (default 64KB) via tokio `spawn_blocking`.
- **`fs.createWriteStream(path)`** — Writable backed by `std::fs::File`.
- **`AbortController` / `AbortSignal`** — full set including
  `AbortSignal.abort` / `.timeout(ms)` / `.any([…])` / `throwIfAborted`.
  Note: `fetch` does not yet observe the `signal`.
- **Concurrent `Bun.serve`** — backed by hyper instead of tiny_http. Each
  request runs in its own tokio task; a slow handler no longer blocks
  acceptance of new connections. Verified: 5 × 100ms requests now
  finish in ~250ms (was ~500ms+ on 0.1).
- **Sourcemap-aware error stacks** — throw at `bad.ts:4` now reports
  `bad.ts:4` rather than the rewriter line. Synthetic frames
  (generated import shims) are tagged `<bunrs-internal>`. Column info
  dropped; JSX-heavy files may drift slightly.

### Changed

- The event loop now drains async-runtime tasks (`fetch`, `fs.promises`,
  pending Bun.serve responses) alongside timers — the runtime no longer
  deadlocks awaiting a pending Promise while concurrent I/O is in flight.

### Known still-missing
- `fetch` doesn't observe `AbortSignal` yet.
- HTTPS, HTTP/2, WebSocket.
- `bun install` / `bun build` / `bun test`.
- Live ESM bindings.

## [0.1.0] – 2026-05-12

The first public release. A small but credibly working JavaScript / TypeScript
runtime built on JavaScriptCore (via FFI) and oxc (for TS/JSX).

### What works

**Language**
- Run `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` files
- Full ES Modules: static and **dynamic `import()`**, **top-level `await`**,
  named/default/namespace imports, `export * from`, `export { x } from`,
  circular dependencies
- `import.meta.url` / `filename` / `dirname` / `main`
- node_modules resolution (`oxc_resolver`, supports `exports`/`main`/`module`)
- Async / await + native Promise
- REPL (`bun-rs` with no args, multi-line continuation)

**Built-in globals**
- `console.*` (log/info/warn/error/debug/trace/dir)
- `process.*` (argv, env, cwd, exit, platform, arch, pid, versions,
  stdout/stderr/stdin, hrtime, nextTick)
- `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` / `queueMicrotask`
- **`fetch`** — async via tokio + reqwest (rustls), does not block the JS thread
- `Buffer` — full class extending Uint8Array, zero-copy from Rust
- `URL` / `URLSearchParams`, `Headers` / `Request` / `Response`
- `TextEncoder` / `TextDecoder` (UTF-8), `atob` / `btoa`

**`Bun.*` namespace**
- `Bun.serve({ port, fetch })` — minimal HTTP server, sync handler
- `Bun.file(path)` with `.text()` / `.json()` / `.bytes()` / `.arrayBuffer()` / `.exists()`
- `Bun.write(path, data)`, `Bun.sleep(ms)`, `Bun.env`
- `Bun.version`, `Bun.revision`

**`node:` modules**
- `node:path`, `node:os`, `node:fs` (sync + **true async `fs.promises`**),
  `node:buffer`, `node:events`, `node:util`, `node:crypto`, `node:child_process`,
  `node:assert`, `node:querystring`, `node:url`

**Platforms**
- macOS (system `JavaScriptCore.framework`)
- Linux (`libjavascriptcoregtk-4.1`) — build path in place, smoke-tested via CI

### What does NOT work in 0.1
- Streams (`ReadableStream`, `node:stream`)
- HTTPS server, HTTP/2, WebSocket
- `Bun.serve` concurrency (one request at a time on the JS thread)
- Live ESM bindings (`import` captures values at load time, not bindings)
- Sourcemap-aware error stacks (errors point to rewritten lines)
- Worker / Cluster
- `bun install` / `bun build` (no package manager / bundler)
- shell / SQL / bake / FFI
- Windows

### Known issues
- Reading a binary file via `fs.readFileSync(path, "utf8")` does `from_utf8_lossy`;
  use `fs.readFileSync(path)` (no encoding) to get a Buffer
- `Bun.serve` blocks the JS thread while a handler is running (handlers run
  sequentially); fetch inside a handler is still async
- Many `node:*` APIs implement only the most common subset; please report
  specific missing functions

See `docs/guide.md` for a detailed per-API breakdown and `docs/roadmap.md`
for what's coming next.
