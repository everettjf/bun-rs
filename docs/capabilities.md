# bun-rs Capabilities

A honest snapshot of what's implemented, what's stubbed, and what's missing.
Numbers below come from running Bun's official `test/js/bun` suite against
`bun-rs`. See `docs/tutorial.md` for guided examples of the working
surface.

Last suite run: **159 / 409 files passing (38.9 %)** · 6391 individual tests
passing.

## Conventions

- ✅ **works** — implemented in Rust + JS, passes real-world tests.
- 🟡 **partial / stub** — exposed by name; many calls behave correctly but
  edge cases or full semantics are missing.
- ❌ **not implemented** — calling it throws or returns a placeholder.

The runtime never panics on missing APIs: stubs throw a JS `Error` so user
code can `try / catch`.

---

## 1. Language & module system

| Feature | Status | Notes |
|---|---|---|
| ES2022+ syntax via JavaScriptCore | ✅ | All JSC-supported syntax. Top-level `await` works. |
| TypeScript stripping (oxc) | ✅ | Types removed at load. Decorators / `enum` work; `experimentalDecorators` honoured. |
| TSX / JSX | ✅ | `.tsx` files get JSX automatically. Plain `.js` with `</` triggers JSX too. |
| ES modules — static `import` / `export` | ✅ | Hoisted via the rewriter so cycles resolve. |
| Dynamic `import()` | ✅ | Returns a namespace object (`{ __esModule, default, …named }`). |
| `node_modules` resolution | ✅ | Backed by `oxc_resolver` (handles `exports`, `main`, conditions). |
| CJS / ESM interop | ✅ | `require("esm-pkg")` and `import x from "cjs-pkg"` both work. |
| Import attributes `with { type: "json" }` | ✅ | Strict JSON. |
| Import attributes `with { type: "yaml" / "text" / "toml" }` | ❌ | Rewriter does not yet thread the `with` clause to the loader. |
| `require.resolve` / `require.cache` | 🟡 | `resolve` is best-effort; `cache` is the global module map. |
| `__filename` / `__dirname` (CJS) | ✅ | Injected as `var` so `const __filename = ...` in ESM also works. |
| Source maps (error stacks) | 🟡 | JSC `@url:line:col` format is preserved and remapped to user-original lines. V8 `at funcname (…)` format is not produced. |

## 2. Test runner (`bun-rs test`)

| Feature | Status | Notes |
|---|---|---|
| `describe` / `test` / `it` / hooks | ✅ | Full nesting; `beforeAll` inside a test body fires after the body. |
| `expect` matchers | ✅ | ~120 matchers including `.not`, `.resolves`, `.rejects`, asymmetric (`expect.any`, `objectContaining`, …), `toMatchSnapshot`, `toMatchInlineSnapshot`. |
| Snapshot files | 🟡 | First call creates `__snapshots__/<file>.snap`. Real diffing / write-on-update is not implemented. |
| `test.each` / `describe.each` | ✅ | Including `.skipIf` / `.todoIf` / `.if` chains. |
| `test.failing` | ✅ | Throwing test passes; non-throwing test fails. Timeouts always count as failures. |
| `test.only` / `describe.only` | ✅ | When any `.only` is registered, other tests in the file are skipped. |
| `test.concurrent` | 🟡 | Recognized as a flag (used by `onTestFinished` for the “cannot call here” error) but tests still run serially. |
| `jest.setTimeout` / per-test timeout | ✅ | Uses `Promise.race`. |
| `jest.useFakeTimers` / `jest.setSystemTime` | ❌ | Would require mocking `Date` while preserving identity — JSC has no clean hook. |
| `mock.module(spec, factory)` | 🟡 | Works when the mock call runs before the first import. Re-evaluating already-imported modules is not implemented. |
| Bunfig `[test].preload` | ✅ | |
| Output format | ✅ | Matches Bun: `bun test <ver>` header on stdout, `✓` / `✗` on stderr, footer counters, `Ran N tests across M files.` |

## 3. Bun.* surface

| Feature | Status | Notes |
|---|---|---|
| `Bun.serve` (HTTP + HTTPS) | ✅ | hyper-backed; `routes:` config; `tls.cert` / `tls.key` accept files; reload, ref/unref, dispose, address. |
| `Bun.serve` (HTTP/2) | ❌ | |
| `Bun.serve` Unix sockets | 🟡 | URL reports `unix://path`; the listener still binds TCP under the hood. |
| WebSocket server (`server.upgrade`) | 🟡 | Returns shape-correct objects; real WS upgrade not done. |
| `Bun.serve` `fetch` handler | ✅ | Full request/response round-trip via hyper. |
| `Bun.file(path)` | ✅ | `.text()`, `.json()`, `.bytes()`, `.arrayBuffer()`, `.stream()`, `.slice()`, `.exists()`, `.write()`. |
| `Bun.file(fd)` | 🟡 | Limited — reads work for regular fds, edge cases (oversized slice) skipped. |
| `Bun.write(dest, data)` | ✅ | string / Buffer / Uint8Array / Blob / Response / Bun.file. |
| `fetch` | ✅ | reqwest-backed; redirect / abort / keepalive / body types covered. |
| `Bun.spawn` / `Bun.spawnSync` | 🟡 | Sync path is solid; async streams via node:child_process. `proc.stdin` (Writable) is **not** exposed — tests that write to stdin after spawn fail. Timeout & SIGTERM supported. |
| `Bun.$` (tagged template) | 🟡 | Delegates to `sh -c`. Supports `${{raw}}` interpolation, `.cwd` / `.env` / `.quiet` / `.throws`, `.text()` / `.json()` / `.blob()` (Promises pre-await, sync post-await). Bun-style usage messages for `dirname`, `basename`, `exit`. No real lexer / parser. |
| `Bun.Glob` | ✅ | Backed by `globset` + `walkdir`. |
| `Bun.password.hash` / `.verify` | 🟡 | Argument validation matches Bun; the actual hash is **HMAC-SHA256** (not argon2 or bcrypt). |
| `Bun.CryptoHasher` / `Bun.SHA1`/`SHA256`/`MD5`/etc. | ✅ | All node:crypto algos plus sha3-{224,256,384,512}, blake2b/s, ripemd160, md4. `copy()` replays updates so HMAC state survives. |
| `Bun.randomUUIDv5` | ✅ | RFC 4122 SHA-1. |
| `Bun.dns.lookup` / `.prefetch` / `.getCacheStats` | 🟡 | `lookup` returns `[{address, family, ttl}]`; oversized names reject. Cache stats reflect `prefetch` + `fetch`. |
| `Bun.YAML.parse` / `.stringify` | 🟡 | serde_yaml backed. Expands `<<` merge keys. Doesn't support YAML 1.2 octal/hex literals or multi-document. |
| `Bun.TOML.parse` | ✅ | toml crate. RangeError on deep inline tables. |
| `Bun.JSONL.parse` / `.parseChunk` | ✅ | Stream-style with `{values, read, done, error}`. Rejects 1 GB+ inputs. |
| `Bun.markdown` | ✅ | pulldown-cmark. |
| `Bun.Transpiler` | ✅ | oxc. `tsconfig` JSX factory partial. |
| `Bun.stripANSI`, `Bun.sliceAnsi`, `Bun.wrapAnsi`, `Bun.stringWidth` | 🟡 | ANSI-aware skip, CJK width 2, basic emoji width; grapheme clusters / ZWJ sequences / regional indicators / nested color re-emission are not handled. |
| `Bun.Cookie` / `Bun.CookieMap` | ✅ | RFC 6265 attribute order, FormData-style iteration. Validation of name/value/domain chars is loose. |
| `Bun.S3Client` | 🟡 | Surface only — `file()` / `write()` / `presign()` throw. queueSize clamping works. |
| `Bun.semver`, `Bun.color`, `Bun.deepEquals`, `Bun.gc`, `Bun.allocUnsafe`, … | ✅ | |
| `Bun.generateHeapSnapshot("v8")` | 🟡 | Emits a minimal valid V8 snapshot (1 node + 1 edge), enough to satisfy `v8-heapsnapshot`. Real heap walking is not done. |
| `Bun.listen` / `Bun.connect` (TCP / TLS) | ❌ | Shape stubs only; real I/O throws. |
| `Bun.Image` | ❌ | |
| `Bun.FileSystemRouter` | ❌ | Stub. |
| `Bun.SQL` / `Bun.RedisClient` | ❌ | |
| `Bun.plugin` | 🟡 | Registers loaders but most virtual-module behavior is unimplemented. |
| `Bun.shellInternals.parse` / `Bun.$.lex` | ❌ | Naive tokenizer only. |

## 4. Node built-ins

| Module | Status | Notes |
|---|---|---|
| `node:fs` (sync + promises + streams) | ✅ | Including `ReadStream` / `WriteStream` / `Stats` / `Dirent` classes. |
| `node:path` (posix + win32) | ✅ | |
| `node:os` | ✅ | |
| `node:buffer` | ✅ | base64url, copyBytesFrom, allocUnsafeSlow. |
| `node:events` | ✅ | EventEmitter + once + on. |
| `node:util` (`promisify`, `inspect`, `format`, `types`, `parseArgs`) | ✅ | |
| `node:crypto` | ✅ | Hash, Hmac, randomBytes, randomUUID, timingSafeEqual, getRandomValues; webcrypto sub-namespace stub. |
| `node:child_process` (`spawn`, `spawnSync`, `exec`, `execSync`, `execFile`, `fork`) | 🟡 | All synchronous; `spawn` doesn't expose a writable stdin or real Readable stdout. |
| `node:assert` (+ `/strict`) | ✅ | |
| `node:querystring` / `node:url` (URL/URLSearchParams) | ✅ | |
| `node:stream` (+ `/web` + `/promises` + `/consumers`) | ✅ | |
| `node:readline` (+ `/promises`) | ✅ | |
| `node:zlib` | ✅ | gzip / deflate / brotli via flate2 / brotli. |
| `node:http` / `node:https` | 🟡 | Server works; client uses reqwest under the hood. No Agent class internals, no keep-alive pool tuning. |
| `node:tty` | 🟡 | `isatty`, `ReadStream` / `WriteStream` shape. |
| `node:net` | ❌ | `Socket` / `Server` / `connect` / `createServer` throw. `BlockList`, `SocketAddress`, default-auto-select-family helpers are shape stubs. |
| `node:tls` | ❌ | `connect` throws. SecureContext etc. are stubs. |
| `node:dns` (+ `/promises`) | 🟡 | Resolution returns `127.0.0.1` / `::1`; `setServers` validates input. No real resolver. |
| `node:dgram` | ❌ | |
| `node:worker_threads` | ❌ | `Worker` class throws. |
| `node:cluster`, `node:domain`, `node:async_hooks`, `node:repl`, `node:sea` | 🟡 | Shape stubs; `AsyncLocalStorage` works. |
| `node:perf_hooks` | 🟡 | `performance.now`, basic histogram stubs. |
| `node:vm` | 🟡 | `runInNewContext` via `new Function`. |
| `node:v8` | 🟡 | Heap snapshot via `Bun.generateHeapSnapshot`; serializer / deserializer are stubs. |
| `node:test`, `node:diagnostics_channel`, `node:inspector`, `node:trace_events`, `node:wasi` | 🟡 | Shape stubs. |

## 5. Web globals

| API | Status |
|---|---|
| `fetch`, `Request`, `Response`, `Headers`, `URL`, `URLSearchParams` | ✅ |
| `FormData` | 🟡 (no `multipart/form-data` Request body serialization with WebKit boundary) |
| `Blob`, `File` | ✅ |
| `TextEncoder`, `TextDecoder` (incl. `encodeInto`) | ✅ |
| `ReadableStream`, `WritableStream`, `TransformStream`, byob reader | ✅ |
| `WebSocket` (client) | ✅ |
| `crypto` / `crypto.subtle` | 🟡 (random + digest; no key import / sign / verify / derive) |
| `setTimeout`, `setInterval`, `setImmediate`, `queueMicrotask`, `process.nextTick` | ✅ |
| `AbortController`, `AbortSignal` (+ `timeout`, `any`, `abort`) | ✅ |
| `structuredClone` | ✅ |
| `Worker` | ❌ |
| `BroadcastChannel`, `MessageChannel`, `MessagePort` | ❌ |
| `EventSource` | ❌ |

## 6. CLI

| Command | Status | Notes |
|---|---|---|
| `bun-rs <file>` / `run <file>` | ✅ | |
| `bun-rs -e <code>` / `-p <code>` | ✅ | `-e` wraps in async IIFE so top-level `await` works. |
| `bun-rs test [paths]` | ✅ | |
| `bun-rs build <entry> [-o out]` | 🟡 | Bundles for trivial single-file ESM; no tree shaking, code splitting, plugins, or minify. |
| `bun-rs install` | 🟡 | Resolves and downloads from registry; no lockfile, peerDeps, or workspaces. |
| `bun-rs init` | ❌ | |
| `bun-rs upgrade`, `bun-rs add`, `bun-rs remove`, `bun-rs run <script>` | ❌ | |
| REPL | 🟡 | Basic line editor; no completion / syntax highlighting. |

## 7. What we deliberately don't aim for

- JavaScript JIT optimizations (Baseline / DFG / FTL) — we use stock JSC.
- Bun's compile-to-binary (`bun build --compile`).
- Bun's own shell parser. We exec via system `sh -c`.
- Worker thread isolates with shared global state — JSC contexts are not
  reused across threads in our binding.
- Full Node compatibility for low-level networking (raw TCP/UDP/TLS).

## 8. Known gaps that block the most tests

These are the largest single levers in terms of test files unblocked, in
rough descending order of impact:

1. **Real `Bun.spawn` async stdin/stdout streams** — many spawn/IPC tests.
2. **Import attributes (`with { type: ... }`)** — text-loader, yaml-loader,
   toml-loader fixtures.
3. **`Bun.build` real bundler** — bundler.test, css.test, snapshot-tests.
4. **`mock.module` cache invalidation** — mock/*.test.ts files.
5. **Worker threads** — Bun.isMainThread, IPC.
6. **Real `node:net` / `node:tls`** — http server tests, fetch behaviour tests.
7. **V8-style stack format** — test/stack.test.ts, error-rendering tests.
8. **`jest.setSystemTime` mockable Date** — fake-timers, test-timers.
