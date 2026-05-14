//! `node:crypto` — subset for hashing, HMAC, random data, timing-safe compare.

use bun_jsc::{Callback, Context, Value};
use digest::Digest;
use hmac::Mac;
use rand::RngCore;
use std::cell::RefCell;
use std::rc::Rc;

type Md5 = md5::Md5;
type Sha1 = sha1::Sha1;
type Sha256 = sha2::Sha256;
type Sha384 = sha2::Sha384;
type Sha512 = sha2::Sha512;

/// Enum dispatch over the supported algorithms. Generic `Digest` types don't
/// play nicely with `Box<dyn Trait>` for our use case, so we list each kind.
enum Hasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
}

impl Hasher {
    fn new(alg: &str) -> Option<Self> {
        Some(match alg.to_lowercase().as_str() {
            "md5" => Hasher::Md5(Md5::new()),
            "sha1" => Hasher::Sha1(Sha1::new()),
            "sha256" => Hasher::Sha256(Sha256::new()),
            "sha384" => Hasher::Sha384(Sha384::new()),
            "sha512" => Hasher::Sha512(Sha512::new()),
            _ => return None,
        })
    }
    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Md5(h) => h.update(data),
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
            Hasher::Sha384(h) => h.update(data),
            Hasher::Sha512(h) => h.update(data),
        }
    }
    fn finalize(self) -> Vec<u8> {
        match self {
            Hasher::Md5(h) => h.finalize().to_vec(),
            Hasher::Sha1(h) => h.finalize().to_vec(),
            Hasher::Sha256(h) => h.finalize().to_vec(),
            Hasher::Sha384(h) => h.finalize().to_vec(),
            Hasher::Sha512(h) => h.finalize().to_vec(),
        }
    }
}

enum Hmacker {
    Sha1(hmac::Hmac<Sha1>),
    Sha256(hmac::Hmac<Sha256>),
    Sha384(hmac::Hmac<Sha384>),
    Sha512(hmac::Hmac<Sha512>),
    Md5(hmac::Hmac<Md5>),
}

impl Hmacker {
    fn new(alg: &str, key: &[u8]) -> Option<Self> {
        Some(match alg.to_lowercase().as_str() {
            "sha1" => Hmacker::Sha1(hmac::Hmac::<Sha1>::new_from_slice(key).ok()?),
            "sha256" => Hmacker::Sha256(hmac::Hmac::<Sha256>::new_from_slice(key).ok()?),
            "sha384" => Hmacker::Sha384(hmac::Hmac::<Sha384>::new_from_slice(key).ok()?),
            "sha512" => Hmacker::Sha512(hmac::Hmac::<Sha512>::new_from_slice(key).ok()?),
            "md5" => Hmacker::Md5(hmac::Hmac::<Md5>::new_from_slice(key).ok()?),
            _ => return None,
        })
    }
    fn update(&mut self, data: &[u8]) {
        match self {
            Hmacker::Sha1(h) => h.update(data),
            Hmacker::Sha256(h) => h.update(data),
            Hmacker::Sha384(h) => h.update(data),
            Hmacker::Sha512(h) => h.update(data),
            Hmacker::Md5(h) => h.update(data),
        }
    }
    fn finalize(self) -> Vec<u8> {
        match self {
            Hmacker::Sha1(h) => h.finalize().into_bytes().to_vec(),
            Hmacker::Sha256(h) => h.finalize().into_bytes().to_vec(),
            Hmacker::Sha384(h) => h.finalize().into_bytes().to_vec(),
            Hmacker::Sha512(h) => h.finalize().into_bytes().to_vec(),
            Hmacker::Md5(h) => h.finalize().into_bytes().to_vec(),
        }
    }
}

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:crypto]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    bind(ctx, &exports, "createHash", |args| {
        let alg = args.get(0).to_string();
        build_hash_object(args.context(), &alg)
    });

    bind(ctx, &exports, "createHmac", |args| {
        let alg = args.get(0).to_string();
        let key_v = args.get(1);
        let key: Vec<u8> = match key_v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => key_v.to_string().into_bytes(),
        };
        build_hmac_object(args.context(), &alg, &key)
    });

    bind(ctx, &exports, "randomBytes", |args| {
        let n = args.get(0).to_number() as usize;
        let mut buf = vec![0u8; n];
        rand::rng().fill_bytes(&mut buf);
        Ok(crate::buffer::buffer_from_bytes(args.context(), buf))
    });

    bind(ctx, &exports, "randomUUID", |args| {
        Ok(Value::new_string(
            args.context(),
            &uuid::Uuid::new_v4().to_string(),
        ))
    });

    bind(ctx, &exports, "randomInt", |args| {
        let (min, max) = if args.len() >= 2 {
            (
                args.get(0).to_number() as i64,
                args.get(1).to_number() as i64,
            )
        } else {
            (0, args.get(0).to_number() as i64)
        };
        if min >= max {
            return Err("randomInt: min must be < max".into());
        }
        use rand::Rng;
        let v = rand::rng().random_range(min..max);
        Ok(Value::new_number(args.context(), v as f64))
    });

    bind(ctx, &exports, "timingSafeEqual", |args| {
        let a = args
            .get(0)
            .typed_array_bytes()
            .map(|b| b.to_vec())
            .unwrap_or_default();
        let b = args
            .get(1)
            .typed_array_bytes()
            .map(|b| b.to_vec())
            .unwrap_or_default();
        if a.len() != b.len() {
            return Err("timingSafeEqual: inputs must have the same length".into());
        }
        let eq: bool = subtle::ConstantTimeEq::ct_eq(a.as_slice(), b.as_slice()).into();
        Ok(Value::new_bool(args.context(), eq))
    });

    bind(ctx, &exports, "getHashes", |args| {
        let ctx = args.context();
        let arr = ctx.eval("[]", Some("[hashes]")).unwrap();
        let obj = arr.to_object().unwrap();
        let hashes = ["md5", "sha1", "sha256", "sha384", "sha512"];
        for (i, name) in hashes.iter().enumerate() {
            obj.set_property(&i.to_string(), &Value::new_string(ctx, name))
                .unwrap();
        }
        obj.set_property("length", &Value::new_number(ctx, hashes.len() as f64))
            .unwrap();
        Ok(arr)
    });

    // crypto.getRandomValues — implemented in JS to use the typed-array
    // index setters (so we can write through Uint8Array/Uint32Array/etc).
    let _ = ctx.eval(
        r#"
        ((exports) => {
            exports.getRandomValues = function (arr) {
                if (!ArrayBuffer.isView(arr)) throw new TypeError("getRandomValues requires a TypedArray");
                const bytes = exports.randomBytes(arr.byteLength);
                const view = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
                for (let i = 0; i < bytes.byteLength; i++) view[i] = bytes[i];
                return arr;
            };
        })
        "#,
        Some("[node:crypto.getRandomValues]"),
    ).and_then(|f| f.to_object().and_then(|o| o.call(None, &[exports_v])));
    // webcrypto sub-namespace (Node exposes node:crypto.webcrypto with
    // randomUUID / getRandomValues / subtle).
    let _ = ctx.eval(
        r#"
        ((exports) => {
            exports.webcrypto = {
                randomUUID: exports.randomUUID,
                getRandomValues: exports.getRandomValues,
                subtle: globalThis.crypto && globalThis.crypto.subtle,
            };
            exports.constants = exports.constants || {};
        })
        "#,
        Some("[node:crypto.webcrypto]"),
    ).and_then(|f| f.to_object().and_then(|o| o.call(None, &[exports_v])));

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn build_hash_object<'ctx>(ctx: &'ctx Context, alg: &str) -> Result<Value<'ctx>, String> {
    let hasher = Hasher::new(alg).ok_or_else(|| format!("unknown hash: {alg}"))?;
    let cell: Rc<RefCell<Option<Hasher>>> = Rc::new(RefCell::new(Some(hasher)));

    let v = ctx.eval("({})", Some("[Hash]")).unwrap();
    let obj = v.to_object().unwrap();
    let v_raw = v.as_raw();

    let cell_u = cell.clone();
    bind(ctx, &obj, "update", move |args| {
        let mut g = cell_u.borrow_mut();
        let h = g.as_mut().ok_or("hash already finalized")?;
        let data_v = args.get(0);
        if let Some(bytes) = data_v.typed_array_bytes() {
            h.update(bytes);
        } else {
            h.update(data_v.to_string().as_bytes());
        }
        Ok(unsafe { Value::from_raw_public(args.context(), v_raw) })
    });

    let cell_d = cell.clone();
    bind(ctx, &obj, "digest", move |args| {
        let mut g = cell_d.borrow_mut();
        let h = g.take().ok_or("hash already finalized")?;
        let raw = h.finalize();
        let enc = args.get(0);
        let ctx = args.context();
        if enc.is_string() {
            return Ok(Value::new_string(
                ctx,
                &encode_bytes(&raw, &enc.to_string().to_lowercase())?,
            ));
        }
        Ok(crate::buffer::buffer_from_bytes(ctx, raw))
    });

    Ok(v)
}

fn build_hmac_object<'ctx>(
    ctx: &'ctx Context,
    alg: &str,
    key: &[u8],
) -> Result<Value<'ctx>, String> {
    let mac = Hmacker::new(alg, key).ok_or_else(|| format!("unknown hmac alg: {alg}"))?;
    let cell: Rc<RefCell<Option<Hmacker>>> = Rc::new(RefCell::new(Some(mac)));

    let v = ctx.eval("({})", Some("[Hmac]")).unwrap();
    let obj = v.to_object().unwrap();
    let v_raw = v.as_raw();

    let cell_u = cell.clone();
    bind(ctx, &obj, "update", move |args| {
        let mut g = cell_u.borrow_mut();
        let h = g.as_mut().ok_or("hmac already finalized")?;
        let data_v = args.get(0);
        if let Some(bytes) = data_v.typed_array_bytes() {
            h.update(bytes);
        } else {
            h.update(data_v.to_string().as_bytes());
        }
        Ok(unsafe { Value::from_raw_public(args.context(), v_raw) })
    });

    let cell_d = cell.clone();
    bind(ctx, &obj, "digest", move |args| {
        let mut g = cell_d.borrow_mut();
        let h = g.take().ok_or("hmac already finalized")?;
        let raw = h.finalize();
        let enc = args.get(0);
        let ctx = args.context();
        if enc.is_string() {
            return Ok(Value::new_string(
                ctx,
                &encode_bytes(&raw, &enc.to_string().to_lowercase())?,
            ));
        }
        Ok(crate::buffer::buffer_from_bytes(ctx, raw))
    });

    Ok(v)
}

fn encode_bytes(bytes: &[u8], enc: &str) -> Result<String, String> {
    match enc {
        "hex" => Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect()),
        "base64" => Ok(base64_encode(bytes)),
        "base64url" => Ok(base64_encode(bytes)
            .replace('+', "-")
            .replace('/', "_")
            .trim_end_matches('=')
            .to_string()),
        "latin1" | "binary" => Ok(bytes.iter().map(|&b| b as char).collect()),
        "utf8" | "utf-8" => Ok(String::from_utf8_lossy(bytes).into_owned()),
        other => Err(format!("unknown encoding: {other}")),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const LUT: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes[i + 1];
        let b2 = bytes[i + 2];
        out.push(LUT[(b0 >> 2) as usize] as char);
        out.push(LUT[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(LUT[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        out.push(LUT[(b2 & 0x3f) as usize] as char);
        i += 3;
    }
    if i < bytes.len() {
        let b0 = bytes[i];
        out.push(LUT[(b0 >> 2) as usize] as char);
        if i + 1 < bytes.len() {
            let b1 = bytes[i + 1];
            out.push(LUT[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            out.push(LUT[((b1 & 0x0f) << 2) as usize] as char);
            out.push('=');
        } else {
            out.push(LUT[((b0 & 0x03) << 4) as usize] as char);
            out.push('=');
            out.push('=');
        }
    }
    out
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
