# bun-rs roadmap

Versions are aspirational targets, not promises. Each milestone is a
**theme** plus a concrete exit checklist. Order within a milestone may
shuffle.

## 0.2 — Streams, concurrent HTTP, better debugging

ETA: ~4 weeks of focused work after 0.1.

**Theme:** make bun-rs honestly usable for a small production HTTP service.

| Item | Why it matters |
|---|---|
| `Bun.serve` true concurrency | Switch tiny_http → hyper on the existing tokio runtime; each request handler runs on its own tokio task so a slow handler doesn't stall others |
| `ReadableStream` / `WritableStream` / `TransformStream` | Required for streaming HTTP bodies, big-file pipelines, and the next set of node:* modules |
| `node:stream` (Readable / Writable / Duplex, pipe) | Unblocks lots of npm packages that wrap streams |
| Sourcemap-aware error stacks | Map JSC's stack frames back to original `.ts` lines via a sourcemap built during the rewriter pass |
| `node:fs` streaming (`createReadStream` / `createWriteStream`) | Use the new stream APIs |
| `node:http` minimal | A `request(url, cb)` and `createServer(cb)` that share the hyper-based plumbing |
| `AbortController` / `AbortSignal` | Cancelling fetch + timers |
| `Bun.spawn` | Streaming subprocess with EventEmitter-like API |
| Buffer pooling + slice perf | Reduce allocations for the common HTTP path |

**Exit:** can host a small static-site server with streaming responses,
respond to 10k QPS / 10kB payload, and crash with stack traces that
point at user `.ts` lines.

## 0.3 — Test runner + bundler MVP

ETA: ~6–8 weeks after 0.2.

**Theme:** development workflow.

| Item | Why |
|---|---|
| `bun-rs test` | A small Jest-compatible test runner (`describe` / `test` / `expect`, parallel files); reuses our existing module loader |
| `bun-rs build` | One-file bundler: take an entry and a `--outdir`, produce a tree-shaken JS bundle. Wraps `rolldown` (which already uses oxc) |
| `node:vm` (subset) | Library code that runs untrusted snippets |
| Live ESM bindings | Rewrite import sites to read from `__m.x` (not `const x = __m.x`); fix the circular-dep snapshot gotcha |
| `import.meta.glob` (Vite-style) | Cheap and very useful for serving static assets |
| WebSocket client + server | The WHATWG `WebSocket` global plus an upgrade path for `Bun.serve` |
| `node:tty` + `readline` | Real stdin in REPL and CLIs |

**Exit:** `bun-rs test my-package/**/*.test.ts` works for a package
that uses `node:fs`, `node:crypto`, and `fetch`. `bun-rs build src/app.ts`
produces a JS bundle that runs in Node.

## 0.4 — npm install

ETA: hard to predict; probably ~3 months of dedicated work.

**Theme:** users can run real npm projects.

| Item |
|---|
| `bun-rs install` — npm registry client + lockfile (compatible with `bun.lockb` or a fresh format) |
| Workspaces / monorepo support |
| Native-addon path: `node-gyp` interop OR a `bun-rs` plugin model |
| `dlx` / `bunx` equivalent |

**Exit:** clone a typical `vite` / `tsx` / `eslint` project and run their
scripts end-to-end.

## 0.5 — Native APIs + plugin system

| Item |
|---|
| `bun:ffi` minimal (`dlopen` + simple primitive types) |
| `bun:sqlite` (via `rusqlite`) |
| HTTPS server + HTTP/2 |
| `worker_threads` (one tokio runtime, JSC per worker, message channels) |
| `cluster` |
| Plugin loader (similar to Bun's `Bun.plugin`) |

## 1.0 — Stability + binary distribution

Targets:

- API frozen for the publicly documented surface in this guide.
- `cargo install bun-rs` works and grabs prebuilt binaries on macOS / Linux.
- Continuous fuzzing of the parser + module loader.
- Performance bake-off against Bun + Deno + Node published as a
  reproducible benchmark.
- Documented "100 most popular npm packages" compatibility matrix.

## Non-goals (for the foreseeable future)

- Windows. WebKit doesn't publish a Windows JSC build. If demand is real
  we'd vendor JSC, but that's a ~30-minute-build dep we'd be on the hook
  to maintain. Until then: WSL works.
- Visual Studio Code debugger integration. Useful eventually but not the
  scarcest thing.
- Source-level compatibility with every Bun internal API. We re-implement
  the *public* `Bun.*` namespace, not Bun's internal helpers.
- 100% Node API surface. Many `node:` modules ship with hundreds of
  rarely-used functions; we cover the high-traffic subset and rely on
  npm shims for the rest.

## How we decide what's next

Roughly:

1. **Correctness bugs** in shipped APIs always trump new features.
2. **Foundations** (streams, tokio integration, sourcemaps) trump
   horizontal API surface — they unlock multiple downstream features.
3. **What real users hit** trumps internal cleanups, once we have real users.

If you're looking at this and want X moved up, the path is: file an issue
with a concrete script that doesn't work today and the smallest fix that
would make it work.
