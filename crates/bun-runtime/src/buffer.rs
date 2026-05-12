//! `globalThis.Buffer` and the `node:buffer` module.
//!
//! Buffer is a JS class that extends `Uint8Array`; we install it as a global
//! and route `node:buffer` to the same exports.
//!
//! The class is defined in pure JS (extending Uint8Array). For zero-copy
//! creation from Rust we go through `Value::new_uint8_array` and then call
//! `Buffer.from(uint8)` so the result is a Buffer instance.

use bun_jsc::{Context, Value};

pub fn install(ctx: &Context) {
    ctx.eval(POLYFILL, Some("[buffer-polyfill]"))
        .expect("install Buffer polyfill");
}

/// Take a Rust byte buffer and return it as a Buffer instance in JS.
/// Equivalent to `Buffer.from(uint8)`, but zero-copy.
pub fn buffer_from_bytes<'ctx>(ctx: &'ctx Context, bytes: Vec<u8>) -> Value<'ctx> {
    let u8arr = Value::new_uint8_array(ctx, bytes);
    let buffer_ctor = match ctx
        .global_object()
        .get_property("Buffer")
        .and_then(|v| v.to_object())
    {
        Ok(o) => o,
        Err(_) => return u8arr,
    };
    let from_fn = match buffer_ctor.get_property("from").and_then(|v| v.to_object()) {
        Ok(o) => o,
        Err(_) => return u8arr,
    };
    from_fn
        .call(Some(buffer_ctor), &[u8arr])
        .unwrap_or_else(|_| Value::new_undefined(ctx))
}

const POLYFILL: &str = r#"
(function (g) {
  // Buffer extends Uint8Array. The C++ Node version has a lot more, but
  // this covers ~90% of common usage.
  class Buffer extends Uint8Array {
    static from(value, encodingOrOffset, length) {
      if (value instanceof Uint8Array) {
        // From a typed array — sharing backing store is what Node does.
        const buf = new Buffer(value.buffer, value.byteOffset, value.byteLength);
        return buf;
      }
      if (Array.isArray(value)) {
        const buf = new Buffer(value.length);
        for (let i = 0; i < value.length; i++) buf[i] = value[i] & 0xff;
        return buf;
      }
      if (typeof value === "string") {
        const encoding = (encodingOrOffset || "utf-8").toLowerCase();
        return Buffer._fromString(value, encoding);
      }
      if (value instanceof ArrayBuffer) {
        return new Buffer(value, encodingOrOffset || 0, length);
      }
      if (typeof value === "number") {
        throw new TypeError("Buffer.from(number) is not supported; use Buffer.alloc(n)");
      }
      throw new TypeError("Unsupported Buffer.from input: " + typeof value);
    }

    static _fromString(s, encoding) {
      if (encoding === "utf8" || encoding === "utf-8") {
        const enc = new TextEncoder();
        return Buffer.from(enc.encode(s));
      }
      if (encoding === "ascii" || encoding === "latin1" || encoding === "binary") {
        const buf = new Buffer(s.length);
        for (let i = 0; i < s.length; i++) buf[i] = s.charCodeAt(i) & 0xff;
        return buf;
      }
      if (encoding === "hex") {
        if (s.length % 2 !== 0) throw new TypeError("invalid hex");
        const buf = new Buffer(s.length / 2);
        for (let i = 0; i < buf.length; i++) {
          buf[i] = parseInt(s.slice(i * 2, i * 2 + 2), 16);
        }
        return buf;
      }
      if (encoding === "base64") {
        // atob is part of the JSC context (added in 12.0+); fall back to
        // a manual decode if not. JSC bare context lacks atob, so do it.
        const lut = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        const clean = s.replace(/[^A-Za-z0-9+/]/g, "");
        const pad = (clean.endsWith("==")) ? 2 : (clean.endsWith("=") ? 1 : 0);
        const out = new Buffer(((clean.length * 3) >> 2) - pad);
        let p = 0;
        for (let i = 0; i < clean.length; i += 4) {
          const b1 = lut.indexOf(clean[i]);
          const b2 = lut.indexOf(clean[i + 1]);
          const b3 = i + 2 < clean.length ? lut.indexOf(clean[i + 2]) : -1;
          const b4 = i + 3 < clean.length ? lut.indexOf(clean[i + 3]) : -1;
          if (p < out.length) out[p++] = (b1 << 2) | (b2 >> 4);
          if (b3 !== -1 && b3 !== 64 && p < out.length) out[p++] = ((b2 & 0xf) << 4) | (b3 >> 2);
          if (b4 !== -1 && b4 !== 64 && p < out.length) out[p++] = ((b3 & 0x3) << 6) | b4;
        }
        return out;
      }
      throw new TypeError("Unsupported encoding: " + encoding);
    }

    static alloc(size, fill, encoding) {
      const b = new Buffer(size);
      if (fill !== undefined) {
        if (typeof fill === "number") b.fill(fill);
        else if (typeof fill === "string") {
          const f = Buffer.from(fill, encoding || "utf-8");
          for (let i = 0; i < b.length; i++) b[i] = f[i % f.length];
        }
      }
      return b;
    }

    static allocUnsafe(size) { return new Buffer(size); }
    static byteLength(value, encoding) {
      if (value instanceof Uint8Array) return value.byteLength;
      if (typeof value === "string") {
        return Buffer.from(value, encoding || "utf-8").length;
      }
      return value.length || 0;
    }
    static isBuffer(b) { return b instanceof Buffer; }
    static isEncoding(s) {
      return ["utf8", "utf-8", "ascii", "latin1", "binary", "hex", "base64"]
        .includes(String(s).toLowerCase());
    }
    static concat(list, totalLength) {
      let len = totalLength;
      if (len === undefined) {
        len = 0;
        for (const b of list) len += b.length;
      }
      const out = Buffer.alloc(len);
      let off = 0;
      for (const b of list) {
        if (off >= len) break;
        const take = Math.min(b.length, len - off);
        out.set(b.subarray(0, take), off);
        off += take;
      }
      return out;
    }

    toString(encoding, start, end) {
      encoding = (encoding || "utf-8").toLowerCase();
      const view = this.subarray(start || 0, end !== undefined ? end : this.length);
      if (encoding === "utf8" || encoding === "utf-8") {
        return new TextDecoder("utf-8").decode(view);
      }
      if (encoding === "ascii" || encoding === "latin1" || encoding === "binary") {
        let s = "";
        for (let i = 0; i < view.length; i++) s += String.fromCharCode(view[i]);
        return s;
      }
      if (encoding === "hex") {
        let s = "";
        for (let i = 0; i < view.length; i++) {
          const h = view[i].toString(16);
          s += h.length === 1 ? "0" + h : h;
        }
        return s;
      }
      if (encoding === "base64") {
        const lut = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let s = "";
        let i = 0;
        for (; i + 2 < view.length; i += 3) {
          s += lut[view[i] >> 2];
          s += lut[((view[i] & 0x3) << 4) | (view[i + 1] >> 4)];
          s += lut[((view[i + 1] & 0xf) << 2) | (view[i + 2] >> 6)];
          s += lut[view[i + 2] & 0x3f];
        }
        if (i < view.length) {
          s += lut[view[i] >> 2];
          if (i + 1 === view.length) {
            s += lut[(view[i] & 0x3) << 4];
            s += "==";
          } else {
            s += lut[((view[i] & 0x3) << 4) | (view[i + 1] >> 4)];
            s += lut[(view[i + 1] & 0xf) << 2];
            s += "=";
          }
        }
        return s;
      }
      throw new TypeError("Unsupported encoding: " + encoding);
    }

    equals(other) {
      if (!(other instanceof Uint8Array)) return false;
      if (this.length !== other.length) return false;
      for (let i = 0; i < this.length; i++) if (this[i] !== other[i]) return false;
      return true;
    }

    toJSON() {
      return { type: "Buffer", data: Array.from(this) };
    }

    write(string, offset, length, encoding) {
      offset = offset || 0;
      if (typeof length === "string") { encoding = length; length = undefined; }
      const b = Buffer.from(string, encoding || "utf-8");
      const n = Math.min(length === undefined ? b.length : length, this.length - offset);
      for (let i = 0; i < n; i++) this[offset + i] = b[i];
      return n;
    }

    slice(start, end) { return Buffer.from(this.subarray(start, end)); }
    copy(target, targetStart, sourceStart, sourceEnd) {
      targetStart = targetStart || 0;
      sourceStart = sourceStart || 0;
      sourceEnd = sourceEnd === undefined ? this.length : sourceEnd;
      const len = Math.min(target.length - targetStart, sourceEnd - sourceStart);
      for (let i = 0; i < len; i++) target[targetStart + i] = this[sourceStart + i];
      return len;
    }
  }
  g.Buffer = Buffer;
  // Common alias used by base64/hex/etc shims.
  if (typeof g.atob === "undefined") {
    g.atob = (s) => Buffer.from(s, "base64").toString("latin1");
  }
  if (typeof g.btoa === "undefined") {
    g.btoa = (s) => Buffer.from(s, "latin1").toString("base64");
  }
})(globalThis);
"#;
