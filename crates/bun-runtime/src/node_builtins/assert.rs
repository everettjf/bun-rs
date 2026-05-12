//! `node:assert` — pure JS implementation.
//!
//! Provides `assert()` (callable) plus `.ok / .equal / .notEqual /
//! .strictEqual / .notStrictEqual / .deepEqual / .deepStrictEqual /
//! .throws / .doesNotThrow / .rejects / .doesNotReject / .fail / .match /
//! .doesNotMatch / .ifError`. Errors are `AssertionError` with the same
//! shape as Node.

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let v = ctx.eval(POLYFILL, Some("[node:assert]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("default", &v).unwrap();
    v
}

const POLYFILL: &str = r#"
(() => {
  class AssertionError extends Error {
    constructor(opts) {
      opts = opts || {};
      const msg = opts.message || `expected ${stringify(opts.expected)}, got ${stringify(opts.actual)}`;
      super(msg);
      this.name = "AssertionError";
      this.actual = opts.actual;
      this.expected = opts.expected;
      this.operator = opts.operator;
      this.code = "ERR_ASSERTION";
    }
  }

  function stringify(v) {
    if (v === undefined) return "undefined";
    if (v === null) return "null";
    if (typeof v === "function") return `[Function: ${v.name || "anonymous"}]`;
    try { return JSON.stringify(v); } catch { return String(v); }
  }

  function isDeepEqual(a, b, strict, seen) {
    if (a === b) return true;
    if (strict) {
      if (typeof a !== typeof b) return false;
    } else {
      // ==-style equality, but only collapse undefined/null and number/string
      if (a == b) return true;
    }
    if (a === null || b === null) return false;
    if (typeof a !== "object" || typeof b !== "object") return false;
    seen = seen || new WeakMap();
    if (seen.has(a) && seen.get(a) === b) return true;
    seen.set(a, b);
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    if (a.constructor !== b.constructor && strict) return false;
    if (a instanceof Date && b instanceof Date) return a.getTime() === b.getTime();
    if (a instanceof RegExp && b instanceof RegExp) return a.toString() === b.toString();
    if (ArrayBuffer.isView(a) && ArrayBuffer.isView(b)) {
      if (a.byteLength !== b.byteLength) return false;
      for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
      return true;
    }
    if (a instanceof Map && b instanceof Map) {
      if (a.size !== b.size) return false;
      for (const [k, v] of a) if (!isDeepEqual(v, b.get(k), strict, seen)) return false;
      return true;
    }
    if (a instanceof Set && b instanceof Set) {
      if (a.size !== b.size) return false;
      for (const v of a) if (!b.has(v)) return false;
      return true;
    }
    const ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (const k of ka) {
      if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
      if (!isDeepEqual(a[k], b[k], strict, seen)) return false;
    }
    return true;
  }

  function assert(actual, message) {
    if (!actual) {
      throw new AssertionError({
        actual, expected: true, operator: "==", message: message || "false == true",
      });
    }
  }

  assert.AssertionError = AssertionError;
  assert.ok = assert;
  assert.fail = (message) => {
    throw new AssertionError({ message: typeof message === "string" ? message : "Failed" });
  };
  assert.equal = (actual, expected, message) => {
    if (actual != expected) {
      throw new AssertionError({ actual, expected, operator: "==", message });
    }
  };
  assert.notEqual = (actual, expected, message) => {
    if (actual == expected) {
      throw new AssertionError({ actual, expected, operator: "!=", message });
    }
  };
  assert.strictEqual = (actual, expected, message) => {
    if (!Object.is(actual, expected)) {
      throw new AssertionError({ actual, expected, operator: "===", message });
    }
  };
  assert.notStrictEqual = (actual, expected, message) => {
    if (Object.is(actual, expected)) {
      throw new AssertionError({ actual, expected, operator: "!==", message });
    }
  };
  assert.deepEqual = (actual, expected, message) => {
    if (!isDeepEqual(actual, expected, false)) {
      throw new AssertionError({ actual, expected, operator: "deepEqual", message });
    }
  };
  assert.notDeepEqual = (actual, expected, message) => {
    if (isDeepEqual(actual, expected, false)) {
      throw new AssertionError({ actual, expected, operator: "notDeepEqual", message });
    }
  };
  assert.deepStrictEqual = (actual, expected, message) => {
    if (!isDeepEqual(actual, expected, true)) {
      throw new AssertionError({ actual, expected, operator: "deepStrictEqual", message });
    }
  };
  assert.notDeepStrictEqual = (actual, expected, message) => {
    if (isDeepEqual(actual, expected, true)) {
      throw new AssertionError({ actual, expected, operator: "notDeepStrictEqual", message });
    }
  };
  assert.throws = (fn, expected, message) => {
    let caught;
    try { fn(); } catch (e) { caught = e; }
    if (!caught) {
      throw new AssertionError({ message: message || "Missing expected exception" });
    }
    if (expected && !matchError(caught, expected)) {
      throw new AssertionError({ actual: caught, expected, operator: "throws", message });
    }
  };
  assert.doesNotThrow = (fn, message) => {
    try { fn(); } catch (e) {
      throw new AssertionError({ actual: e, operator: "doesNotThrow", message });
    }
  };
  assert.rejects = async (promiseOrFn, expected, message) => {
    let p = typeof promiseOrFn === "function" ? promiseOrFn() : promiseOrFn;
    let caught;
    try { await p; } catch (e) { caught = e; }
    if (!caught) {
      throw new AssertionError({ message: message || "Missing expected rejection" });
    }
    if (expected && !matchError(caught, expected)) {
      throw new AssertionError({ actual: caught, expected, operator: "rejects", message });
    }
  };
  assert.doesNotReject = async (promiseOrFn, message) => {
    let p = typeof promiseOrFn === "function" ? promiseOrFn() : promiseOrFn;
    try { await p; } catch (e) {
      throw new AssertionError({ actual: e, operator: "doesNotReject", message });
    }
  };
  assert.match = (s, re, message) => {
    if (!re.test(s)) {
      throw new AssertionError({ actual: s, expected: re, operator: "match", message });
    }
  };
  assert.doesNotMatch = (s, re, message) => {
    if (re.test(s)) {
      throw new AssertionError({ actual: s, expected: re, operator: "doesNotMatch", message });
    }
  };
  assert.ifError = (err) => {
    if (err !== null && err !== undefined) {
      throw new AssertionError({ actual: err, expected: null, operator: "ifError" });
    }
  };

  function matchError(err, expected) {
    if (typeof expected === "function") {
      // Class or predicate
      if (err instanceof expected) return true;
      try { return expected(err) === true; } catch { return false; }
    }
    if (expected instanceof RegExp) return expected.test(err && err.message ? err.message : String(err));
    if (typeof expected === "object" && expected !== null) {
      for (const k of Object.keys(expected)) {
        if (err[k] !== expected[k]) return false;
      }
      return true;
    }
    return false;
  }

  // `strict` namespace where loose-equality methods alias to strict ones.
  const strict = Object.assign(function strict(...args) { return assert(...args); }, assert);
  strict.equal = assert.strictEqual;
  strict.notEqual = assert.notStrictEqual;
  strict.deepEqual = assert.deepStrictEqual;
  strict.notDeepEqual = assert.notDeepStrictEqual;
  assert.strict = strict;
  strict.strict = strict;

  return assert;
})()
"#;
