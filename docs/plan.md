# bun-rs MVP 规划

> 目标:把 [Bun.js](https://github.com/oven-sh/bun) (Zig + C++) 重写为 Rust 版本,先交付一个能跑 TypeScript + 基础 web/node API + `Bun.serve` 的 MVP。

## 0. 三大根决策(2026-05-11 与项目所有者对齐)

| 决策项 | 选择 | 原因 |
|---|---|---|
| **JS 引擎** | JavaScriptCore via Rust FFI | 保留 Bun 的性能定位(JSC 启动快、内存低)。不用 V8(那会变成 Deno-in-Rust),不用 Boa(性能不达标)。 |
| **范围** | MVP only:`bun run script.ts` + `Bun.serve` + 基础 fs/http/process | 全量 1:1 翻译需要 50–200 人年;MVP + 复用生态把目标降到单人 3–6 个月。 |
| **依赖策略** | 最大化复用 Rust 生态 | transpile=oxc, async=tokio, HTTP=hyper/reqwest, allocator=mimalloc。只自己写 Bun 特有的 API 层和 JSC 绑定胶水。 |

**不在 MVP 范围**:bundler、package manager (`bun install`)、shell、sql、bake、css 处理、Worker/Cluster、`bun build`。

## 1. 架构

```
┌─────────────────────────────────────────────────────────┐
│  bun-cli (binary)                                       │
│    run / repl / -e / --version                          │
├─────────────────────────────────────────────────────────┤
│  bun-runtime  (event loop + module loader)              │
│    - JSC ↔ tokio LocalSet 协作                          │
│    - ESM/CJS 模块加载                                   │
│    - microtask drainer / setTimeout                     │
├──────────────┬──────────────┬──────────────┬────────────┤
│ bun-jsc-sys  │ bun-transpile│ bun-node     │ bun-web    │
│ (raw FFI)    │ (oxc wrap)   │ (fs/path/..) │ (fetch/..) │
│ bun-jsc      │              │              │            │
│ (safe wrap)  │              │              │            │
├──────────────┴──────────────┴──────────────┴────────────┤
│ 外部 crate: oxc / tokio / hyper / reqwest / mimalloc    │
│ 系统库:     JavaScriptCore.framework (macOS)           │
└─────────────────────────────────────────────────────────┘
```

**核心循环**:JSC 跑在主线程,tokio runtime 在同线程的 `LocalSet`。JS Promise 挂起时 → Rust 端 await tokio future → 完成后把结果回灌进 JSC 并 drain microtasks。对应 Bun 里的 `JSC::DeferredWorkTimer`/`drainMicrotasks` 模型。

## 2. workspace 布局

```
bun-rs/
├── Cargo.toml                # workspace root
├── rust-toolchain.toml       # 钉住 1.94 stable
├── docs/                     # 本规划 + 设计笔记
├── crates/
│   ├── bun-cli/              # 入口 binary
│   ├── bun-runtime/          # event loop + module loader
│   ├── bun-jsc-sys/          # JSC C API 原始 FFI(unsafe)
│   ├── bun-jsc/              # 安全 RAII 包装
│   ├── bun-transpile/        # oxc 包装(.ts/.tsx/.jsx)
│   ├── bun-node/             # node:fs / node:path / ...   [P3]
│   ├── bun-web/              # fetch/Headers/Request/...   [P2-3]
│   └── bun-api/              # Bun.* (Bun.serve, Bun.file) [P2]
├── tests/e2e/                # 跑真实脚本的 e2e
└── examples/                 # 演示用 .ts 脚本
```

## 3. 分阶段交付

### P0 · 脚手架(第 1–2 周)
- workspace 初始化、crate 骨架
- macOS 链上系统 `JavaScriptCore.framework`(Linux/Windows P4 再说)
- `bun-rs -e "1+1"` 输出 `2`
- `bun-rs -e "throw new Error('x')"` 打印错误并退出码非零
- **Exit**: 三个 e2e 测试在 macOS arm64 通过

### P1 · 最小 runtime(第 3–6 周)
- ESM/CJS 模块加载器(`import` 静态解析,先不支持 dynamic import)
- TypeScript / TSX:接 `oxc_transformer`,内存转译,挂 sourcemap
- `globalThis.console.{log,error,warn,info,debug,dir,trace}`
- `globalThis.process.{argv,env,cwd,exit,platform,pid,versions}`
- `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` / `queueMicrotask` 接 tokio
- **Exit**: `bun-rs run hello.ts` 跑通含 `import`、`async/await`、`console.log` 的 30 行 TS

### P2 · Bun.serve(第 7–10 周)
- `Bun.serve({ port, fetch })`:hyper server + Request/Response 绑成 JSC 对象
- `Headers` / `Request` / `Response` / `URL` / `URLSearchParams`(WHATWG)
- `fetch()` 走 reqwest(async)
- **Exit**: Bun 官方文档第一个 echo server 跑通,wrk 对比 Node + Bun 给基线

### P3 · 最小 node: 兼容(第 11–16 周)
- `node:fs`(sync + Promises,先 10 个最常用方法)
- `node:path` / `node:os` / `node:url` / `node:buffer`(Buffer = Uint8Array subclass)
- `node:child_process.spawn`
- **Exit**: 跑一个真实开源小工具(如 `tsx` 核心场景)

### P4 · 收尾(第 17–24 周)
- 错误信息 + 堆栈映射回 `.ts`(JSC stack + sourcemap)
- 分配器换 mimalloc
- `bun-rs test` 最小测试 runner(`describe`/`test`/`expect.toBe`)
- 单 binary release 构建,JSC 静态链接尝试
- **Exit**: 启动时间 / `Bun.serve` QPS / `fs.readFile` 吞吐对比报告

## 4. 关键风险

| 风险 | 对策 |
|---|---|
| Rust 没成熟 JSC 绑定 | P0 自写 `bun-jsc-sys`,先绑 ~40 个 API。**单点风险:P0 跑不通就要重估方案。** |
| JSC × tokio 协作 | 单线程 `LocalSet`,Promise resolve marshal 回 JS 线程。MVP 不做 Worker。 |
| ABI/平台差异 | macOS = 系统 framework(跟 Safari 版本);Linux = `webkit2gtk-4.1` 的 JSC 部分,版本钉死;Windows 不进 MVP。 |
| oxc 还在演进 | 钉版本号。CSS 不进 MVP,所以不依赖 lightningcss。 |
| Buffer/TypedArray 性能 | JSC `ArrayBuffer` backing store 可直接共享给 Rust,早期 prototype 验证。 |

## 5. 当前进度(2026-05-11 第 1 天)

P0 全部完成,P1 大部分完成。

**已交付**(`cargo test --workspace` → 30 tests pass):
- ✅ workspace + 5 crates(`bun-cli` / `bun-runtime` / `bun-jsc-sys` / `bun-jsc` / `bun-transpile`)
- ✅ `bun-jsc-sys`:42 个 JSC C API FFI 绑定,链 macOS `JavaScriptCore.framework`
- ✅ `bun-jsc`:RAII Context / Value / JsString / Object / JsException + 通过 JSClass 私有数据实现的闭包回调
- ✅ `bun-transpile`:oxc 0.129 → TS / JSX(classic React.createElement runtime)
- ✅ `bun-runtime`:
  - `console.{log,info,warn,error,debug,trace,dir}`(stderr 走 warn/error)
  - `process.{argv,env,cwd,exit,platform,arch,pid,versions.bun}`
  - `queueMicrotask` (Promise.resolve polyfill)
  - `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` + 最小事件循环
- ✅ `bun-cli`:`bun-rs -e <code>` / `bun-rs -p <expr>` / `bun-rs run <file>` / `bun-rs <file>` / `bun-rs --version`
- ✅ 19 个 e2e 测试覆盖以上路径,release 二进制 3.2MB,启动到运行 TS 约 10ms

**剩余 P1**:
- ESM `import` 模块加载器(当前是单文件)
- `bun-rs repl`
- sourcemap 错误回映射到 .ts

**待启动**:
- P2:`Bun.serve` + Web API(`fetch` / `Request` / `Response` / `URL`)
- P3:`node:fs` / `node:path` / `node:os` / `node:buffer` / `node:child_process`
- P4:mimalloc、JSC 静态链接、benchmark vs Bun / Node

## 6. 风险更新

- ✅ 单点风险(没有成熟 Rust JSC 绑定)已解除 — 自写 sys 层 + 安全包装在第一天跑通。
- 🟡 Rust 工具链锁定在 nightly(oxc 用 `if let` match guard,2025-12 nightly 起仍是 unstable)。等特性稳定再迁回 stable。
- 🟡 事件循环目前是 `std::thread::sleep`-based,只在 main script 结束后跑 timers。P2 接入 tokio LocalSet 后会重写。
