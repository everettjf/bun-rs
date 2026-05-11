# bun-rs

A Rust port of [Bun.js](https://github.com/oven-sh/bun), backed by JavaScriptCore via FFI.

**Status:** P0 done, most of P1 done (Day 1 of MVP). See [`docs/plan.md`](docs/plan.md).

## What works today

```sh
cargo build --release
./target/release/bun-rs --version            # bun-rs 0.0.1
./target/release/bun-rs -e "console.log(1+1)"
./target/release/bun-rs -p "40 + 2"          # 42
./target/release/bun-rs run examples/hello.ts
./target/release/bun-rs run app.ts           # multi-file TS w/ ESM imports
```

- TypeScript / TSX files transpile via [oxc](https://oxc.rs/) and run in JSC
- **ESM imports** (static): named, default, namespace, renamed, `export * from`,
  `export { x } from`, `export * as ns from`, circular deps,
  diamond-shared deps, `node_modules` via `oxc_resolver`
- `console.{log,info,warn,error,debug,trace,dir}`
- `process.{argv,env,cwd,exit,platform,arch,pid,versions}`
- `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval`
- `queueMicrotask` (Promise-based polyfill)
- Native async/await + Promise resolution

## What doesn't work yet

- Dynamic `import()` and top-level `await` (Phase 2, with tokio)
- `Bun.serve`, `fetch`, any Web API
- `node:fs`, `node:path`, …
- REPL
- Sourcemap-aware error stacks
- Linux & Windows (macOS only for now — uses the system JavaScriptCore.framework)

## Layout

```
crates/
  bun-cli/         entrypoint binary
  bun-runtime/     event loop + globals (console / process / timers / module loader)
  bun-jsc-sys/     raw JSC C API FFI
  bun-jsc/         safe RAII wrapper
  bun-transpile/   oxc-powered TS/JSX → JS
  bun-loader/      path resolver (oxc_resolver) + ESM → IIFE rewriter
```

## Build & test

```sh
cargo build --workspace             # debug build (~10s cold)
cargo build --release -p bun-cli    # 3.2MB single binary
cargo test --workspace              # 56 tests, all green on macOS arm64
```

Toolchain: pinned to a recent Rust nightly (oxc uses `if let` match guards).
See [`rust-toolchain.toml`](rust-toolchain.toml).

## License

MIT (matches Bun).
