# Changelog

All notable changes to bun-rs are documented here. Versioning follows
[SemVer](https://semver.org/) (with the customary "anything goes pre-1.0" caveat).

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
