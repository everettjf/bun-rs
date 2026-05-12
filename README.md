# bun-rs

A Rust port of [Bun.js](https://github.com/oven-sh/bun), backed by JavaScriptCore via FFI.

**Status:** 0.1.0 — runs TypeScript + ESM, has a useful Web / `node:*` /
`Bun.*` surface, async I/O via tokio. See:

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
- **`fetch`** — async via tokio + reqwest (rustls); does not block the JS thread
- **`Buffer`** (Node-compatible, extends Uint8Array, zero-copy from Rust)
- `URL` / `URLSearchParams` (parsing via Rust `url`)
- `Headers` / `Request` / `Response`
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

### `Bun.*` namespace

- **`Bun.serve({ port, fetch })`** — minimal HTTP server (tiny_http, sync handler)
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

## What's still missing

- **Stream APIs** (`ReadableStream` / `WritableStream` / `node:stream`)
- **HTTPS** in Bun.serve, **HTTP/2**, **WebSocket**
- **Async `node:fs.promises`** (currently the Promises namespace just re-exports sync — works but blocks the thread)
- **`Bun.serve` concurrency** (currently one request at a time on the JS thread; tokio is wired but serve still uses tiny_http)
- **Live ESM bindings** (`import` is currently a value snapshot at load time)
- **Sourcemap-aware error stacks** (errors point to rewritten lines, not source)
- **Worker / Cluster**
- **bundler** / **package manager** (`bun install`, `bun build`)
- **shell / SQL / bake**

## Layout

```
crates/
  bun-cli/         entrypoint binary
  bun-runtime/     event loop + globals (console / process / timers /
                                          modules / web / Bun.* / node:*)
  bun-jsc-sys/     raw JSC C API FFI
  bun-jsc/         safe RAII wrapper
  bun-transpile/   oxc-powered TS/JSX → JS
  bun-loader/      path resolver + ESM → IIFE rewriter
```

## Build & test

```sh
cargo build --workspace             # debug build
cargo build --release -p bun-cli    # ~3.5MB single binary
cargo test --workspace              # 90+ tests, all green on macOS arm64
```

Toolchain: nightly Rust (oxc uses `if let` match guards). See
[`rust-toolchain.toml`](rust-toolchain.toml).

## License

MIT (matches Bun).
