# bun-rs guide

This document is the per-API reference for 0.1. For each surface area we list
what works, what's a stub, what's missing, and where bun-rs deliberately
differs from Node.js or Bun.

> **Top-level caveat.** bun-rs is a Rust port targeting a pragmatic subset
> of Node + Bun. Many APIs implement the most common 80% and stop short of
> obscure flags. If you hit something the guide doesn't list, treat it as
> "probably not implemented yet" and file an issue.

Conventions:
- ✅ = works correctly
- 🟡 = partial / has caveats (see the note)
- ❌ = not implemented

---

## CLI

| Command | Status | Notes |
|---|---|---|
| `bun-rs <file>` | ✅ | Bare file shorthand for `run` |
| `bun-rs run <file>` | ✅ | |
| `bun-rs -e <code>`, `--eval` | ✅ | Inline source; does **not** go through the module loader, so `import` syntax inside `-e` won't work |
| `bun-rs -p <expr>`, `--print` | ✅ | Like `-e` but prints the value |
| `bun-rs --version`, `-v` | ✅ | |
| `bun-rs --help`, `-h` | ✅ | |
| `bun-rs` (no args) | ✅ | REPL with multi-line continuation |

The script's argv shows up in `process.argv` starting at index 2.

---

## Language / module system

| Feature | Status | Notes |
|---|---|---|
| `.ts` / `.tsx` / `.jsx` transpile | ✅ | Via oxc. Type annotations stripped; JSX lowered to classic `React.createElement` (no runtime auto-import) |
| `.mjs` / `.cjs` | ✅ | Run as ESM either way (CJS-specific globals like `require` are not provided) |
| `import` / `export` (static) | ✅ | Named, default, namespace, renamed, side-effect-only, `export * from`, `export { x } from`, `export * as ns from` |
| Dynamic `import()` | ✅ | |
| Top-level `await` | ✅ | |
| `import.meta.url / filename / dirname / main` | ✅ | `url` is a `file://` URL, `main` is `false` (we don't yet distinguish entry from imports) |
| node_modules resolution | ✅ | Via `oxc_resolver` with conditions `["bun", "import", "default", "node"]` |
| `package.json` `exports` / `main` / `module` | ✅ | |
| Circular imports | 🟡 | Doesn't deadlock, but bindings are a snapshot at import time (CJS-style). True live bindings are P2 work |
| `require()` | ❌ | All code runs as ESM. If you need `require`, write `await import(...)` |
| Source maps in error stacks | 🟡 | Stack frames are remapped to the user's `.ts` line numbers; columns are dropped. JSX-heavy files may have slight drift because oxc's transpile can shift lines. Synthetic frames (from generated import shims) get tagged `<bunrs-internal>`. |

---

## Built-in globals

### `console`

`log`, `info`, `warn`, `error`, `debug`, `trace`, `dir`. All work. ✅

`warn` and `error` write to **stderr**; the rest write to stdout. Objects
are formatted via `JSON.stringify`; functions / errors via `toString`.
Format-string substitution (`console.log("hi %s", x)`) is **not** wired
in — pass concatenated strings if you need that.

### `process`

| Field / method | Status | Notes |
|---|---|---|
| `process.argv` | ✅ | `["bun-rs", "<script>", …script args]` |
| `process.env` | ✅ | Plain object copy of env at startup; mutations are local |
| `process.cwd()` | ✅ | |
| `process.exit(code?)` | ✅ | Flushes stdio first |
| `process.platform` | ✅ | `"darwin"` / `"linux"` |
| `process.arch` | ✅ | `"arm64"` / `"x64"` |
| `process.pid` | ✅ | |
| `process.versions.bun` | ✅ | |
| `process.stdout.write(s)` | ✅ | Accepts string or Uint8Array/Buffer |
| `process.stderr.write(s)` | ✅ | |
| `process.stdin.read()` | 🟡 | Returns `null` (no input support yet) |
| `process.stdout.isTTY / columns / rows` | 🟡 | isTTY/columns work on Unix; rows is hardcoded to 24 |
| `process.hrtime()` | 🟡 | Returns `[secs, nanos]` against UNIX epoch, not a monotonic anchor — fine for diffs, not for absolute monotonic timing |
| `process.nextTick(fn)` | ✅ | Implemented as `queueMicrotask` |
| `process.kill` / `chdir` / `umask` / signals / `on()` | ❌ | |

### Timers

| | Status | Notes |
|---|---|---|
| `setTimeout(fn, ms)` | ✅ | |
| `setInterval(fn, ms)` | ✅ | |
| `clearTimeout(id)` / `clearInterval(id)` | ✅ | |
| `queueMicrotask(fn)` | ✅ | |
| `setImmediate` | ❌ | Use `setTimeout(fn, 0)` |

### Web Platform

| | Status | Notes |
|---|---|---|
| `fetch(url, init?)` | ✅ | Async via tokio + reqwest (rustls). Supports GET/POST/PUT/PATCH/DELETE/HEAD, headers, string + Uint8Array bodies. No streaming uploads, no AbortController yet. |
| `URL`, `URLSearchParams` | ✅ | Parsed via Rust `url` crate. Setters for `protocol/host/...` are stubs that don't actually re-parse. |
| `Headers` | ✅ | WHATWG-compatible Map with case-insensitive keys. `append` concatenates with `, ` like the spec. |
| `Request`, `Response` | ✅ | Constructors + `text()` / `json()` / `bytes()` / `arrayBuffer()` body consumption. `Response.json(obj)` static helper. `Response.redirect` and `Response.error` minimal. No streaming bodies. |
| `TextEncoder` / `TextDecoder` | ✅ | UTF-8 only |
| `atob` / `btoa` | ✅ | Via the Buffer polyfill |
| `Buffer` | ✅ | Extends `Uint8Array`; from-string utf8/hex/base64/ascii/latin1; toString same. Buffer.alloc/concat/equals/toJSON/write/slice/copy. **Reading file bytes is zero-copy from Rust** (no String round-trip). |
| `Blob`, `File`, `FormData` | ❌ | |
| `WebSocket` | ❌ | |
| `AbortController` / `AbortSignal` | ❌ | |
| `ReadableStream` / `WritableStream` / `TransformStream` | 🟡 | Spec subset: default reader/writer, `pipeTo` / `pipeThrough` / `tee`, `ReadableStream.from(iter)`, async-iteration. **No** BYOB readers, no byte streams (`ReadableByteStreamController`). `Response.body` is a stream. |
| `crypto.subtle` | ❌ | Use `node:crypto` instead |
| `performance.now()` | ❌ | Use `Date.now()` or `process.hrtime()` |

---

## `Bun.*` namespace

| | Status | Notes |
|---|---|---|
| `Bun.serve({ port, fetch })` | 🟡 | Works for the common case. Requests are processed one at a time on the JS thread; while a handler is running on the JS thread, other requests wait in a queue. (Within a handler, `await fetch(...)` etc. still works because tokio runs the I/O.) No HTTPS, no WebSocket upgrade, no streaming bodies. The returned `Server` exposes `port`, `hostname`, `url`, `stop()`. |
| `Bun.file(path)` | ✅ | `text()` / `json()` / `bytes()` / `arrayBuffer()` / `exists()` / `size` / `name` / `type` (MIME guess from extension). Reads are zero-copy. |
| `Bun.write(dest, data)` | ✅ | Accepts string or Uint8Array/Buffer. |
| `Bun.sleep(ms)` | 🟡 | **Blocking** — uses `std::thread::sleep` on the JS thread. Don't use inside HTTP handlers if you care about throughput; prefer `await new Promise(r=>setTimeout(r, ms))`. |
| `Bun.env` | ✅ | Same shape as `process.env`. |
| `Bun.version` / `Bun.revision` | ✅ | |
| `Bun.password.*` / `Bun.spawn` / `Bun.shell` / `bun:sqlite` / `bun:ffi` | ❌ | |

---

## `node:*` modules

To import a builtin you can use either the `node:` prefix or the bare name:

```ts
import path from "node:path";
import path from "path";    // also works
```

Default export is the module object itself, so `import x from "node:foo"`
and `import * as x from "node:foo"` give equivalent shapes.

### `node:path` ✅

`join`, `resolve`, `normalize`, `dirname`, `basename`, `extname`,
`isAbsolute`, `relative`, `sep`, `delimiter`, plus `path.posix` and
`path.win32` sub-namespaces.

Missing: `parse` / `format` / `toNamespacedPath`.

### `node:os` 🟡

`platform`, `arch`, `type`, `release`, `hostname`, `homedir`, `tmpdir`,
`EOL`, `totalmem`, `freemem`, `uptime`, `cpus`, `userInfo`, `constants`,
`networkInterfaces`.

- `cpus()` returns the correct count but `model`/`speed`/`times` are
  placeholders.
- `freemem()` reports `totalmem` (real free-memory probe is pending).
- `networkInterfaces()` returns `{}`.
- `uptime()` returns seconds since UNIX epoch, not process uptime.

### `node:fs` ✅ (sync) / ✅ (async)

Sync — `readFileSync` (Buffer when no encoding; utf-8 / hex / latin1 / ascii when given a string encoding), `writeFileSync` (accepts string or Buffer/Uint8Array), `appendFileSync`, `existsSync`, `statSync` (with `isFile()` / `isDirectory()` / `isSymbolicLink()` method forms AND field forms), `readdirSync`, `mkdirSync` (`{ recursive }`), `rmSync` (`{ recursive }`), `unlinkSync`, `renameSync`, `copyFileSync`, `realpathSync`, `mkdtempSync`.

Async (`fs.promises`) — `readFile`, `writeFile`, `appendFile`, `unlink`,
`mkdir`, `rm`, `readdir`, `stat`, `copyFile`, `rename`, `realpath`.
**Properly off the JS thread** via tokio `spawn_blocking`.

Missing: `open` / file descriptors, `watch`, `createReadStream` /
`createWriteStream`, `chmod` / `chown`, `link` / `symlink` / `readlink`,
glob, recursive stat, the callback-style `fs.readFile(path, cb)`.

### `node:buffer` ✅

Re-exports the global `Buffer` class.

### `node:events` ✅

`EventEmitter` with `on` / `once` / `off` / `emit` / `addListener` /
`removeListener` / `removeAllListeners` / `listeners` / `listenerCount` /
`eventNames` / `setMaxListeners` / `getMaxListeners` /
`prependListener` / `prependOnceListener`. Throws on unhandled `'error'`
events. Static `EventEmitter.defaultMaxListeners` = 10.

Missing: `once(emitter, name)` static helper, `on(emitter, name)` async
iterator helper, `captureRejections`.

### `node:util` ✅

`promisify`, `callbackify`, `inspect` (with depth + circular detection),
`format` (%s/%d/%j/%o/%O/%i/%f), `debuglog` (gated on `NODE_DEBUG`),
`types.{isArrayBuffer,isUint8Array,isDate,isMap,isSet,isRegExp,isPromise,isAsyncFunction,isNativeError,isTypedArray}`,
`inherits`, `deprecate` (no-op wrapper).

Missing: full `inspect` color / table support, `util.parseArgs`,
`util.styleText`, `util.MIMEType`.

### `node:crypto` ✅

`createHash` (md5, sha1, sha256, sha384, sha512), `createHmac` (same
algorithms). Returned objects support chained `.update().digest()` with
encoding strings (`hex`, `base64`, `base64url`, `latin1`, `utf-8`) or a
Buffer if no encoding given.

`randomBytes(n)` → Buffer. `randomUUID()` → v4 string. `randomInt(min?, max)`.
`timingSafeEqual(a, b)` (constant-time via `subtle`). `getHashes()`.

Missing: ciphers (`createCipheriv` / `createDecipheriv`), `pbkdf2` /
`scrypt`, asymmetric (`generateKeyPair`, sign / verify), `webcrypto`.

### `node:child_process` 🟡

`spawnSync(cmd, args?, opts?)` → `{ status, signal, pid, stdout, stderr }`.
`execSync(command, opts?)` → string or Buffer based on `encoding`.
`exec(command, callback)` — runs synchronously on the calling thread but
delivers the result via the callback (not truly async yet).

Missing: `spawn()` returning a `ChildProcess` EventEmitter, IPC, stream
stdio, kill / signals.

### `node:assert` ✅

`assert(value, message)` / `ok`, `equal`, `notEqual`, `strictEqual`,
`notStrictEqual`, `deepEqual`, `notDeepEqual`, `deepStrictEqual`,
`notDeepStrictEqual`, `throws`, `doesNotThrow`, `rejects`, `doesNotReject`,
`match`, `doesNotMatch`, `fail`, `ifError`. The `strict` namespace
(`import { strict as assert } from "node:assert"`) is wired so `equal`
/ `deepEqual` route to the strict variants.

Errors are `AssertionError` with `actual` / `expected` / `operator` /
`code = "ERR_ASSERTION"`.

### `node:querystring` ✅

`parse(s, sep?, eq?, opts?)`, `stringify(obj, sep?, eq?)`, `escape`,
`unescape`. Multi-value keys come back as arrays.

### `node:url` ✅

Re-exports `URL` and `URLSearchParams` plus `fileURLToPath(url)` and
`pathToFileURL(path)` (returns a URL instance).

### `node:stream` ✅

`Readable`, `Writable`, `Duplex`, `PassThrough`, `Transform` (alias of
Duplex). EventEmitter-based. Supports `push` / `read` / `pipe` / `pause`
/ `resume` / `destroy`, `for await...of`, `Readable.from(iter)`. Web
Streams interop via `Readable.toWeb` / `Readable.fromWeb` (same on
Writable). `pipeline(...streams, cb?)` and `finished(s, cb?)` helpers.

### `node:fs` streaming ✅

`createReadStream(path, opts?)` returns a Node Readable that streams
the file in `highWaterMark`-sized chunks (default 64KB) via tokio
`spawn_blocking`. `createWriteStream(path)` returns a Writable that
writes synchronously on a per-chunk callback. Both work with `pipe()`.

### Other `node:*` ❌

Not present yet: `net`, `http`, `https`, `tls`, `dns`, `zlib`,
`tty`, `readline`, `worker_threads`, `cluster`, `inspector`,
`async_hooks`, `v8`, `vm`, `repl`, `domain`, `dgram`.

---

## Differences from Node and from "real" Bun

These are deliberate, not accidental.

| | Node | Bun | bun-rs 0.1 |
|---|---|---|---|
| Default file type | CJS unless `.mjs` or `"type":"module"` | ESM | **Always ESM** (we don't ship a CJS loader) |
| `Buffer.from(number)` | Throws (security) | Throws | Throws |
| `fs.readFileSync` w/o encoding | Buffer | Buffer | **Buffer** (was String pre-0.1) |
| Built-in tests | none | `bun test` | none yet |
| Bundler | none | `bun build` | none |
| Package manager | npm | `bun install` | none |
| JS engine | V8 | JSC (vendored) | JSC (**system framework** on macOS, distro lib on Linux) |
| HTTP/2 | yes | yes | no |
| HTTPS server | yes | yes | no |

---

## Performance notes

Release build is 3.5MB. Cold-start to running a 30-line TS file is ~10ms
on M-series Macs.

Where bun-rs is slow (relative to V8 / a vendored JSC):
- **Starting up JSC**: macOS's system framework is slightly slower than a
  custom build with profile-guided optimization (Bun's bundled JSC).
- **`Bun.serve` throughput**: handlers serialize on the JS thread; QPS for
  a sub-millisecond handler is bound by single-thread JS execution.
  Concurrent handler dispatch is on the roadmap.
- **Big binary `fetch` bodies**: we read everything into memory before
  resolving (no streaming).

Where bun-rs is comparable:
- **JS execution**: same engine as Safari/Bun. Loops and most builtins
  run at full speed.
- **TS transpile**: oxc is one of the fastest TS parsers; transpile of a
  3kLoC file is sub-10ms.

---

## Troubleshooting

### `error: Uncaught Can't find variable: foo`
Likely you're using a Web API or Node global we don't expose. Check this
guide for ❌ entries.

### `error: cannot find module 'X' from 'Y'`
- Did you forget the file extension? (We try `.ts/.tsx/.js/.jsx/.mjs/.cjs/.json` and `index.<ext>`.)
- For `node_modules`, make sure `package.json` has a `main` or `exports` field.

### `unexpected import` when using `-e`
`-e` is a flat eval — no module loader. Save the snippet to a file and
`run` it instead.

### Script exits 0 but my server didn't respond
You probably didn't `await` something. `Bun.serve(...)` returns
immediately; the runtime keeps the event loop running as long as the
server is up, but if your script exits before that point you're fine —
the issue is if you also `await` something that never resolves.

### My ts-node / tsx config doesn't work
We don't read `tsconfig.json`. JSX falls back to classic `React.createElement`.
If you need the automatic JSX runtime, you'd need to write the import
manually for now.

### Linux build complains about missing `javascriptcoregtk-4.1`

```sh
sudo apt-get install libjavascriptcoregtk-4.1-dev pkg-config
```

See [`build.md`](build.md) for other distros.
