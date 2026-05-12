//! `node:querystring` — parse / stringify / escape / unescape.

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let v = ctx.eval(POLYFILL, Some("[node:querystring]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("default", &v).unwrap();
    v
}

const POLYFILL: &str = r#"
(() => {
  const qs = {};
  qs.escape = encodeURIComponent;
  qs.unescape = decodeURIComponent;
  qs.parse = function (s, sep, eq, options) {
    sep = sep || "&";
    eq = eq || "=";
    const maxKeys = (options && options.maxKeys != null) ? options.maxKeys : 1000;
    const out = {};
    if (typeof s !== "string" || s.length === 0) return out;
    const parts = s.split(sep);
    let count = 0;
    for (const part of parts) {
      if (maxKeys && count++ >= maxKeys) break;
      const idx = part.indexOf(eq);
      let k, v;
      if (idx === -1) { k = part; v = ""; }
      else { k = part.slice(0, idx); v = part.slice(idx + 1); }
      try { k = qs.unescape(k.replace(/\+/g, " ")); } catch { /* keep raw */ }
      try { v = qs.unescape(v.replace(/\+/g, " ")); } catch { /* keep raw */ }
      if (out[k] === undefined) out[k] = v;
      else if (Array.isArray(out[k])) out[k].push(v);
      else out[k] = [out[k], v];
    }
    return out;
  };
  qs.stringify = function (obj, sep, eq, options) {
    sep = sep || "&";
    eq = eq || "=";
    if (!obj || typeof obj !== "object") return "";
    const parts = [];
    for (const k of Object.keys(obj)) {
      const ek = qs.escape(k);
      const v = obj[k];
      if (Array.isArray(v)) {
        for (const item of v) parts.push(ek + eq + qs.escape(item == null ? "" : String(item)));
      } else if (v == null) {
        parts.push(ek + eq);
      } else {
        parts.push(ek + eq + qs.escape(String(v)));
      }
    }
    return parts.join(sep);
  };
  qs.encode = qs.stringify;
  qs.decode = qs.parse;
  return qs;
})()
"#;
