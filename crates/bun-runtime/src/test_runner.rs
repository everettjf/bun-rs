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

    let rt = crate::Runtime::new(vec!["bun-rs".to_string(), "test".to_string()]);
    install_globals(&rt.ctx);

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
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
                    let pass = 0, fail = 0;
                    const failed = [];
                    async function runOne(t) {
                        const fullName = t.path.length ? t.path.concat(t.name).join(" > ") : t.name;
                        try {
                            for (const h of t.beforeEach) await h();
                            await t.fn();
                            for (const h of t.afterEach) await h();
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
                    return { pass, fail, failed };
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
        total_pass += file_pass;
        total_fail += file_fail;
        if file_fail > 0 {
            failed_files.push(file.display().to_string());
        }
    }

    eprintln!("\n──────────");
    eprintln!("files:  {} ({} failed)", files.len(), failed_files.len());
    eprintln!("tests:  {} passed, {} failed", total_pass, total_fail);
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
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.tsx")
        || name.ends_with(".spec.js")
}

fn install_globals(ctx: &Context) {
    ctx.eval(
        GLOBALS,
        Some("[test-globals]"),
    )
    .expect("install test globals");
}

const GLOBALS: &str = r#"
(function (g) {
  g.__bun_test_collector = [];
  // describe stack — current nested name + hooks
  const stack = [{ path: [], beforeAll: [], afterAll: [], beforeEach: [], afterEach: [] }];

  function curr() { return stack[stack.length - 1]; }

  g.describe = function (name, body) {
    stack.push({
      path: [...curr().path, name],
      beforeAll: [...curr().beforeAll],
      afterAll: [...curr().afterAll],
      beforeEach: [...curr().beforeEach],
      afterEach: [...curr().afterEach],
    });
    try { body(); } finally { stack.pop(); }
  };

  function pushTest(name, fn, opts) {
    const c = curr();
    g.__bun_test_collector.push({
      name, fn,
      path: c.path,
      beforeEach: [...c.beforeEach],
      afterEach: [...c.afterEach],
      skip: !!(opts && opts.skip),
    });
  }

  g.test = (name, fn) => pushTest(name, fn);
  g.it = g.test;
  g.test.skip = (name) => pushTest(name, () => {}, { skip: true });
  g.it.skip = g.test.skip;

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
      toHaveLength(n) { check(received && received.length === n, n, "toHaveLength"); },
      toMatch(re) { check(typeof re === "string" ? received.includes(re) : re.test(received), re, "toMatch"); },
      toThrow(matcher) {
        let caught;
        try { received(); } catch (e) { caught = e; }
        const matched = !!caught && (matcher === undefined ||
          (matcher instanceof RegExp ? matcher.test(caught.message || String(caught)) :
           typeof matcher === "string" ? (caught.message || String(caught)).includes(matcher) :
           typeof matcher === "function" ? caught instanceof matcher : false));
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
    return obj;
  }

  g.expect = function (received) {
    const e = mkExpect(received, false);
    Object.defineProperty(e, "not", { get() { return mkExpect(received, true); } });
    return e;
  };
})(globalThis);
"#;
