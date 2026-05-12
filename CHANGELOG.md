# Changelog

All notable changes to bun-rs are documented here. Versioning follows
[SemVer](https://semver.org/) (with the customary "anything goes pre-1.0" caveat).

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
