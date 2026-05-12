//! `node:zlib` — deflate / inflate / gzip / gunzip via flate2.

use std::io::{Read, Write};

use bun_jsc::{Callback, Context, Value};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::Compression;

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:zlib]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    bind_codec(ctx, &exports, "deflateSync", encode_zlib);
    bind_codec(ctx, &exports, "inflateSync", decode_zlib);
    bind_codec(ctx, &exports, "deflateRawSync", encode_raw);
    bind_codec(ctx, &exports, "inflateRawSync", decode_raw);
    bind_codec(ctx, &exports, "gzipSync", encode_gzip);
    bind_codec(ctx, &exports, "gunzipSync", decode_gzip);

    // Async variants — Node uses callbacks. For simplicity we run sync and
    // call the callback on the next microtask.
    bind_codec_async(ctx, &exports, "deflate", encode_zlib);
    bind_codec_async(ctx, &exports, "inflate", decode_zlib);
    bind_codec_async(ctx, &exports, "deflateRaw", encode_raw);
    bind_codec_async(ctx, &exports, "inflateRaw", decode_raw);
    bind_codec_async(ctx, &exports, "gzip", encode_gzip);
    bind_codec_async(ctx, &exports, "gunzip", decode_gzip);

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn encode_zlib(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(input).map_err(|e| e.to_string())?;
    e.finish().map_err(|e| e.to_string())
}

fn decode_zlib(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = ZlibDecoder::new(input);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn encode_raw(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
    e.write_all(input).map_err(|e| e.to_string())?;
    e.finish().map_err(|e| e.to_string())
}

fn decode_raw(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = DeflateDecoder::new(input);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn encode_gzip(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(input).map_err(|e| e.to_string())?;
    e.finish().map_err(|e| e.to_string())
}

fn decode_gzip(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = GzDecoder::new(input);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn bind_codec(
    ctx: &Context,
    obj: &bun_jsc::Object<'_>,
    name: &str,
    op: fn(&[u8]) -> Result<Vec<u8>, String>,
) {
    let cb = Callback::new(ctx, name, move |args| {
        let v = args.get(0);
        let bytes: Vec<u8> = match v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => v.to_string().into_bytes(),
        };
        let out = op(&bytes)?;
        Ok(crate::buffer::buffer_from_bytes(args.context(), out))
    });
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}

fn bind_codec_async(
    ctx: &Context,
    obj: &bun_jsc::Object<'_>,
    name: &str,
    op: fn(&[u8]) -> Result<Vec<u8>, String>,
) {
    let name_owned = name.to_string();
    let cb = Callback::new(ctx, name, move |args| {
        let v = args.get(0);
        let bytes: Vec<u8> = match v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => v.to_string().into_bytes(),
        };
        let cb_v = args.get(args.len() - 1);
        if !cb_v.is_object() {
            return Err(format!("{}: missing callback", name_owned));
        }
        let cb_obj = cb_v.to_object().map_err(|e| e.to_string())?;
        let result = op(&bytes);
        let ctx = args.context();
        match result {
            Ok(out) => {
                let null = Value::new_null(ctx);
                let buf = crate::buffer::buffer_from_bytes(ctx, out);
                let _ = cb_obj.call(None, &[null, buf]);
            }
            Err(e) => {
                let err = Value::new_string(ctx, &e);
                let _ = cb_obj.call(None, &[err]);
            }
        }
        Ok(Value::new_undefined(ctx))
    });
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
