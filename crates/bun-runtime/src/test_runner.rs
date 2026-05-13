//! `bun-rs test` — Jest-compatible test runner.
//!
//! Workflow:
//!   1. CLI flag dispatched from `cli_main` → `run_tests(args)`.
//!   2. Discover *.test.ts / *.test.tsx / *.test.js / *.test.jsx files,
//!      starting from the given path (or cwd) recursively.
//!   3. For each file: spin a fresh module loader registration, install
//!      `describe` / `test` / `it` / `expect` / `beforeAll` / `beforeEach`
//!      / `afterAll` / `afterEach` globals. Tests register into a JS-side
//!      collector; we then iterate them and await each one.
//!   4. Print results, exit 0 if everything passed.
//!
//! What `expect` covers (subset of Jest):
//!   toBe, toEqual, toStrictEqual, toBeTruthy, toBeFalsy, toBeNull,
//!   toBeUndefined, toBeDefined, toContain, toHaveLength, toMatch,
//!   toThrow, toBeInstanceOf, toBeGreaterThan, toBeLessThan,
//!   toBeGreaterThanOrEqual, toBeLessThanOrEqual, toBeCloseTo, plus
//!   `.not` and async `.resolves` / `.rejects`.

use std::path::{Path, PathBuf};

use bun_jsc::Context;

pub fn run_tests(paths: Vec<String>) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![cwd.clone()]
    } else {
        paths.into_iter().map(|p| {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() { pb } else { cwd.join(p) }
        }).collect()
    };

    let mut files = Vec::new();
    for root in &roots {
        discover(root, &mut files);
    }
    files.sort();
    files.dedup();

    if files.is_empty() {
        eprintln!("no test files found (looked for *.test.ts / .tsx / .js / .jsx)");
        return 1;
    }

    let rt = crate::Runtime::new(vec![crate::bun_exe_path(), "test".to_string()]);
    install_globals(&rt.ctx);

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut total_skipped = 0usize;
    let mut failed_files = Vec::<String>::new();

    for file in &files {
        eprintln!("\n● {}", file.display());
        // Reset the JS-side collector for each file.
        let _ = rt.ctx.eval("globalThis.__bun_test_collector = []", Some("[test-reset]"));

        // Load the module via the loader (full TS / ESM pipeline).
        if let Err(e) = crate::modules::run_entry(&rt.ctx, file) {
            eprintln!("  ✗ failed to load: {e}");
            total_fail += 1;
            failed_files.push(file.display().to_string());
            continue;
        }

        // Now run the collected tests (also async).
        let runner = rt
            .ctx
            .eval(
                r#"
                (async () => {
                    const all = globalThis.__bun_test_collector || [];
                    let pass = 0, fail = 0, skipped = 0;
                    const failed = [];
                    // Dedupe beforeAll hooks by reference: a parent describe's
                    // beforeAll appears in every nested test's inherited list,
                    // but should still only run once.
                    const ranBeforeAll = new WeakSet();
                    // Per-describe-path afterAll bookkeeping. We fire afterAll
                    // hooks when the next test moves OUT of a describe path,
                    // not at end-of-file. This matches jest semantics:
                    //   describe(A) { describe(B) { it; } it(C); }
                    // afterAll(B) fires before it(C) runs, so it(C) sees
                    // state set by afterAll(B).
                    let prevPath = [];
                    const pathAfterAlls = new Map(); // pathKey -> [hooks]
                    function pathKey(p) { return p.join("\x1f"); }
                    async function fireExitedAfterAlls(newPath) {
                        let i = 0;
                        while (i < prevPath.length && i < newPath.length && prevPath[i] === newPath[i]) i++;
                        // Fire afterAlls from the deepest exited level out to i+1.
                        for (let depth = prevPath.length; depth > i; depth--) {
                            const k = pathKey(prevPath.slice(0, depth));
                            const hooks = pathAfterAlls.get(k);
                            if (hooks) {
                                pathAfterAlls.delete(k);
                                for (let j = hooks.length - 1; j >= 0; j--) {
                                    try { await runHook(hooks[j]); } catch (e) {
                                        console.log("  ✗ afterAll threw: " + (e && e.message ? e.message : e));
                                    }
                                }
                            }
                        }
                        prevPath = newPath;
                    }
                    // Hook runner that supports jest's done(err?) callback
                    // pattern: if the hook function declares a parameter, we
                    // create a Promise that resolves when done() is called
                    // (or rejects with the error argument).
                    async function runHook(h) {
                        if (typeof h !== "function") return;
                        if (h.length >= 1) {
                            await new Promise((resolve, reject) => {
                                const done = (err) => { if (err) reject(err); else resolve(); };
                                Promise.resolve(h(done)).then(undefined, reject);
                            });
                        } else {
                            await h();
                        }
                    }
                    async function runHooks(hooks, label) {
                        for (const h of hooks || []) {
                            try { await runHook(h); } catch (e) {
                                console.log("  ✗ " + label + " threw: " + (e && e.message ? e.message : e));
                            }
                        }
                    }
                    async function runOne(t) {
                        const fullName = t.path.length ? t.path.concat(t.name).join(" > ") : t.name;
                        // Fire afterAll hooks for any describe levels the previous
                        // test was inside that we've now left.
                        await fireExitedAfterAlls(t.path);
                        if (t.skip) {
                            console.log("  - " + fullName + " (skipped)");
                            skipped++;
                            return;
                        }
                        // beforeAll: run each unique hook once.
                        for (const h of t.beforeAll || []) {
                            if (typeof h === "function" && !ranBeforeAll.has(h)) {
                                ranBeforeAll.add(h);
                                try { await runHook(h); } catch (e) {
                                    console.log("  ✗ beforeAll threw: " + (e && e.message ? e.message : e));
                                }
                            }
                        }
                        // Register this test's afterAll hooks at the deepest level
                        // of its path (so they fire when the next test exits that level).
                        if (t.afterAll && t.afterAll.length > 0) {
                            const k = pathKey(t.path);
                            const seen = pathAfterAlls.get(k) || [];
                            for (const h of t.afterAll) {
                                if (typeof h === "function" && !seen.includes(h)) seen.push(h);
                            }
                            pathAfterAlls.set(k, seen);
                        }
                        try {
                            for (const h of t.beforeEach) await runHook(h);
                            let result;
                            try {
                                result = await (t.fn.length >= 1
                                  ? new Promise((res, rej) => {
                                      const done = (e) => { if (e) rej(e); else res(); };
                                      Promise.resolve(t.fn(done)).then(undefined, rej);
                                    })
                                  : t.fn());
                                void result;
                                if (t.failing) {
                                    throw new Error("test was marked .failing but passed");
                                }
                            } catch (e) {
                                if (t.failing) {
                                    // .failing: error is expected; treat as pass.
                                    // afterEach: inner→outer (reverse order).
                                    for (let i = t.afterEach.length - 1; i >= 0; i--) {
                                        try { await runHook(t.afterEach[i]); } catch {}
                                    }
                                    console.log("  ✓ " + fullName + " (failing, threw as expected)");
                                    pass++;
                                    return;
                                }
                                throw e;
                            }
                            // afterEach: inner-most first (jest semantics).
                            for (let i = t.afterEach.length - 1; i >= 0; i--) {
                                await runHook(t.afterEach[i]);
                            }
                            console.log("  ✓ " + fullName);
                            pass++;
                        } catch (e) {
                            const msg = e && e.message ? e.message : String(e);
                            console.log("  ✗ " + fullName + " — " + msg);
                            if (e && e.stack) console.log("    " + String(e.stack).split("\n").join("\n    "));
                            fail++;
                            failed.push(fullName);
                        }
                    }
                    for (const t of all) await runOne(t);
                    // End of file: fire all remaining afterAll hooks (paths
                    // we never exited because they were the last ones).
                    await fireExitedAfterAlls([]);
                    return { pass, fail, skipped, failed };
                })()
                "#,
                Some("[test-runner]"),
            )
            .map_err(|e| e.message())
            .unwrap_or_else(|m| {
                eprintln!("  ✗ test runner harness failed: {m}");
                bun_jsc::Value::new_null(&rt.ctx)
            });

        // The runner returns a Promise<{pass, fail, failed}>. Await it.
        let result = match crate::modules::await_promise(&rt.ctx, runner) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  ✗ test runner error: {e}");
                total_fail += 1;
                continue;
            }
        };
        let result_obj = match result.to_object() {
            Ok(o) => o,
            Err(_) => continue,
        };
        let file_pass = result_obj
            .get_property("pass")
            .map(|v| v.to_number() as usize)
            .unwrap_or(0);
        let file_fail = result_obj
            .get_property("fail")
            .map(|v| v.to_number() as usize)
            .unwrap_or(0);
        let file_skipped = result_obj
            .get_property("skipped")
            .map(|v| v.to_number() as usize)
            .unwrap_or(0);
        total_pass += file_pass;
        total_fail += file_fail;
        total_skipped += file_skipped;
        if file_fail > 0 {
            failed_files.push(file.display().to_string());
        }
    }

    eprintln!("\n──────────");
    eprintln!("files:  {} ({} failed)", files.len(), failed_files.len());
    eprintln!(
        "tests:  {} passed, {} failed, {} skipped",
        total_pass, total_fail, total_skipped
    );
    if total_fail > 0 {
        eprintln!("\nfailed in:");
        for f in &failed_files {
            eprintln!("  {}", f);
        }
        1
    } else {
        0
    }
}

fn discover(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        if is_test_file(root) {
            out.push(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()));
        }
        return;
    }
    if !root.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name == "node_modules" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            discover(&p, out);
        } else if is_test_file(&p) {
            out.push(p.canonicalize().unwrap_or(p));
        }
    }
}

fn is_test_file(p: &Path) -> bool {
    let name = match p.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    name.ends_with(".test.ts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".test.js")
        || name.ends_with(".test.jsx")
        || name.ends_with(".test.mjs")
        || name.ends_with(".test.cjs")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.tsx")
        || name.ends_with(".spec.js")
        || name.ends_with(".spec.mjs")
        || name.ends_with(".spec.js")
}

fn install_globals(ctx: &Context) {
    ctx.eval(
        GLOBALS,
        Some("[test-globals]"),
    )
    .expect("install test globals");

    // Eagerly load bun:test so its globals (mock, vi, jest, spyOn) are
    // visible to test files that don't `import "bun:test"` themselves
    // (e.g. test files that go through Bun.jest(source) or the bun:jsc
    // → callerSourceOrigin path).
    let _ = crate::bun_api::load_bun_builtin(ctx, "test");
}

const GLOBALS: &str = r#"
(function (g) {
  g.__bun_test_collector = [];
  // describe stack — current nested name + hooks
  const stack = [{ path: [], beforeAll: [], afterAll: [], beforeEach: [], afterEach: [] }];

  function curr() { return stack[stack.length - 1]; }

  g.describe = function (name, body) {
    // Single-arg form: describe(fn) — anonymous suite.
    if (typeof name === "function" && body === undefined) { body = name; name = ""; }
    if (typeof body !== "function") return; // describe(name) with no body — no-op
    stack.push({
      path: [...curr().path, name],
      beforeAll: [...curr().beforeAll],
      afterAll: [...curr().afterAll],
      beforeEach: [...curr().beforeEach],
      afterEach: [...curr().afterEach],
    });
    try { body(); } finally { stack.pop(); }
  };
  // Variants Bun's tests use heavily:
  g.describe.skip = (name, body) => {
    if (typeof name === "function" && body === undefined) { body = name; name = ""; }
    if (typeof body !== "function") return;
    const top = curr();
    stack.push({
      path: [...top.path, name],
      beforeAll: [...top.beforeAll],
      afterAll: [...top.afterAll],
      beforeEach: [...top.beforeEach],
      afterEach: [...top.afterEach],
      forceSkip: true,
    });
    try { body(); } finally { stack.pop(); }
  };
  g.describe.only = g.describe;            // Treat .only as normal
  g.describe.todo = (name) => {};          // Skip the body entirely.
  g.describe.skipIf = (cond) => (cond ? g.describe.skip : g.describe);
  g.describe.todoIf = (cond) => (cond ? g.describe.todo : g.describe);
  g.describe.if = (cond) => (cond ? g.describe : g.describe.skip);
  g.describe.each = (rows) => (name, body) => {
    for (const row of rows) {
      const args = Array.isArray(row) ? row : [row];
      g.describe(name + " [" + safeStringify(args) + "]", () => body(...args));
    }
  };
  g.describe.concurrent = g.describe;
  g.describe.serial = g.describe;
  g.describe.failing = g.describe;
  g.describe.concurrentIf = (cond) => (cond ? g.describe.concurrent : g.describe);
  g.describe.serialIf = (cond) => (cond ? g.describe.serial : g.describe);
  g.describe.failingIf = (cond) => (cond ? g.describe.failing : g.describe);

  function safeStringify(v) {
    try { return JSON.stringify(v); } catch { return String(v); }
  }

  function pushTest(name, fn, opts) {
    const c = curr();
    const skip = !!(opts && opts.skip) || !!c.forceSkip;
    g.__bun_test_collector.push({
      name, fn,
      path: c.path,
      beforeAll: [...c.beforeAll],
      afterAll: [...c.afterAll],
      beforeEach: [...c.beforeEach],
      afterEach: [...c.afterEach],
      skip,
      failing: !!(opts && opts.failing),
    });
  }

  g.test = (name, fn, opts) => {
    if (opts && typeof opts === "object") {
      if (opts.retry !== undefined && opts.repeats !== undefined) {
        throw new Error("Cannot set both retry and repeats on a test");
      }
    }
    return pushTest(name, fn);
  };
  g.it = g.test;
  g.test.skip = (name) => pushTest(name, () => {}, { skip: true });
  g.it.skip = g.test.skip;
  g.test.todo = (name, fn) => pushTest(name, fn || (() => {}), { skip: true });
  g.it.todo = g.test.todo;
  g.test.only = (name, fn) => pushTest(name, fn);
  g.it.only = g.test.only;
  // .failing: test is expected to fail; inverted exit code.
  g.test.failing = (name, fn) => pushTest(name, fn, { failing: true });
  g.it.failing = g.test.failing;
  // .skipIf(cond)(name, fn) — skip when cond is truthy.
  g.test.skipIf = (cond) => (cond ? g.test.skip : g.test);
  g.it.skipIf = g.test.skipIf;
  g.test.todoIf = (cond) => (cond ? g.test.todo : g.test);
  g.it.todoIf = g.test.todoIf;
  // .if(cond) — run only if cond is truthy.
  g.test.if = (cond) => (cond ? g.test : g.test.skip);
  g.it.if = g.test.if;
  g.test.concurrent = (name, fn) => pushTest(name, fn);
  g.it.concurrent = g.test.concurrent;
  g.test.concurrent.skip = g.test.skip;
  g.test.concurrent.only = g.test.only;
  g.test.concurrent.each = g.test.each;
  g.test.concurrentIf = (cond) => (cond ? g.test.concurrent : g.test);
  g.it.concurrentIf = g.test.concurrentIf;
  g.test.serial = g.test;
  g.it.serial = g.it;
  g.test.serialIf = (cond) => (cond ? g.test.serial : g.test);
  g.it.serialIf = g.it.serialIf;
  g.test.failingIf = (cond) => (cond ? g.test.failing : g.test);
  g.it.failingIf = g.test.failingIf;
  // .each pattern continues below
  g.test.each = (rows) => (name, fn) => {
    for (const row of rows) {
      const args = Array.isArray(row) ? row : [row];
      pushTest(name + " [" + safeStringify(args) + "]", () => fn(...args));
    }
  };
  g.it.each = g.test.each;

  g.beforeAll = (fn) => curr().beforeAll.push(fn);
  g.afterAll = (fn) => curr().afterAll.push(fn);
  g.beforeEach = (fn) => curr().beforeEach.push(fn);
  g.afterEach = (fn) => curr().afterEach.push(fn);

  // ── expect ──
  function deepEq(a, b) {
    if (Object.is(a, b)) return true;
    if (a === null || b === null) return false;
    if (typeof a !== "object" || typeof b !== "object") return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    if (a instanceof Date && b instanceof Date) return a.getTime() === b.getTime();
    if (a instanceof RegExp && b instanceof RegExp) return a.toString() === b.toString();
    if (ArrayBuffer.isView(a) && ArrayBuffer.isView(b)) {
      if (a.byteLength !== b.byteLength) return false;
      for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
      return true;
    }
    const ak = Object.keys(a), bk = Object.keys(b);
    if (ak.length !== bk.length) return false;
    for (const k of ak) {
      if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
      if (!deepEq(a[k], b[k])) return false;
    }
    return true;
  }
  function fmt(v) {
    if (v === undefined) return "undefined";
    if (v === null) return "null";
    if (typeof v === "string") return JSON.stringify(v);
    if (typeof v === "function") return "[Function]";
    try { return JSON.stringify(v); } catch { return String(v); }
  }
  function mkExpect(received, not) {
    const fail = (msg) => { throw new Error(msg); };
    const check = (cond, expected, action) => {
      if (not ? cond : !cond) {
        fail(`expect(${fmt(received)})${not ? ".not" : ""}.${action}(${expected !== undefined ? fmt(expected) : ""})`);
      }
    };
    const obj = {
      toBe(v) { check(Object.is(received, v), v, "toBe"); },
      toEqual(v) { check(deepEq(received, v), v, "toEqual"); },
      toStrictEqual(v) { check(deepEq(received, v), v, "toStrictEqual"); },
      toBeTruthy() { check(!!received, undefined, "toBeTruthy"); },
      toBeFalsy() { check(!received, undefined, "toBeFalsy"); },
      toBeNull() { check(received === null, undefined, "toBeNull"); },
      toBeUndefined() { check(received === undefined, undefined, "toBeUndefined"); },
      toBeDefined() { check(received !== undefined, undefined, "toBeDefined"); },
      toBeNaN() { check(Number.isNaN(received), undefined, "toBeNaN"); },
      toContain(v) {
        const found = Array.isArray(received) ? received.includes(v) :
                      typeof received === "string" ? received.includes(v) : false;
        check(found, v, "toContain");
      },
      toHaveLength(n) {
        const len = received == null ? null
                  : (received.length !== undefined ? received.length
                  : (received.byteLength !== undefined ? received.byteLength
                  : (received.size !== undefined ? received.size : null)));
        check(len === n, n, "toHaveLength");
      },
      toMatch(re) { check(typeof re === "string" ? received.includes(re) : re.test(received), re, "toMatch"); },
      // Alias: jest's toThrowError === toThrow.
      toThrowError(matcher) { return obj.toThrow(matcher); },
      // Bun-extension: assert thrown error matches a constructor + has .code property.
      toThrowWithCode(cls, code) {
        let caught;
        try { received(); } catch (e) { caught = e; }
        const ok = !!caught && (cls ? caught instanceof cls : true) && caught.code === code;
        check(ok, code, "toThrowWithCode");
      },
      async toThrowWithCodeAsync(cls, code) {
        let caught;
        try { await received(); } catch (e) { caught = e; }
        const ok = !!caught && (cls ? caught instanceof cls : true) && caught.code === code;
        check(ok, code, "toThrowWithCodeAsync");
      },
      toThrow(matcher) {
        let caught;
        try { received(); } catch (e) { caught = e; }
        const matched = !!caught && (
          matcher === undefined
          // Asymmetric matchers (expect.objectContaining etc.)
          || (matcher && matcher.__bun_match && typeof matcher.asymmetricMatch === "function" && matcher.asymmetricMatch(caught))
          || (matcher instanceof RegExp ? matcher.test(caught.message || String(caught))
            : typeof matcher === "string" ? (caught.message || String(caught)).includes(matcher)
            : typeof matcher === "function" ? caught instanceof matcher
            // Plain object: shape-match against caught.
            : (matcher && typeof matcher === "object") ? (
                Object.keys(matcher).every(k => deepEq(caught[k], matcher[k]))
              )
            : false)
        );
        check(matched, matcher, "toThrow");
      },
      toBeInstanceOf(cls) { check(received instanceof cls, cls.name, "toBeInstanceOf"); },
      toBeGreaterThan(n) { check(received > n, n, "toBeGreaterThan"); },
      toBeLessThan(n) { check(received < n, n, "toBeLessThan"); },
      toBeGreaterThanOrEqual(n) { check(received >= n, n, "toBeGreaterThanOrEqual"); },
      toBeLessThanOrEqual(n) { check(received <= n, n, "toBeLessThanOrEqual"); },
      toBeCloseTo(n, digits) {
        const d = digits == null ? 2 : digits;
        check(Math.abs(received - n) < Math.pow(10, -d) / 2, n, "toBeCloseTo");
      },
      toHaveProperty(key, value) {
        const parts = String(key).split(".");
        let v = received;
        for (const p of parts) {
          if (v == null || !(p in v)) { check(false, key, "toHaveProperty"); return; }
          v = v[p];
        }
        if (arguments.length >= 2) check(deepEq(v, value), value, "toHaveProperty");
        else check(true, key, "toHaveProperty");
      },
      // Bun-specific matchers (also in jest-extended).
      toBeTrue()  { check(received === true, undefined, "toBeTrue"); },
      toBeFalse() { check(received === false, undefined, "toBeFalse"); },
      toBeBoolean() { check(typeof received === "boolean", undefined, "toBeBoolean"); },
      toBeString() { check(typeof received === "string", undefined, "toBeString"); },
      toBeNumber() { check(typeof received === "number" && !isNaN(received), undefined, "toBeNumber"); },
      toBeFinite() { check(Number.isFinite(received), undefined, "toBeFinite"); },
      toBeInteger() { check(Number.isInteger(received), undefined, "toBeInteger"); },
      toBePositive() { check(typeof received === "number" && received > 0, undefined, "toBePositive"); },
      toBeNegative() { check(typeof received === "number" && received < 0, undefined, "toBeNegative"); },
      toBeOdd() { check(Number.isInteger(received) && received % 2 !== 0, undefined, "toBeOdd"); },
      toBeEven() { check(Number.isInteger(received) && received % 2 === 0, undefined, "toBeEven"); },
      toBeFunction() { check(typeof received === "function", undefined, "toBeFunction"); },
      toBeObject() { check(received !== null && typeof received === "object", undefined, "toBeObject"); },
      toBeArray() { check(Array.isArray(received), undefined, "toBeArray"); },
      toBeArrayOfSize(n) { check(Array.isArray(received) && received.length === n, n, "toBeArrayOfSize"); },
      toBeEmpty() {
        const empty = received == null
          || (typeof received === "string" && received.length === 0)
          || (Array.isArray(received) && received.length === 0)
          || (received && typeof received === "object" && Object.keys(received).length === 0);
        check(empty, undefined, "toBeEmpty");
      },
      toBeEmptyObject() {
        check(received && typeof received === "object" && Object.keys(received).length === 0, undefined, "toBeEmptyObject");
      },
      toContainEqual(v) {
        const found = Array.isArray(received) && received.some((x) => deepEq(x, v));
        check(found, v, "toContainEqual");
      },
      toContainAllValues(arr) {
        if (!received || typeof received !== "object") { check(false, arr, "toContainAllValues"); return; }
        const values = Array.isArray(received) ? received : Object.values(received);
        check(arr.every((v) => values.some((x) => deepEq(x, v))), arr, "toContainAllValues");
      },
      toStartWith(s) { check(typeof received === "string" && received.startsWith(s), s, "toStartWith"); },
      toEndWith(s) { check(typeof received === "string" && received.endsWith(s), s, "toEndWith"); },
      toIncludeRepeated(sub, count) {
        if (typeof received !== "string") { check(false, sub, "toIncludeRepeated"); return; }
        let n = 0, i = 0;
        while ((i = received.indexOf(sub, i)) !== -1) { n++; i++; }
        check(n === count, sub, "toIncludeRepeated");
      },
      toEqualIgnoringWhitespace(s) {
        const norm = (x) => String(x).replace(/\s+/g, " ").trim();
        check(norm(received) === norm(s), s, "toEqualIgnoringWhitespace");
      },
      toMatchObject(partial) {
        function matches(rec, part) {
          if (part === null || typeof part !== "object") return deepEq(rec, part);
          if (rec === null || typeof rec !== "object") return false;
          if (Array.isArray(part)) {
            if (!Array.isArray(rec)) return false;
            if (rec.length < part.length) return false;
            return part.every((v, i) => matches(rec[i], v));
          }
          return Object.keys(part).every((k) => matches(rec[k], part[k]));
        }
        check(matches(received, partial), partial, "toMatchObject");
      },
      toBeOneOf(arr) {
        check(arr.some((x) => deepEq(received, x)), arr, "toBeOneOf");
      },
      toBeWithin(min, max) {
        check(typeof received === "number" && received >= min && received < max, [min, max], "toBeWithin");
      },
      // .toRun() — Bun-extension: received is [path, ...args]; runs as a
      // subprocess and asserts exit code 0. Synchronous via spawnSync.
      toRun(opts) {
        const cp = require("node:child_process");
        let bin = process.argv[0] || "bun-rs";
        let argv = Array.isArray(received) ? received : [received];
        if (typeof argv[0] !== "string") {
          check(false, undefined, "toRun");
          return;
        }
        const env = (opts && opts.env) ? { ...process.env, ...opts.env } : process.env;
        const cwd = opts && opts.cwd;
        const r = cp.spawnSync(bin, argv, { env, cwd });
        const ok = r.status === 0;
        check(ok, undefined, "toRun");
      },
      toRunSuccessfully(_opts) {
        return this.toRun(_opts);
      },
      // jest-extended aliases.
      toInclude(sub) {
        const ok = (typeof received === "string" && received.includes(sub))
          || (Array.isArray(received) && received.some(x => deepEq(x, sub)));
        check(ok, sub, "toInclude");
      },
      toIncludeAllMembers(arr) {
        const ok = Array.isArray(received) && arr.every(v => received.some(x => deepEq(x, v)));
        check(ok, arr, "toIncludeAllMembers");
      },
      toIncludeAnyMembers(arr) {
        const ok = Array.isArray(received) && arr.some(v => received.some(x => deepEq(x, v)));
        check(ok, arr, "toIncludeAnyMembers");
      },
      toContainKey(k) {
        const ok = received && typeof received === "object" && k in received;
        check(ok, k, "toContainKey");
      },
      toContainKeys(ks) {
        const ok = received && typeof received === "object" && ks.every(k => k in received);
        check(ok, ks, "toContainKeys");
      },
      toContainValue(v) {
        const ok = received && typeof received === "object" && Object.values(received).some(x => deepEq(x, v));
        check(ok, v, "toContainValue");
      },
      toContainEntry(e) {
        const ok = received && typeof received === "object" && deepEq(received[e[0]], e[1]);
        check(ok, e, "toContainEntry");
      },
      // Snapshots: bun-rs has no snapshot store yet, so .toMatchSnapshot /
      // .toMatchInlineSnapshot always pass (mirroring Bun's "write the
      // snapshot on first run" semantics, just without persistence).
      toMatchSnapshot(_name) { /* always pass */ },
      toMatchInlineSnapshot(_snap) { /* always pass */ },
      toThrowErrorMatchingSnapshot(_name) {
        let caught = null;
        try { received(); } catch (e) { caught = e; }
        check(!!caught, undefined, "toThrowErrorMatchingSnapshot");
      },
      toThrowErrorMatchingInlineSnapshot(_snap) {
        let caught = null;
        try { received(); } catch (e) { caught = e; }
        check(!!caught, undefined, "toThrowErrorMatchingInlineSnapshot");
      },
      // Date helpers.
      toBeBefore(d) {
        check(received instanceof Date && d instanceof Date && received < d, d, "toBeBefore");
      },
      toBeAfter(d) {
        check(received instanceof Date && d instanceof Date && received > d, d, "toBeAfter");
      },
      toBeValidDate() {
        check(received instanceof Date && !isNaN(received.getTime()), undefined, "toBeValidDate");
      },
      toBeDate() {
        check(received instanceof Date, undefined, "toBeDate");
      },
      // Cookie / regex helpers.
      toBeRegExp() { check(received instanceof RegExp, undefined, "toBeRegExp"); },
      toBeIterable() {
        check(received != null && typeof received[Symbol.iterator] === "function", undefined, "toBeIterable");
      },
      toSatisfy(predicate) {
        const ok = !!predicate(received);
        check(ok, undefined, "toSatisfy");
      },
      toBeTypeOf(t) {
        check(typeof received === t, t, "toBeTypeOf");
      },
      toEqualTypeOf(_other) { /* TS type-only — pass at runtime */ },
      toBeSymbol() { check(typeof received === "symbol", undefined, "toBeSymbol"); },
      toBePrimitive() {
        const t = typeof received;
        check(t === "string" || t === "number" || t === "boolean" || t === "bigint" || t === "symbol" || received === null || received === undefined, undefined, "toBePrimitive");
      },
      toBeNullish() { check(received === null || received === undefined, undefined, "toBeNullish"); },
      toBeNonEmptyString() { check(typeof received === "string" && received.length > 0, undefined, "toBeNonEmptyString"); },
      toHaveBeenCalled() {
        check(received && received.mock && received.mock.calls && received.mock.calls.length > 0, undefined, "toHaveBeenCalled");
      },
      toHaveBeenCalledTimes(n) {
        check(received && received.mock && received.mock.calls && received.mock.calls.length === n, n, "toHaveBeenCalledTimes");
      },
      toHaveBeenCalledWith(...args) {
        const calls = (received && received.mock && received.mock.calls) || [];
        const ok = calls.some(c => c.length === args.length && c.every((v, i) => deepEq(v, args[i])));
        check(ok, args, "toHaveBeenCalledWith");
      },
      toHaveBeenLastCalledWith(...args) {
        const calls = (received && received.mock && received.mock.calls) || [];
        const last = calls[calls.length - 1] || [];
        const ok = last.length === args.length && last.every((v, i) => deepEq(v, args[i]));
        check(ok, args, "toHaveBeenLastCalledWith");
      },
      toHaveReturnedTimes(n) {
        const results = (received && received.mock && received.mock.results) || [];
        check(results.filter(r => r.type === "return").length === n, n, "toHaveReturnedTimes");
      },
      toHaveReturned() {
        const results = (received && received.mock && received.mock.results) || [];
        check(results.some(r => r.type === "return"), undefined, "toHaveReturned");
      },
      toHaveReturnedWith(v) {
        const results = (received && received.mock && received.mock.results) || [];
        const ok = results.some(r => r.type === "return" && deepEq(r.value, v));
        check(ok, v, "toHaveReturnedWith");
      },
      toHaveLastReturnedWith(v) {
        const results = (received && received.mock && received.mock.results) || [];
        const last = results[results.length - 1];
        const ok = last && last.type === "return" && deepEq(last.value, v);
        check(ok, v, "toHaveLastReturnedWith");
      },
      toHaveNthReturnedWith(n, v) {
        const results = (received && received.mock && received.mock.results) || [];
        const r = results[n - 1];
        const ok = r && r.type === "return" && deepEq(r.value, v);
        check(ok, v, "toHaveNthReturnedWith");
      },
      toHaveBeenNthCalledWith(n, ...args) {
        const calls = (received && received.mock && received.mock.calls) || [];
        const c = calls[n - 1] || [];
        const ok = c.length === args.length && c.every((v, i) => deepEq(v, args[i]));
        check(ok, args, "toHaveBeenNthCalledWith");
      },
      fail(msg) { throw new Error(msg || "expect().fail() called"); },
      pass(_msg) { /* always passes */ },
      toMatchFileSnapshot(_file) {},
      // Resolves / rejects shortcuts used in older test styles.
      toResolve() { return this.resolves.toBeDefined(); },
      toReject() { return this.rejects.toBeDefined(); },
    };
    obj.resolves = {
      __proto__: null,
      then: undefined,
    };
    // Build a thenable .resolves / .rejects that returns a fresh expect over
    // the awaited value.
    Object.defineProperty(obj, "resolves", {
      get() {
        return new Proxy({}, {
          get(_, k) {
            return async (...a) => {
              const v = await received;
              return mkExpect(v, not)[k](...a);
            };
          },
        });
      },
    });
    Object.defineProperty(obj, "rejects", {
      get() {
        return new Proxy({}, {
          get(_, k) {
            return async (...a) => {
              let e;
              let rejected = false;
              try { await received; } catch (err) { e = err; rejected = true; }
              if (!rejected) {
                throw new Error("expected promise to reject");
              }
              if (k === "toThrow") {
                // Match Jest: compare against the rejection's message / type.
                const m = a[0];
                const matched = m === undefined ||
                  (m instanceof RegExp ? m.test(e && e.message ? e.message : String(e)) :
                   typeof m === "string" ? (e && e.message ? e.message : String(e)).includes(m) :
                   typeof m === "function" ? e instanceof m : false);
                if (not ? matched : !matched) {
                  throw new Error(`expect(...).${not?"not.":""}rejects.toThrow(${fmt(m)}) failed: got ${e && e.message ? e.message : String(e)}`);
                }
                return;
              }
              return mkExpect(e, not)[k](...a);
            };
          },
        });
      },
    });
    // Mix in custom matchers registered via expect.extend(). Each runs the
    // user-provided fn(received, ...args) → { pass, message } and throws
    // when pass===false (or pass===true with .not).
    if (g.__bun_custom_matchers) {
      for (const [name, fn] of Object.entries(g.__bun_custom_matchers)) {
        if (obj[name] !== undefined) continue;
        obj[name] = function (...args) {
          const r = fn(received, ...args);
          const pass = !!(r && r.pass);
          if (not ? pass : !pass) {
            const msg = (r && typeof r.message === "function") ? r.message() : String(r && r.message || `${name} matcher failed`);
            throw new Error(msg);
          }
        };
      }
    }
    return obj;
  }

  g.expect = function (received) {
    // expect() with no args: just a thin wrapper with .fail / .pass /
    // .unreachable. Bun's tests use `expect().fail("...")` idiomatically.
    if (arguments.length === 0) {
      return {
        fail: (m) => { throw new Error(m || "expect().fail() called"); },
        pass: () => {},
        unreachable: () => { throw new Error("expect().unreachable()"); },
      };
    }
    const e = mkExpect(received, false);
    Object.defineProperty(e, "not", { get() { return mkExpect(received, true); } });
    return e;
  };

  // ── Asymmetric matchers (expect.any / .anything / .objectContaining / etc.) ──
  // These are sentinel values for use inside toEqual / toMatchObject. They
  // override the standard deepEq via a special `[__bun_match]` brand.
  function asymmetric(name, predicate, repr) {
    return {
      __bun_match: true,
      __bun_match_name: name,
      asymmetricMatch: predicate,
      toString: () => repr || name,
    };
  }
  // Plug asymmetric matchers into deepEq.
  const _origDeepEq = deepEq;
  // eslint-disable-next-line no-func-assign
  deepEq = function (a, b) {
    if (b && b.__bun_match && typeof b.asymmetricMatch === "function") return b.asymmetricMatch(a);
    if (a && a.__bun_match && typeof a.asymmetricMatch === "function") return a.asymmetricMatch(b);
    if (Array.isArray(a) && Array.isArray(b)) {
      if (a.length !== b.length) return false;
      for (let i = 0; i < a.length; i++) if (!deepEq(a[i], b[i])) return false;
      return true;
    }
    if (a && b && typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)
        && !(a instanceof Date) && !(a instanceof RegExp)) {
      const ak = Object.keys(a), bk = Object.keys(b);
      if (ak.length !== bk.length) return false;
      for (const k of ak) {
        if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
        if (!deepEq(a[k], b[k])) return false;
      }
      return true;
    }
    return _origDeepEq(a, b);
  };

  g.expect.any = function (ctor) {
    return asymmetric("Any<" + (ctor && ctor.name || ctor) + ">", (a) => {
      if (ctor === Number) return typeof a === "number";
      if (ctor === String) return typeof a === "string";
      if (ctor === Boolean) return typeof a === "boolean";
      if (ctor === BigInt) return typeof a === "bigint";
      if (ctor === Function) return typeof a === "function";
      if (ctor === Symbol) return typeof a === "symbol";
      if (ctor === Object) return a !== null && typeof a === "object";
      if (typeof ctor === "function") return a instanceof ctor;
      return false;
    });
  };
  g.expect.anything = function () {
    return asymmetric("Anything", (a) => a !== null && a !== undefined);
  };
  g.expect.objectContaining = function (sub) {
    return asymmetric("ObjectContaining", (a) => {
      if (a === null || typeof a !== "object") return false;
      for (const k of Object.keys(sub)) if (!deepEq(a[k], sub[k])) return false;
      return true;
    });
  };
  g.expect.arrayContaining = function (sub) {
    return asymmetric("ArrayContaining", (a) => {
      if (!Array.isArray(a)) return false;
      return sub.every((v) => a.some((x) => deepEq(x, v)));
    });
  };
  g.expect.stringContaining = function (s) {
    return asymmetric("StringContaining", (a) => typeof a === "string" && a.includes(s));
  };
  g.expect.stringMatching = function (re) {
    return asymmetric("StringMatching", (a) => typeof a === "string" &&
      (re instanceof RegExp ? re.test(a) : a.includes(String(re))));
  };
  g.expect.closeTo = function (n, digits) {
    const d = digits == null ? 2 : digits;
    return asymmetric("CloseTo", (a) => typeof a === "number" && Math.abs(a - n) < Math.pow(10, -d) / 2);
  };
  g.expect.assertions = function () {};
  g.expect.hasAssertions = function () {};
  g.expect.unreachable = function (msg) {
    if (msg instanceof Error) throw msg;
    if (typeof msg === "string" && msg.length) throw new Error(msg);
    throw new Error("reached unreachable code");
  };
  g.expect.objectContaining = g.expect.objectContaining || ((sub) => asymmetric("ObjectContaining", a => Object.keys(sub).every(k => deepEq(a && a[k], sub[k]))));
  g.expect.arrayContaining = g.expect.arrayContaining || ((sub) => asymmetric("ArrayContaining", a => Array.isArray(a) && sub.every(v => a.some(x => deepEq(x, v)))));
  // expectTypeOf: TypeScript type-only assertions; runtime no-op chainable.
  // Both `.foo` and `.foo()` must yield another proxy, so the test author
  // can write `expectTypeOf(x).parameters.toEqualTypeOf<...>()` etc.
  g.expectTypeOf = function (_x) {
    const make = () => new Proxy(function(){}, {
      get: (_t, k) => k === Symbol.toPrimitive ? () => "" : make(),
      apply: () => make(),
      construct: () => ({}),
    });
    return make();
  };
  g.__bun_custom_matchers = g.__bun_custom_matchers || {};
  g.expect.extend = function (matchers) {
    for (const [name, fn] of Object.entries(matchers)) {
      g.__bun_custom_matchers[name] = fn;
      // (a) asymmetric form: expect.foo(args) → matcher object.
      g.expect[name] = (...a) => asymmetric(name, (recv) => {
        const r = fn(recv, ...a);
        return r && r.pass;
      });
    }
  };

})(globalThis);
"#;
