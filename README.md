# bun-rs

> **⚠️ This is an experimental hobby project. It is not production-ready and is very rough around the edges.**
>
> The goal is to fully rewrite Bun in Rust as a personal toy project.
> (Note: the [official Bun project](https://github.com/oven-sh/bun) was itself rewritten from Zig to Rust — this repo
> is unrelated to that effort and exists purely for fun and learning.)
>
> See [`docs/capabilities.md`](docs/capabilities.md) for a candid breakdown of what does and does
> not work today.

A Rust port of [Bun.js](https://github.com/oven-sh/bun), backed by JavaScriptCore via FFI.

**Status:** experimental — Bun's official test suite under `test/js/bun` passes
~38.9 % of files (159 / 409) as of the last run. Many APIs exist as best-effort
stubs that pass shape checks but skip the real implementation (e.g. argon2,
real TCP/UDP, JIT, snapshot file diffing).

What you get out of the box:
- `bun-rs run app.ts` — TypeScript + full ESM (static + dynamic
  `import()` + top-level `await` + `node_modules` + CJS interop)
- `bun-rs test` — Jest-style test runner
- `bun-rs build app.ts -o out.js` — single-file bundler
- `bun-rs install` — npm package installer
- HTTP / HTTPS / HTTP/2 server (`Bun.serve` + `node:http`)
- `WebSocket`, `fetch`, `URL`, `Buffer`, full WHATWG Streams
- Threading via `Worker`
- `bun:sqlite` + `bun:ffi`
- Common `node:*` modules: fs/path/os/buffer/events/util/crypto/
  child_process/assert/querystring/url/stream/readline/zlib/http

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

- **`Bun.serve({ port, fetch, tls? })`** — concurrent HTTP / HTTPS / HTTP/2 (hyper, tokio per-request, ALPN-negotiated)
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

## Known limitations (1.0 deliberate scope)

- **Live ESM bindings** — `import { x }` is a value snapshot at load
  time, not a live binding (matters only for circular dependencies)
- **`fetch` AbortSignal** fires at request setup; doesn't interrupt
  mid-stream
- **`bun-rs install`** does loose semver (^/~ stripped to exact);
  reliable for pinned and `latest`, hit-and-miss for ranges
- **`Worker`** doesn't support SharedArrayBuffer / transferables /
  nesting; messages travel as JSON
- **Sourcemap stacks** map lines, not columns; JSX-heavy files may drift
- **macOS + Linux only** — no Windows JSC build available
- **WebSocket** is client-only (server upgrade in `Bun.serve` TBD)
- **shell / SQL beyond bun:sqlite / bake / cluster** — out of scope

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
