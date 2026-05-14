# bun-rs

> ⚠️ **Experimental hobby project — very rough, very incomplete.**
>
> The goal is to fully rewrite [Bun](https://github.com/oven-sh/bun) in Rust.
> (Yes, the official Bun project has already rewritten itself in Rust — this repo is unrelated to that effort and exists purely as a personal toy / for-fun project.)
>
> Start here:
> - [`docs/capabilities.md`](docs/capabilities.md) — what works today, what's a stub, what throws
> - [`docs/tutorial.md`](docs/tutorial.md) — runnable walk-through of the things that actually work

A Rust port of [Bun.js](https://github.com/oven-sh/bun), backed by JavaScriptCore via FFI.

**Where it stands:** Bun's official `test/js/bun` file-level suite passes
**159 / 409 (38.9 %)** as of the last run. The core JS runtime (TS + ESM
+ event loop + fetch + Bun.serve + Bun.file + test runner) is solid; lots
of surface (raw TCP, real argon2, real DNS, worker threads, server-side
WebSocket) is intentionally a stub or simply missing.

What you can actually do with it today:

- `bun-rs run app.ts` — TypeScript + ESM (static + dynamic `import()` +
  top-level `await` + `node_modules` + CJS interop)
- `bun-rs test` — Jest-style runner with `expect` matchers,
  `jest.fn`, `jest.useFakeTimers`, `jest.setSystemTime`
- `bun-rs build app.ts -o out.js` — single-file bundler (no tree shake / plugins)
- `bun-rs install` — npm package installer (loose semver, no lockfile)
- HTTP / HTTPS server via `Bun.serve` (+ `node:http`)
- `fetch`, `URL`, `Buffer`, WebSocket **client**, AbortController, full WHATWG Streams
- `bun:sqlite`, `bun:ffi` (primitives only)
- `node:` modules: fs (sync + real-async promises) / path / os / buffer /
  events / util / crypto / child_process / assert / querystring / url /
  stream / readline / zlib / http

See:

- [`docs/tutorial.md`](docs/tutorial.md) — 30-minute walk-through
- [`docs/guide.md`](docs/guide.md) — full reference: what works, what doesn't
- [`docs/roadmap.md`](docs/roadmap.md) — what's coming in 0.2 / 0.3 / 1.0
- [`docs/build.md`](docs/build.md) — build prereqs for macOS / Linux
- [`CHANGELOG.md`](CHANGELOG.md) — version history

## Quick start

```sh
cargo build --release
./target/release/bun-rs --version
./target/release/bun-rs                    # REPL
./target/release/bun-rs -e "console.log(1+1)"
./target/release/bun-rs -p "40 + 2"        # 42
./target/release/bun-rs run app.ts         # multi-file TS + ESM
./target/release/bun-rs run server.ts      # Bun.serve, fetch, ...
./target/release/bun-rs test               # runs *.test.ts files
./target/release/bun-rs build app.ts -o bundle.js   # single-file bundle
```

## What works

### Language

- TypeScript / TSX transpiled via [oxc](https://oxc.rs/)
- ESM:
  - Static `import` / `export` (named, default, namespace, renamed, `export *`,
    `export { x } from`, `export * as ns from`)
  - **Dynamic `import()`**, **top-level `await`**
  - Circular imports (CJS-style snapshot), diamond shared deps
  - `node:` builtins, `node_modules` lookup via `oxc_resolver`
- `import.meta.url` / `.filename` / `.dirname` / `.main`
- Native async / await + Promise resolution
- REPL with multi-line continuation

### Built-in globals

- `console.{log,info,warn,error,debug,trace,dir}`
- `process.{argv,env,cwd,exit,platform,arch,pid,versions}`
- `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval`
- `queueMicrotask`
- **`fetch`** — async via tokio + reqwest (rustls); does not block the JS thread; honors `AbortSignal`
- **`Buffer`** (Node-compatible, extends Uint8Array, zero-copy from Rust)
- **`ReadableStream` / `WritableStream` / `TransformStream`** + `pipeTo` / `pipeThrough` / `tee` / `ReadableStream.from`
- **`WebSocket`** (client, text + binary, custom close codes)
- **`AbortController` / `AbortSignal`** (including `.timeout` / `.any`)
- `URL` / `URLSearchParams` (parsing via Rust `url`)
- `Headers` / `Request` / `Response` (`Response.body` is a real stream)
- `TextEncoder` / `TextDecoder` (UTF-8)
- `atob` / `btoa`

### `node:` modules

| Module | Coverage |
|---|---|
| `node:path` | join / resolve / normalize / dirname / basename / extname / isAbsolute / relative, posix + win32 |
| `node:os` | platform / arch / type / release / hostname / cpus / totalmem / userInfo / EOL |
| `node:fs` | sync: readFile / writeFile / appendFile / exists / stat / readdir / mkdir / rm / rename / unlink / copyFile / realpath / mkdtemp; **`fs.promises` is genuinely async** (tokio spawn_blocking) |
| `node:buffer` | full `Buffer` class (zero-copy from Rust) |
| `node:events` | full `EventEmitter` |
| `node:util` | promisify / callbackify / inspect / format / debuglog / types.isX / inherits |
| `node:crypto` | createHash (md5/sha1/sha256/sha384/sha512) / createHmac / randomBytes / randomUUID / randomInt / timingSafeEqual |
| `node:child_process` | spawnSync / execSync / exec(cb) |
| `node:assert` | strict + non-strict, deep[Strict]Equal, throws/rejects, match |
| `node:querystring` | parse / stringify / escape / unescape |
| `node:url` | URL, URLSearchParams, fileURLToPath, pathToFileURL |
| `node:stream` | Readable / Writable / Duplex / PassThrough, pipeline / finished, Web Streams interop |
| `node:readline` | createInterface, question/on('line')/on('close'), readline.promises |
| `node:zlib` | gzip / gunzip / deflate / inflate / raw, sync + async |
| `node:http` | createServer + get/request (wraps Bun.serve + fetch) |
| `bun:sqlite` | Database / query / prepare (rusqlite) |
| `bun:ffi` | dlopen + primitive types via libffi |

### `Bun.*` namespace

- **`Bun.serve({ port, fetch, tls? })`** — concurrent HTTP / HTTPS via hyper, tokio per-request (HTTP/2 server is **not** implemented)
- **`Bun.file(path)`** — Blob-like with `text()` / `json()` / `bytes()` / `arrayBuffer()` / `exists()` / `size` / `name` / `type`
- `Bun.write(path, data)`
- `Bun.sleep(ms)`
- `Bun.env`
- `Bun.version` / `Bun.revision`

### Platforms

- ✅ **macOS** (system `JavaScriptCore.framework`)
- 🟡 **Linux** (`libjavascriptcoregtk-4.1`) — build set up, not yet smoke-tested
- ❌ **Windows** — no public JSC build; would need to compile WebKit ourselves

See [`docs/build.md`](docs/build.md).

## What's not supported (yet)

Where bun-rs is **today: 159 / 409 file-level tests pass in Bun's own
`test/js/bun` suite (38.9 %).** The remaining 61 % breaks down into
three categories: deliberately stubbed surfaces, partial implementations,
and small edge-case gaps. The big-ticket items:

### Throws or returns a stub — don't reach for these

- **Raw networking** — `node:net`, `node:tls`, `node:dgram`, `Bun.listen`,
  `Bun.connect` (all shape stubs that throw on real I/O).
- **Workers** — `Worker` (global), `node:worker_threads` both throw.
  `Bun.isMainThread` is always `true`.
- **WebSocket server upgrade** in `Bun.serve` — only the client is real.
- **Real DNS** — `node:dns.lookup` always returns `127.0.0.1` / `::1`.
- **Real password hashing** — `Bun.password.hash` / `.verify` use
  HMAC-SHA256 under the hood, **not argon2 or bcrypt**. Compatible API,
  incompatible bytes.
- **`Bun.SQL`, `Bun.RedisClient`, `Bun.Image`, `Bun.FileSystemRouter`,
  `Bun.S3Client`** — surface only or throw.
- **HTTP/2 server**, **Unix-domain sockets** — not implemented.
- **`bun build --compile`** (compile-to-binary) — not implemented.
- **Windows** — no public JSC build (would need to compile WebKit).

### Behaves but lies on the edges

- **Live ESM bindings** — `import { x }` is a value snapshot at load
  time, not a live binding (matters only for circular dependencies).
- **`fetch` AbortSignal** fires at request setup; doesn't interrupt
  mid-stream.
- **`Bun.spawn` stdin** — `proc.stdin` (Writable) is not exposed. Use
  `Bun.spawnSync({ input: ... })`.
- **Import attributes** — only `with { type: "json" }` works. `yaml`,
  `text`, `toml` aren't threaded to the loader; use the file extension.
- **Sourcemap stacks** map lines, not columns; JSX-heavy files may drift.
- **Stack-trace format** — JSC's `name@url:line:col`, not V8's
  `at name (url:line:col)`. Tests that grep for V8 format won't match.
- **Snapshot testing** — `toMatchSnapshot` creates files on first call
  but never diffs or rewrites them.
- **`mock.module`** cache invalidation — only works if hooked before the
  first import of the module.
- **`bun-rs install`** does loose semver (`^` / `~` stripped to exact);
  no lockfile, no peer deps, no workspaces.
- **CLI gaps** — `bun-rs init`, `add`, `remove`, `upgrade`,
  `run <script>`, `--inspect`, `--watch` are not implemented.

### Deliberate non-goals

- JIT tier optimisation (Baseline / DFG / FTL) — we use stock JSC.
- Bun's own shell parser (we exec via system `sh -c`).
- Worker-thread JSC isolates with shared state.
- Bun's own bundler / minifier / CSS pipeline.

See [`docs/capabilities.md`](docs/capabilities.md) for the full per-API
table and [`docs/roadmap.md`](docs/roadmap.md) for what's planned next.

## Layout

```
crates/
  bun-cli/         entrypoint binary
  bun-runtime/     event loop + globals (console / process / timers /
                                          modules / web / Bun.* / node:* /
                                          test runner)
  bun-jsc-sys/     raw JSC C API FFI
  bun-jsc/         safe RAII wrapper
  bun-transpile/   oxc-powered TS/JSX → JS
  bun-loader/      path resolver + ESM → IIFE rewriter (+ line-map for stacks)
  bun-bundler/     single-file bundler (wraps bun-loader's graph walk)
  bun-install/     npm registry client + tarball extractor
```

## Build & test

```sh
cargo build --workspace             # debug build
cargo build --release -p bun-cli    # ~3.5MB single binary
cargo test --workspace              # 100+ tests, all green on macOS arm64
```

Toolchain: nightly Rust (oxc uses `if let` match guards). See
[`rust-toolchain.toml`](rust-toolchain.toml).

## License

MIT (matches Bun).
