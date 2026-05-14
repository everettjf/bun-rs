//! `bun:test` — re-exports the globals our test runner already injects
//! (describe / test / it / expect / beforeAll / afterAll / beforeEach /
//! afterEach). This lets Bun's official test suite import from `bun:test`
//! and still find the right symbols.

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    // When this module is required outside a `bun-rs test` invocation
    // (e.g. by `bun-rs run`), the globals aren't installed. In that case
    // we install a permissive stand-in so the script doesn't crash on
    // load — it just won't actually collect tests.
    let _ = ctx.eval(
        r#"
        if (typeof globalThis.describe !== "function") {
            globalThis.__bun_test_collector ??= [];
            globalThis.describe = (_n, body) => body();
            globalThis.test = globalThis.it = (n, fn) => {
                globalThis.__bun_test_collector.push({ name: n, fn, path: [], beforeEach: [], afterEach: [], skip: false });
            };
            globalThis.test.skip = globalThis.it.skip = () => {};
            globalThis.beforeAll = globalThis.afterAll = globalThis.beforeEach = globalThis.afterEach = () => {};
            globalThis.expect = (received) => {
                const fail = (m) => { throw new Error(m); };
                return new Proxy({}, { get(_, k) { return (...a) => { /* no-op outside test runner */ }; } });
            };
        }
        "#,
        Some("[bun:test-init]"),
    );

    let exports_v = ctx.eval(
        r#"(function(){
            function mkMock(impl) {
                const calls = [];
                const results = [];
                let nextResult = undefined;
                let returnValueSet = false;
                let returnValue = undefined;
                const onceQueue = [];          // fixed returns / impls
                const fn = function (...args) {
                    calls.push(args);
                    try {
                        let v;
                        if (onceQueue.length > 0) {
                            const o = onceQueue.shift();
                            if (o.kind === "return") v = o.value;
                            else v = o.impl.apply(this, args);
                        } else if (returnValueSet) v = returnValue;
                        else if (impl) v = impl.apply(this, args);
                        else v = undefined;
                        results.push({ type: "return", value: v });
                        return v;
                    } catch (e) {
                        results.push({ type: "throw", value: e });
                        throw e;
                    }
                };
                fn.mock = {
                    calls,
                    results,
                    instances: [],
                    contexts: [],
                    lastCall: () => calls[calls.length - 1],
                };
                fn.mockClear = () => { calls.length = 0; results.length = 0; return fn; };
                fn.mockReset = () => { fn.mockClear(); returnValueSet = false; returnValue = undefined; impl = null; return fn; };
                fn.mockRestore = () => fn.mockReset();
                fn.mockReturnValue = (v) => { returnValueSet = true; returnValue = v; return fn; };
                fn.mockReturnValueOnce = (v) => { onceQueue.push({ kind: "return", value: v }); return fn; };
                fn.mockResolvedValue = (v) => { returnValueSet = true; returnValue = Promise.resolve(v); return fn; };
                fn.mockResolvedValueOnce = (v) => { onceQueue.push({ kind: "return", value: Promise.resolve(v) }); return fn; };
                fn.mockRejectedValue = (v) => { returnValueSet = true; returnValue = Promise.reject(v); return fn; };
                fn.mockRejectedValueOnce = (v) => { onceQueue.push({ kind: "return", value: Promise.reject(v) }); return fn; };
                fn.mockImplementation = (newImpl) => { impl = newImpl; return fn; };
                fn.mockImplementationOnce = (newImpl) => { onceQueue.push({ kind: "impl", impl: newImpl }); return fn; };
                fn.mockName = (n) => { fn._mockName = n; return fn; };
                fn.getMockName = () => fn._mockName || "jest.fn()";
                // mock() return is a disposable: `using fn = mock(...)`
                // calls mockRestore on scope exit.
                Object.defineProperty(fn, Symbol.dispose, { value: () => fn.mockRestore(), configurable: true });
                Object.defineProperty(fn, Symbol.asyncDispose, { value: async () => fn.mockRestore(), configurable: true });
                return fn;
            }
            function mock(impl) { return mkMock(impl); }
            // mock.module(spec, factory) — register a factory the loader
            // checks before resolving normally. The next require/import of
            // `spec` will return the factory's result instead of the real
            // module. Async factories (returning a Promise) work too.
            // Resolves relative specs against the caller's source file.
            mock.module = (spec, factory) => {
                // Validate args BEFORE doing any resolution — Bun's contract.
                if (typeof spec !== "string") {
                    throw new TypeError("mock(module, fn) requires a module name string");
                }
                if (typeof factory !== "function") {
                    throw new TypeError("mock(module, fn) requires a function");
                }
                globalThis.__bun_mocked_modules = globalThis.__bun_mocked_modules || new Map();
                let s = String(spec);
                globalThis.__bun_mocked_modules.set(s, factory);
                // node builtin aliases: "fs/promises" ↔ "node:fs/promises".
                if (s.startsWith("node:")) globalThis.__bun_mocked_modules.set(s.slice(5), factory);
                else if (/^[a-z][a-z0-9_/-]*$/.test(s)) globalThis.__bun_mocked_modules.set("node:" + s, factory);
                // Also register under the absolute resolved path so imports
                // that go through file-system resolution find the mock.
                if (s.startsWith("./") || s.startsWith("../")) {
                    try {
                        const stack = new Error().stack || "";
                        const lines = stack.split("\n");
                        for (const ln of lines) {
                            const m = ln.match(/(\/[^()\s:]+\.(ts|tsx|js|jsx|mjs|cjs))/);
                            if (m && m[1] && !m[1].startsWith("/[")) {
                                const path = require("node:path");
                                const dir = path.dirname(m[1]);
                                const abs = path.resolve(dir, s);
                                // Try common extensions if no extension.
                                const tries = [abs, abs + ".ts", abs + ".js", abs + ".tsx", abs + ".jsx", abs + ".mjs"];
                                for (const t of tries) {
                                    globalThis.__bun_mocked_modules.set(t, factory);
                                }
                                break;
                            }
                        }
                    } catch {}
                }
                // Also invalidate cached exports for this spec so the next
                // import re-evaluates through the mock factory.
                if (globalThis.__bun_invalidate_module) {
                    try { globalThis.__bun_invalidate_module(s); } catch {}
                }
            };
            mock.restore = () => {};
            mock.clearAllMocks = () => {};
            mock.resetAllMocks = () => {};
            // Hoist the fake-timer factory so vi can reuse the same impl.
            const __jestObj = (function () {
                const ft = globalThis.__bun_fake_timers = globalThis.__bun_fake_timers || {
                    active: false, mockedNow: 0,
                    origNow: Date.now.bind(Date),
                    origSetTimeout: globalThis.setTimeout,
                    origClearTimeout: globalThis.clearTimeout,
                    origSetInterval: globalThis.setInterval,
                    origClearInterval: globalThis.clearInterval,
                    nextId: 1, timers: [],
                };
                function fakeSetTimeout(cb, ms) {
                    const delay = Math.max(0, Math.ceil(ms || 0));
                    const id = ft.nextId++;
                    ft.timers.push({ id, fireAt: ft.mockedNow + delay, cb, interval: 0 });
                    return id;
                }
                function fakeSetInterval(cb, ms) {
                    const delay = Math.max(1, Math.ceil(ms || 0));
                    const id = ft.nextId++;
                    ft.timers.push({ id, fireAt: ft.mockedNow + delay, cb, interval: delay });
                    return id;
                }
                function fakeClear(id) { ft.timers = ft.timers.filter(t => t.id !== id); }
                function fakeAdvance(ms) {
                    if (!ft.active) return;
                    const target = ft.mockedNow + ms;
                    while (true) {
                        const next = ft.timers.filter(t => t.fireAt <= target).sort((a, b) => a.fireAt - b.fireAt)[0];
                        if (!next) break;
                        ft.mockedNow = next.fireAt;
                        if (next.interval > 0) next.fireAt += next.interval;
                        else ft.timers = ft.timers.filter(t => t.id !== next.id);
                        try { next.cb(); } catch {}
                    }
                    ft.mockedNow = target;
                }
                function install(opts) {
                    ft.active = true;
                    const nowOpt = opts && opts.now;
                    if (typeof nowOpt === "number") ft.mockedNow = nowOpt;
                    else if (nowOpt instanceof Date) ft.mockedNow = nowOpt.getTime();
                    else ft.mockedNow = ft.origNow();
                    Date.now = () => ft.mockedNow;
                    globalThis.setTimeout = fakeSetTimeout;
                    globalThis.clearTimeout = fakeClear;
                    globalThis.setInterval = fakeSetInterval;
                    globalThis.clearInterval = fakeClear;
                }
                function uninstall() {
                    ft.active = false;
                    ft.timers = [];
                    Date.now = ft.origNow;
                    globalThis.setTimeout = ft.origSetTimeout;
                    globalThis.clearTimeout = ft.origClearTimeout;
                    globalThis.setInterval = ft.origSetInterval;
                    globalThis.clearInterval = ft.origClearInterval;
                }
                return {
                    useFakeTimers: install, useRealTimers: uninstall,
                    setSystemTime: (t) => {
                        if (ft.active) {
                            if (typeof t === "number") ft.mockedNow = t;
                            else if (t instanceof Date) ft.mockedNow = t.getTime();
                        }
                    },
                    advanceTimersByTime: fakeAdvance,
                    runOnlyPendingTimers: () => { if (ft.active) fakeAdvance(0); },
                    runAllTimers: () => {
                        if (!ft.active) return;
                        let safety = 10000;
                        while (ft.timers.length > 0 && safety-- > 0) {
                            const next = ft.timers.sort((a,b)=>a.fireAt-b.fireAt)[0];
                            fakeAdvance(Math.max(0, next.fireAt - ft.mockedNow));
                        }
                    },
                };
            })();
            return {
                __esModule: true,
                describe: globalThis.describe,
                test: globalThis.test,
                it: globalThis.it,
                expect: globalThis.expect,
                beforeAll: globalThis.beforeAll,
                afterAll: globalThis.afterAll,
                beforeEach: globalThis.beforeEach,
                afterEach: globalThis.afterEach,
                mock,
                jest: Object.assign({
                    fn: mock, mock: mock.module, spyOn: function () { return mkMock(); },
                    clearAllMocks: () => {}, resetAllMocks: () => {}, restoreAllMocks: () => {},
                    setTimeout: (ms) => { globalThis.__bun_test_default_timeout = ms; },
                    requireActual: (m) => globalThis.require(m),
                    requireMock: (m) => globalThis.require(m),
                    retryTimes: (_n) => {},
                }, __jestObj),
                vi: Object.assign({
                    fn: mock, mock: mock.module, spyOn: function () { return mkMock(); },
                    clearAllMocks: () => {}, resetAllMocks: () => {}, restoreAllMocks: () => {},
                }, __jestObj),
                spyOn: function (obj, key) {
                    const orig = obj[key];
                    const m = mkMock(typeof orig === "function" ? orig.bind(obj) : undefined);
                    obj[key] = m;
                    m.mockRestore = () => { obj[key] = orig; };
                    // Disposable: `using spy = spyOn(obj, "m")` restores on scope exit.
                    Object.defineProperty(m, Symbol.dispose, { value: () => m.mockRestore(), configurable: true });
                    Object.defineProperty(m, Symbol.asyncDispose, { value: async () => m.mockRestore(), configurable: true });
                    return m;
                },
                setSystemTime: () => {},
                setDefaultTimeout: (ms) => { globalThis.__bun_test_default_timeout = ms; },
                expectTypeOf: globalThis.expectTypeOf || (() => new Proxy(function(){}, { get: () => () => undefined, apply: () => undefined })),
                // onTestFinished / onTestFailed — register a cleanup hook
                // that runs at end of current test (or describe).
                onTestFinished: (fn) => {
                    if (globalThis.__bun_in_concurrent_test) {
                        throw new Error("Cannot call onTestFinished() here. It cannot be called inside a concurrent test. Use test.serial or remove test.concurrent.");
                    }
                    globalThis.__bun_current_finally = (globalThis.__bun_current_finally || []);
                    globalThis.__bun_current_finally.push(fn);
                },
                onTestFailed: (_fn) => {},
                test_listing: [],
                isInDescribe: () => false,
                getTestName: () => "",
            };
        })()"#,
        Some("[bun:test]"),
    ).unwrap();
    let obj = exports_v.to_object().unwrap();
    obj.set_property("default", &exports_v).unwrap();

    // Also expose `mock`, `vi`, `jest`, `spyOn` as globals — Bun's tests
    // often use them without an explicit import.
    let _ = ctx.eval(
        r#"
        (function(g, m){
            g.mock = m.mock;
            g.vi = m.vi;
            g.jest = m.jest;
            g.spyOn = m.spyOn;
        })(globalThis, globalThis.__bun_test_module_exports = (function(){ return null; })() || arguments && arguments[0] || null);
        "#,
        Some("[bun:test-globals]"),
    );
    // Actually a simpler way to install globals — just stamp them now.
    if let Ok(obj) = exports_v.to_object() {
        let g = ctx.global_object();
        for k in ["mock", "vi", "jest", "spyOn", "describe", "test", "it", "expect", "beforeAll", "afterAll", "beforeEach", "afterEach"].iter() {
            if let Ok(v) = obj.get_property(k) {
                if !v.is_undefined() {
                    let _ = g.set_property(k, &v);
                }
            }
        }
    }
    exports_v
}
