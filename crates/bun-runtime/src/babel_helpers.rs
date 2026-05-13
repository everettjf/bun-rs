//! `globalThis.babelHelpers` — runtime support for transformer-lowered output.
//!
//! oxc_transformer in External mode emits references to `babelHelpers.foo`
//! instead of inlining the helper body or requiring an external module. We
//! install one global object with each helper a feature we've turned on can
//! actually need.
//!
//! Current scope:
//!   - `usingCtx` — for ES2026 `using` / `await using` declarations
//!     (enabled in bun-transpile because JSC's parser rejects the syntax).
//!
//! When we enable more transforms (class private fields, decorators, ...)
//! the corresponding helpers go here too.

use bun_jsc::Context;

pub fn install(ctx: &Context) {
    let _ = ctx.eval(POLYFILL, Some("[babel-helpers]"));
}

// `usingCtx` is the spec-faithful version of the Babel runtime helper. Returns
// an object with:
//   - `u(value)` → register sync `using` resource; returns value
//   - `a(value)` → register `await using` resource; returns value
//   - `e`        → slot for caught error (assigned to by emitted code)
//   - `d()`      → dispose all in reverse order, rethrow `e` if set; for
//                  `await using` returns a Promise.
//
// Uses `Symbol.dispose` / `Symbol.asyncDispose` (Stage 3 → Stage 4 well-known
// symbols). JSC ships them; older engines fall back to `Symbol.for(...)`.
const POLYFILL: &str = r#"
(function (g) {
  // JSC versions before 7619 don't expose Symbol.dispose / Symbol.asyncDispose
  // as well-known symbols. Polyfill them with registered symbols so user code
  // using `[Symbol.dispose]() { ... }` ends up with the same key everywhere.
  // Polyfill SuppressedError as a global if JSC doesn't expose it.
  if (typeof globalThis.SuppressedError !== "function") {
    globalThis.SuppressedError = function SuppressedError(error, suppressed, message) {
      const e = new Error(message || "");
      e.name = "SuppressedError";
      e.error = error;
      e.suppressed = suppressed;
      return e;
    };
  }

  if (typeof Symbol.dispose !== "symbol") {
    Object.defineProperty(Symbol, "dispose", {
      value: Symbol.for("Symbol.dispose"),
      configurable: false,
      writable: false,
    });
  }
  if (typeof Symbol.asyncDispose !== "symbol") {
    Object.defineProperty(Symbol, "asyncDispose", {
      value: Symbol.for("Symbol.asyncDispose"),
      configurable: false,
      writable: false,
    });
  }

  function _usingCtx() {
    var SuppressedErrorCtor =
      typeof SuppressedError === "function"
        ? SuppressedError
        : function (error, suppressed) {
            var err = new Error();
            err.name = "SuppressedError";
            err.error = error;
            err.suppressed = suppressed;
            return err;
          };
    var empty = {};
    var stack = [];
    function using(isAwait, value) {
      if (value !== null && value !== undefined) {
        if (Object(value) !== value) {
          throw new TypeError(
            "using declarations can only be used with objects, null, or undefined."
          );
        }
        var dispose;
        if (isAwait) {
          dispose = value[Symbol.asyncDispose || Symbol.for("Symbol.asyncDispose")];
        }
        if (dispose === undefined || dispose === null) {
          dispose = value[Symbol.dispose || Symbol.for("Symbol.dispose")];
        }
        if (typeof dispose !== "function") {
          throw new TypeError("Property [Symbol.dispose] is not a function.");
        }
        stack.push({ v: value, d: dispose, a: isAwait });
      } else if (isAwait) {
        // null/undefined await using is a no-op except the await tick.
        stack.push({ d: value, a: isAwait });
      }
      return value;
    }
    return {
      e: empty,
      u: using.bind(null, false),
      a: using.bind(null, true),
      d: function () {
        var error = this.e;
        function next() {
          var entry;
          while ((entry = stack.pop())) {
            try {
              var result = entry.d && entry.d.call(entry.v);
              if (entry.a) {
                return Promise.resolve(result).then(next, err);
              }
            } catch (e) {
              return err(e);
            }
          }
          if (error !== empty) throw error;
        }
        function err(e) {
          error = error !== empty ? new SuppressedErrorCtor(e, error) : e;
          return next();
        }
        return next();
      },
    };
  }

  g.babelHelpers = g.babelHelpers || {};
  g.babelHelpers.usingCtx = _usingCtx;
})(globalThis);
"#;
