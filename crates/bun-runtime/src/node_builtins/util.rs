//! `node:util` — promisify, inspect, types, format, debuglog (no-op).

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let v = ctx.eval(POLYFILL, Some("[node:util]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("default", &v).unwrap();
    v
}

const POLYFILL: &str = r#"
(() => {
  const util = {};

  // promisify(fn): converts (...args, cb(err, val)) into Promise<val>.
  util.promisify = function (fn) {
    const promisified = function (...args) {
      return new Promise((resolve, reject) => {
        fn.call(this, ...args, (err, value) => {
          if (err) reject(err); else resolve(value);
        });
      });
    };
    Object.setPrototypeOf(promisified, Object.getPrototypeOf(fn));
    return promisified;
  };
  util.promisify.custom = Symbol.for("util.promisify.custom");

  // callbackify is the inverse: async fn → (...args, cb)
  util.callbackify = function (fn) {
    return function (...args) {
      const cb = args.pop();
      fn(...args).then(v => cb(null, v), e => cb(e));
    };
  };

  // inspect: a small subset of Node's debug-formatter.
  util.inspect = function (value, opts) {
    opts = opts || {};
    const depth = opts.depth === undefined ? 2 : (opts.depth === null ? Infinity : opts.depth);
    const seen = new WeakSet();
    function fmt(v, d) {
      if (v === null) return "null";
      if (v === undefined) return "undefined";
      if (typeof v === "string") return JSON.stringify(v);
      if (typeof v === "number" || typeof v === "boolean") return String(v);
      if (typeof v === "function") return "[Function: " + (v.name || "anonymous") + "]";
      if (typeof v === "symbol") return v.toString();
      if (typeof v === "bigint") return v.toString() + "n";
      if (typeof v !== "object") return String(v);
      if (seen.has(v)) return "[Circular]";
      seen.add(v);
      if (d <= 0) return Array.isArray(v) ? "[Array]" : "[Object]";
      if (v instanceof Date) return v.toISOString();
      if (v instanceof RegExp) return v.toString();
      if (v instanceof Error) return v.stack || (v.name + ": " + v.message);
      if (Array.isArray(v)) {
        const items = v.map(item => fmt(item, d - 1));
        return "[ " + items.join(", ") + " ]";
      }
      if (v instanceof Map) {
        const items = [];
        for (const [k, val] of v) items.push(fmt(k, d - 1) + " => " + fmt(val, d - 1));
        return "Map(" + v.size + ") { " + items.join(", ") + " }";
      }
      if (v instanceof Set) {
        const items = [];
        for (const item of v) items.push(fmt(item, d - 1));
        return "Set(" + v.size + ") { " + items.join(", ") + " }";
      }
      const keys = Object.keys(v);
      const parts = keys.map(k => k + ": " + fmt(v[k], d - 1));
      const tag = v.constructor && v.constructor.name && v.constructor.name !== "Object"
        ? v.constructor.name + " "
        : "";
      return tag + "{ " + parts.join(", ") + " }";
    }
    return fmt(value, depth);
  };

  // format("%s/%d/%j", ...)
  util.format = function (...args) {
    if (typeof args[0] !== "string") {
      return args.map(a => util.inspect(a)).join(" ");
    }
    const fmt = args[0];
    let i = 1;
    const out = fmt.replace(/%[sdjifoO%]/g, m => {
      if (m === "%%") return "%";
      const arg = args[i++];
      switch (m) {
        case "%s": return arg == null ? String(arg) : String(arg);
        case "%d": case "%i": case "%f": return typeof arg === "number" ? String(arg) : (arg == null ? String(arg) : String(arg));
        case "%j": return JSON.stringify(arg);
        case "%o": case "%O": return util.inspect(arg);
      }
      return m;
    });
    if (i < args.length) {
      // Append leftover args as Node does (only when format ran out before
      // args did).
      return out + " " + args.slice(i).map(a => util.inspect(a)).join(" ");
    }
    return out;
  };

  // debuglog: returns a no-op unless NODE_DEBUG includes the section.
  util.debuglog = function (section) {
    const env = (typeof process !== "undefined" && process.env && process.env.NODE_DEBUG) || "";
    const re = env.split(",").map(s => s.trim()).filter(Boolean);
    const enabled = re.includes("*") || re.includes(section);
    if (!enabled) return () => {};
    return function (...a) {
      console.error("[" + section + "]", util.format(...a));
    };
  };
  util.debug = util.debuglog;

  // types: minimal — just the ones libraries use heavily.
  util.types = {
    isArrayBuffer: v => v instanceof ArrayBuffer,
    isUint8Array: v => v instanceof Uint8Array,
    isDate: v => v instanceof Date,
    isMap: v => v instanceof Map,
    isSet: v => v instanceof Set,
    isRegExp: v => v instanceof RegExp,
    isPromise: v => v instanceof Promise,
    isAsyncFunction: v => typeof v === "function" && v.constructor && v.constructor.name === "AsyncFunction",
    isNativeError: v => v instanceof Error,
    isTypedArray: v => ArrayBuffer.isView(v) && !(v instanceof DataView),
  };

  // Deprecate-style: just return the function.
  util.deprecate = function (fn, _msg, _code) { return fn; };

  // Convenience: util.inherits (old style)
  util.inherits = function (ctor, super_) {
    Object.setPrototypeOf(ctor.prototype, super_.prototype);
  };

  return util;
})()
"#;
