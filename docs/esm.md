# ESM 加载器设计

> 目标:让 `bun-rs run app.ts` 能加载多文件 TypeScript 项目,支持
> `import`/`export`、`import x from 'pkg'`(node_modules)。
>
> **Phase 1 范围**(本次):静态 import/export,同步加载,不支持 dynamic import 也不支持 top-level await。
> **Phase 2 范围**(下次,跟 tokio 一起):dynamic `import()`,TLA,Worker。

## 1. 总体策略

JSC 公开 C API 没暴露模块系统(`JSEvaluateScript` 只跑 classic script,
不跑 module)。所以我们走经典编译派路线:**ESM → 同步 IIFE**。每个
模块被改写成

```js
(function (__exports, __bun_require, __filename, __dirname) {
  // module body, with imports/exports rewritten
})(/*exports*/, /*require*/, /*__filename*/, /*__dirname*/);
```

Rust 侧维护一个按绝对路径键的 `HashMap<PathBuf, JsValue>` 缓存,
`__bun_require(spec)` 是一个全局函数:

1. 用 Resolver 把 `spec`(相对路径或 bare 名)解析成绝对路径
2. 如果已缓存,直接返回 cached exports
3. 否则:读文件 → oxc 转译 → AST 重写 → 立刻同步求值 → 缓存 → 返回

循环引用:在第 3 步之前先把"未完成的 `{}` exports 对象"放进缓存,
然后求值。求值中如果再次 require 自己,返回的就是这个还在填充的对象
(跟 Node CJS 同语义)。

## 2. import / export 重写规则

| 源代码 | 重写后 |
|---|---|
| `import x from "./y"` | `const __m_0 = __bun_require("./y", __filename); const x = __m_0.default;` |
| `import { a, b as c } from "./y"` | `const __m_0 = __bun_require("./y", __filename); const a = __m_0.a, c = __m_0.b;` |
| `import * as ns from "./y"` | `const ns = __bun_require("./y", __filename);` |
| `import "./y"` | `__bun_require("./y", __filename);` |
| `export const x = 1` | `const x = 1; __exports.x = x;` |
| `export function f() {…}` | `function f() {…} __exports.f = f;` |
| `export class C {…}` | `class C {…} __exports.C = C;` |
| `export default <expr>` | `__exports.default = (<expr>);` |
| `export default function f() {…}` | `function f() {…} __exports.default = f;` |
| `export { a, b as c }` | `__exports.a = a; __exports.c = b;` |
| `export { a } from "./y"` | `const __m_0 = __bun_require("./y", __filename); __exports.a = __m_0.a;` |
| `export * from "./y"` | `const __m_0 = __bun_require("./y", __filename); Object.assign(__exports, __m_0);` |

**Hoisting**:JS `import` 是 hoisted,所有 require 必须出现在原 import
位置或之前。简化做法:把所有 require 调用挪到模块顶部。这跟 Babel
和 esbuild 的 CJS 转换一致。

**Live bindings**:ESM 规范要求"绑定"而不是"值拷贝"。短期内我们用
值拷贝(导入时取一次)。对绝大多数代码无可见差异;真正依赖 live binding
的代码很少。完整实现等 Phase 2。

## 3. Resolver

用 `oxc_resolver` 这个 crate(Rspack/Vite 的解析器,Rust 写的)。

接口:`fn resolve(spec: &str, importer: &Path) -> Result<PathBuf>`

算法(简化版,oxc_resolver 内部做完):
1. 若 `spec` 以 `./` `../` `/` 开头:相对/绝对路径
2. 否则视为 bare specifier:从 `importer.parent()` 沿目录树往上找 `node_modules/<spec>/`
3. 在候选目录里依次尝试:
   - `spec.ts` / `spec.tsx` / `spec.js` / `spec.jsx` / `spec.mjs` / `spec.cjs`
   - `spec/index.<ext>`
   - 若目录里有 `package.json`,读 `exports` / `module` / `main`
4. 找不到返回 `ENOTFOUND`

bun-rs 给的 oxc_resolver 配置:
- conditions:`["import", "default", "node"]` 上要加 `"bun"`
- extensions:`[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json"]`
- mainFields:`["module", "main"]`

## 4. cache + 循环引用

```rust
struct ModuleCache {
    // path → JS exports object (raw JSObjectRef, protected for lifetime)
    cache: RefCell<HashMap<PathBuf, JsValueRef>>,
}
```

`__bun_require(spec, importer)`:

```text
abs = resolver.resolve(spec, importer)
if let Some(exports) = cache.get(&abs):
    return exports                       // include partial during cycle
exports = JS {}                          // empty object
cache.insert(abs, exports)               // insert BEFORE running module body
source = read(abs); transpiled = oxc(source); rewritten = transform(transpiled)
eval(wrapper(rewritten))(exports, __bun_require, abs, abs.parent())
return exports
```

## 5. Phase 2 草图(下次会话)

- 切到 tokio + LocalSet 主事件循环,主线程跑 JSC
- 每个模块包装成 `async function` → 自动支持 TLA
- `import()` 返回 Promise,内部:`spawn_local(async { ... }).await`
- microtask drain 通过 JSC private API(或 polyfill)插入到 tokio 之间
- timer 实现迁移到 `tokio::time::sleep`
- 主 entry:`Runtime::run_async(path) → drives event loop until top-level promise resolves AND timer queue is empty AND no I/O pending`

## 6. 测试目标

Phase 1 e2e 覆盖:
- 命名 import + 命名 export
- default import + default export
- namespace import (`import * as`)
- re-export (`export { x } from`)
- `export *`
- 循环引用(A imports B, B imports A,各只用对方一部分,不死锁)
- 多层 + 共享依赖(diamond)
- 来自 `node_modules` 的简单 bare import(给个 fixture)
- 不存在的 specifier 抛错 + 错误信息可读
