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
        r#"({
            describe: globalThis.describe,
            test: globalThis.test,
            it: globalThis.it,
            expect: globalThis.expect,
            beforeAll: globalThis.beforeAll,
            afterAll: globalThis.afterAll,
            beforeEach: globalThis.beforeEach,
            afterEach: globalThis.afterEach,
            mock: function (fn) { return fn || function () {}; },
            spyOn: function () { return { mockReturnValue() {}, mockResolvedValue() {} }; },
        })"#,
        Some("[bun:test]"),
    ).unwrap();
    let obj = exports_v.to_object().unwrap();
    obj.set_property("default", &exports_v).unwrap();
    exports_v
}
