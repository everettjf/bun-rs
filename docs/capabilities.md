# What bun-rs can and cannot do

> bun-rs is a personal toy project that aims to rewrite [Bun](https://github.com/oven-sh/bun)
> in Rust. **It is very rough and very incomplete.** Treat everything below as
> best-effort. If real software depends on it, use real Bun.

The headline number: **159 / 409 file-level tests in Bun's own `test/js/bun`
suite pass — about 38.9 %.** Roughly 6.3 K individual test cases green.

Three reasons that number isn't higher:

1. Whole API surfaces are deliberately stubbed (raw TCP/UDP/TLS, real DNS,
   workers, true argon2, WebSocket server upgrade, Bun's own shell parser).
2. Several "works" surfaces still trip on edge cases (sourcemap columns,
   import attributes other than JSON, snapshot diffing).
3. Bun's test suite assumes V8-style stack trace format; bun-rs emits JSC
   format. A bunch of file failures are pure string-shape mismatches.

## Legend

| | Meaning |
|---|---|
| ✅ | implemented in Rust + JS; behaves correctly on the cases we tested |
| 🟡 | callable; partial semantics or known edge-case gaps |
| ❌ | calling it throws a JS `Error` or returns a placeholder |

The runtime **never panics** on missing APIs — stubs throw JS errors so
your code can `try / catch`.

---

## 1. What you can build today

Concrete things people have actually run on bun-rs:

- A multi-file TypeScript program with `node_modules`.
- An HTTP / HTTPS service with `Bun.serve` + `fetch` + JSON request bodies.
- A CLI that reads files, hashes them, calls subprocesses.
- A Jest-style test suite (`describe` / `test` / `expect` / `jest.fn` /
  fake timers) running on `bun-rs test`.
- A single-file bundle of a small ESM project via `bun-rs build`.
- A SQLite-backed script via `bun:sqlite`.
- A small WebSocket **client** consuming a real WS server.

## 2. What you should *not* reach for bun-rs to do

- Anything in production.
- Raw TCP / UDP / TLS — `node:net`, `node:tls`, `node:dgram`, `Bun.listen`,
  `Bun.connect` are shape stubs only.
- Real password hashing — `Bun.password.hash` / `.verify` use HMAC-SHA256
  under the hood, **not argon2 or bcrypt**. Compatible API, incompatible bytes.
- Workers / SharedArrayBuffer / transferables — `Worker` throws.
- WebSocket *server* (handshake upgrade in `Bun.serve`) — only the client
  is real.
- Live DNS — `node:dns.lookup` always returns 127.0.0.1 / ::1.
- Bun's compile-to-binary (`bun build --compile`) — not implemented.
- HTTP/2 server, Unix-domain sockets — not implemented.
- Tools that depend on V8 stack format (`at name (url:line:col)`) — JSC
  format is `name@url:line:col`.

---

## 3. Language

| Feature | Status | Notes |
|---|---|---|
| ES2022+ syntax via JavaScriptCore | ✅ | All JSC-supported syntax. |
| TypeScript stripping (oxc) | ✅ | Decorators, enums work; `experimentalDecorators` honoured. |
| TSX / JSX | ✅ | `.tsx` auto-JSX; `.js` with `</` triggers JSX too. Classic `React.createElement` runtime. |
| ESM static `import` / `export` | ✅ | Hoisted by the rewriter so cycles resolve. All forms: named, default, namespace, renamed, `export *`, `export { x } from`, `export * as ns from`. |
| Dynamic `import()` | ✅ | Returns `{ __esModule, default, …named }`. |
| Top-level `await` | ✅ | Every module wrapped in `async function`. |
| `using` / `await using` (ES2026) | ✅ | Lowered to try/finally by oxc before JSC sees them. |
| `node_modules` resolution | ✅ | `oxc_resolver` — handles `exports`, `main`, conditions. |
| CJS / ESM interop | ✅ | `require("esm-pkg")` and `import x from "cjs-pkg"` both work. |
| `import.meta.url / .filename / .dirname / .main` | ✅ | |
| `__filename` / `__dirname` in CJS | ✅ | Injected as `var` so `const __filename = …` in ESM also works. |
| Import attributes `with { type: "json" }` | ✅ | Strict JSON. |
| Import attributes for `yaml` / `text` / `toml` | ❌ | Rewriter doesn't thread the `with` clause to the loader. Use the file extension instead. |
| Source-map remapping in stack traces | 🟡 | Line numbers OK; columns drift on JSX-heavy files; format is JSC, not V8. |

## 4. Test runner (`bun-rs test`)

| Feature | Status | Notes |
|---|---|---|
| `describe` / `test` / `it` / nested describes | ✅ | |
| Lifecycle: `beforeAll` / `beforeEach` / `afterAll` / `afterEach` | ✅ | |
| `expect` matchers | ✅ | ~120 matchers + `.not` + `.resolves` / `.rejects` + asymmetric (`expect.any`, `objectContaining`, …). |
| `test.each` / `describe.each` (+ `.skipIf` / `.todoIf` / `.if`) | ✅ | |
| `test.only` / `describe.only` | ✅ | |
| `test.failing` | ✅ | Throwing → pass; not-throwing → fail. |
| `test.concurrent` | 🟡 | Recognized; tests still run serially. |
| `jest.fn` / `jest.spyOn` / call tracking | ✅ | |
| `jest.useFakeTimers` / `jest.setSystemTime` | ✅ | Mocks `Date.now` + the runtime's timer queue. sinon-FakeTimers compat. `vi.*` aliases share the same factory. |
| `jest.advanceTimersByTime` / `runAllTimers` | ✅ | |
| `jest.clearAllMocks` / `resetAllMocks` / `restoreAllMocks` | 🟡 | No-op stubs. |
| `mock.module(spec, factory)` | 🟡 | Works if hooked before first import; doesn't re-evaluate already-loaded modules. |
| `toMatchSnapshot` / `toMatchInlineSnapshot` | 🟡 | Snapshot files are created on first call but never diffed or rewritten. |
| Bunfig `[test].preload` | ✅ | |
| Output format | ✅ | `bun test <ver>` header, `✓`/`✗` on stderr, footer counters. |

## 5. `Bun.*`

| API | Status | Notes |
|---|---|---|
| `Bun.serve({ port, fetch, tls? })` | ✅ | HTTP + HTTPS, hyper + tokio per-request, ALPN-negotiated. `routes:` config, reload, ref/unref, dispose, address. |
| `Bun.serve` HTTP/2 | ❌ | |
| `Bun.serve` Unix sockets | 🟡 | URL reports `unix://path`; binds TCP under the hood. |
| `Bun.serve` `server.upgrade` (WebSocket) | 🟡 | Shape-correct objects; no real WS upgrade. |
| `Bun.file(path)` | ✅ | `.text()`, `.json()`, `.bytes()`, `.arrayBuffer()`, `.stream()`, `.slice()`, `.exists()`, `.write()`. |
| `Bun.file(fd)` | 🟡 | Reads work for regular fds; some oversize cases skipped. |
| `Bun.write(dest, data)` | ✅ | string / Buffer / Uint8Array / Blob / Response / `Bun.file`. |
| `Bun.spawn` / `Bun.spawnSync` | 🟡 | Sync path solid. Async path now drains stdout as a real stream (batch 210). **`proc.stdin` writable is not exposed** — tests that write after spawn fail. Timeout / SIGTERM supported. |
| `Bun.$` (tagged template shell) | 🟡 | Delegates to `sh -c`. Supports `${{raw}}`, `.cwd`, `.env`, `.quiet`, `.throws`. Awaited result has both Promise-returning and sync `text()` / `json()` / `blob()` / `arrayBuffer()` (batches 200/214). No real lexer/parser. |
| `Bun.Glob` | ✅ | `globset` + `walkdir`. |
| `Bun.password.hash` / `.verify` | 🟡❌ | Argument shape matches Bun; **actual hash is HMAC-SHA256, not argon2 / bcrypt**. |
| `Bun.CryptoHasher` / `Bun.SHA1` / `Bun.SHA256` / `Bun.MD5` / etc. | ✅ | All node:crypto algos plus sha3-{224,256,384,512}, blake2b/s, ripemd160, md4. `copy()` replays updates. |
| `Bun.randomUUIDv5` | ✅ | RFC 4122 SHA-1. |
| `Bun.dns.lookup` / `.prefetch` / `.getCacheStats` | 🟡 | `lookup` returns `[{address, family, ttl}]`. Cache stats reflect prefetch + fetch hits. Resolution is stubbed to 127.0.0.1 / ::1. |
| `Bun.YAML.parse` / `.stringify` | 🟡 | serde_yaml; expands `<<` merge keys; no YAML 1.2 octal/hex literals; single-document only. |
| `Bun.TOML.parse` | ✅ | toml crate. |
| `Bun.JSON5` / `Bun.JSONC` | ✅ | json5 crate. |
| `Bun.JSONL.parse` / `.parseChunk` | ✅ | Stream-style; rejects 1 GB+ inputs. |
| `Bun.markdown` | ✅ | pulldown-cmark. |
| `Bun.Transpiler` | ✅ | oxc. `tsconfig` JSX factory is partial. |
| `Bun.stripANSI`, `Bun.sliceAnsi`, `Bun.wrapAnsi`, `Bun.stringWidth` | 🟡 | ANSI-aware skip; CJK width 2; basic emoji width; no grapheme clusters / ZWJ / regional indicators. |
| `Bun.Cookie` / `Bun.CookieMap` | ✅ | RFC 6265 attribute order. |
| `Bun.semver`, `Bun.color`, `Bun.deepEquals`, `Bun.gc`, `Bun.allocUnsafe`, `Bun.cron`, `Bun.HMAC`, `Bun.CSRF` | ✅ | |
| `Bun.generateHeapSnapshot("v8")` | 🟡 | Emits a minimal valid V8 snapshot (1 node, 1 edge); enough for `v8-heapsnapshot` to parse. |
| `Bun.S3Client` | 🟡 | Surface only. `file()` / `write()` / `presign()` throw. |
| `Bun.listen` / `Bun.connect` (TCP / TLS) | ❌ | Shape stubs; real I/O throws. |
| `Bun.Image` | ❌ | |
| `Bun.FileSystemRouter` | ❌ | |
| `Bun.SQL` / `Bun.RedisClient` | ❌ | |
| `Bun.plugin` | 🟡 | Registers loaders; most virtual-module behaviour unimplemented. |
| `Bun.shellInternals.parse` / `Bun.$.lex` | ❌ | Naive tokenizer only. |
| `Bun.env`, `Bun.version`, `Bun.revision`, `Bun.sleep`, `Bun.sleepSync` | ✅ | |
| `bun:sqlite` | ✅ | rusqlite. |
| `bun:ffi` | 🟡 | `dlopen` + primitive types via libffi. No structs / callbacks. |

## 6. `node:*`

| Module | Status | Notes |
|---|---|---|
| `node:fs` (sync + promises + streams) | ✅ | Includes `ReadStream` / `WriteStream` / `Stats` / `Dirent`. `fs.promises` is **genuinely async** (tokio `spawn_blocking`). |
| `node:path` (posix + win32) | ✅ | |
| `node:os` | ✅ | platform / arch / type / release / hostname / cpus / totalmem / userInfo / EOL. |
| `node:buffer` | ✅ | Zero-copy from Rust; base64url, copyBytesFrom, allocUnsafeSlow. |
| `node:events` | ✅ | Full EventEmitter + `once` + `on` (async iterator). |
| `node:util` | ✅ | `promisify`, `callbackify`, `inspect`, `format`, `debuglog`, `types.isX`, `inherits`, `parseArgs`. |
| `node:crypto` | ✅ | createHash, createHmac, randomBytes, randomUUID, randomInt, timingSafeEqual, getRandomValues. `webcrypto` namespace is a stub. |
| `node:child_process` | 🟡 | `spawnSync` / `execSync` solid. Async `spawn` now drains stdout; **stdin writable still missing**; `fork` stub. |
| `node:assert` (+ `/strict`) | ✅ | |
| `node:querystring` | ✅ | |
| `node:url` | ✅ | URL, URLSearchParams, fileURLToPath, pathToFileURL. |
| `node:stream` (+ `/web` + `/promises` + `/consumers`) | ✅ | Readable / Writable / Duplex / PassThrough, pipeline / finished, Web Streams interop. |
| `node:readline` (+ `/promises`) | ✅ | |
| `node:zlib` | ✅ | gzip / deflate / brotli via flate2 + brotli. |
| `node:http` / `node:https` | 🟡 | Server wraps `Bun.serve`; client wraps `fetch`. No real `Agent`, no keep-alive pool tuning. |
| `node:tty` | 🟡 | `isatty`, `ReadStream` / `WriteStream` shape. |
| `node:net` | ❌ | `Socket` / `Server` / `connect` / `createServer` throw. `BlockList`, `SocketAddress`, default-auto-select-family helpers are shape stubs. |
| `node:tls` | ❌ | `connect` throws. SecureContext etc. are stubs. |
| `node:dns` (+ `/promises`) | 🟡 | Resolution returns 127.0.0.1 / ::1. `setServers` validates. No real resolver. |
| `node:dgram` | ❌ | |
| `node:worker_threads` | ❌ | `Worker` class throws. |
| `node:vm` | 🟡 | `runInNewContext` via `new Function`. No real isolation. |
| `node:perf_hooks` | 🟡 | `performance.now`, basic histogram stubs. |
| `node:v8` | 🟡 | Heap snapshot via `Bun.generateHeapSnapshot`; serializer / deserializer are stubs. |
| `node:cluster`, `node:domain`, `node:async_hooks`, `node:repl`, `node:sea` | 🟡 | Shape stubs; `AsyncLocalStorage` does work. |
| `node:test`, `node:diagnostics_channel`, `node:inspector`, `node:trace_events`, `node:wasi` | 🟡 | Shape stubs. |

## 7. Web globals

| API | Status | Notes |
|---|---|---|
| `console.{log,info,warn,error,debug,trace,dir}` | ✅ | |
| `process.{argv,env,cwd,exit,platform,arch,pid,versions,config,release,features}` | ✅ | |
| `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` / `setImmediate` | ✅ | Compatible with `jest.useFakeTimers`. |
| `queueMicrotask` / `process.nextTick` | ✅ | |
| `fetch` | ✅ | reqwest + rustls; non-blocking; honours `AbortSignal`. |
| `Request` / `Response` / `Headers` | ✅ | `Response.body` is a real stream. |
| `URL` / `URLSearchParams` | ✅ | Rust `url` crate. |
| `Blob` / `File` | ✅ | |
| `FormData` | 🟡 | Class works; no `multipart/form-data` Request-body serialization with WebKit boundary. |
| `TextEncoder` / `TextDecoder` (incl. `encodeInto`) | ✅ | UTF-8. |
| `atob` / `btoa` | ✅ | |
| `Buffer` (Node-compatible, extends Uint8Array) | ✅ | Zero-copy from Rust. |
| `ReadableStream` / `WritableStream` / `TransformStream` (+ BYOB reader, `pipeTo` / `pipeThrough` / `tee` / `ReadableStream.from`) | ✅ | |
| `WebSocket` (client) | ✅ | Text + binary, custom close codes. |
| `AbortController` / `AbortSignal` (+ `.timeout` / `.any`) | ✅ | `fetch` AbortSignal fires at request setup, not mid-stream. |
| `crypto` / `crypto.subtle` | 🟡 | Random + digest. No key import / sign / verify / derive. |
| `structuredClone` | ✅ | |
| `Worker` (web global) | ❌ | |
| `BroadcastChannel` / `MessageChannel` / `MessagePort` | ❌ | |
| `EventSource` | ❌ | |

## 8. CLI

| Command | Status | Notes |
|---|---|---|
| `bun-rs <file>` / `bun-rs run <file>` | ✅ | |
| `bun-rs -e <code>` / `-p <code>` | ✅ | Wrapped in async IIFE so top-level `await` works. |
| `bun-rs test [paths]` | ✅ | |
| `bun-rs build <entry> [-o out]` | 🟡 | Single-file ESM only. No tree shaking, code splitting, plugins, minify, sourcemap. |
| `bun-rs install` | 🟡 | Downloads from registry. No lockfile, no peer deps, no workspaces. Loose semver (^/~ stripped to exact). |
| REPL (no args) | 🟡 | Basic line editor with `JSCheckScriptSyntax`-driven multi-line continuation. No completion / syntax highlighting. |
| `bun-rs init` / `add` / `remove` / `upgrade` | ❌ | |
| `bun-rs run <package-script>` | ❌ | |
| `bun-rs <file> --inspect` / `--watch` | ❌ | |

## 9. Platforms

- ✅ **macOS** (system `JavaScriptCore.framework`).
- 🟡 **Linux** (`libjavascriptcoregtk-4.1`) — build set up, lightly smoke-tested.
- ❌ **Windows** — no public JSC build; would need to compile WebKit ourselves.

## 10. Deliberate non-goals

The following are **out of scope** for the foreseeable future, not "todo":

- JIT tier optimisation (Baseline / DFG / FTL). We use stock JSC.
- Bun's `bun build --compile`.
- Bun's own shell parser. We exec via system `sh -c`.
- Worker-thread JSC isolates with shared state. Different JSC contexts are
  not reused across threads in our binding.
- Full Node compatibility for low-level networking (raw TCP/UDP/TLS).
- Bun's own bundler / minifier / CSS pipeline.

## 11. Biggest unlocked-tests levers, descending

If you wanted to push the 38.9 % number up, these are the largest single
levers, in rough order of impact:

1. Async `Bun.spawn` writable stdin stream — many spawn/IPC tests.
2. Import attributes (`with { type: "..." }`) for non-JSON — text /
   YAML / TOML loader fixtures.
3. `Bun.build` as a real bundler — `bundler.test`, `css.test`,
   snapshot-tests.
4. `mock.module` cache invalidation — `mock/*.test.ts` files.
5. `Worker` / `node:worker_threads` — `Bun.isMainThread`, IPC tests.
6. Real `node:net` / `node:tls` — http server tests, fetch behaviour tests.
7. V8-style stack format — `stack.test.ts`, error-rendering tests.
8. WebSocket server upgrade in `Bun.serve` — WS server tests.

---

## See also

- [`tutorial.md`](tutorial.md) — runnable walk-through of the supported surface.
- [`guide.md`](guide.md) — fuller API reference with examples.
- [`roadmap.md`](roadmap.md) — what's planned next.
- [`plan.md`](plan.md) — the day-by-day build log.
