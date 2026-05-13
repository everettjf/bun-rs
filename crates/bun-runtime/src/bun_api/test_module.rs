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
                const fn = function (...args) {
                    calls.push(args);
                    if (returnValueSet) return returnValue;
                    if (impl) return impl.apply(this, args);
                    return undefined;
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
                fn.mockReturnValueOnce = fn.mockReturnValue;
                fn.mockResolvedValue = (v) => { returnValueSet = true; returnValue = Promise.resolve(v); return fn; };
                fn.mockResolvedValueOnce = fn.mockResolvedValue;
                fn.mockRejectedValue = (v) => { returnValueSet = true; returnValue = Promise.reject(v); return fn; };
                fn.mockRejectedValueOnce = fn.mockRejectedValue;
                fn.mockImplementation = (newImpl) => { impl = newImpl; return fn; };
                fn.mockImplementationOnce = fn.mockImplementation;
                fn.mockName = (n) => { fn._mockName = n; return fn; };
                fn.getMockName = () => fn._mockName || "jest.fn()";
                return fn;
            }
            function mock(impl) { return mkMock(impl); }
            mock.module = (_spec, _factory) => {};
            mock.restore = () => {};
            mock.clearAllMocks = () => {};
            mock.resetAllMocks = () => {};
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
                jest: { fn: mock, mock: mock.module, spyOn: function () { return mkMock(); }, useFakeTimers: () => {}, useRealTimers: () => {}, clearAllMocks: () => {}, resetAllMocks: () => {}, restoreAllMocks: () => {} },
                vi: { fn: mock, mock: mock.module, spyOn: function () { return mkMock(); }, useFakeTimers: () => {}, useRealTimers: () => {}, advanceTimersByTime: () => {}, runAllTimers: () => {}, clearAllMocks: () => {}, resetAllMocks: () => {}, restoreAllMocks: () => {} },
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
                setDefaultTimeout: () => {},
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
