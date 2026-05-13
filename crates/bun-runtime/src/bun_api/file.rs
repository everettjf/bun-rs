//! `Bun.file(path)` — Blob-like wrapper backed by a filesystem path.
//!
//! The returned object exposes:
//!   - `.text()` → Promise<string>
//!   - `.json()` → Promise<any>
//!   - `.bytes()` → Promise<Uint8Array>
//!   - `.arrayBuffer()` → Promise<ArrayBuffer>
//!   - `.exists()` → Promise<boolean>
//!   - `.size` (number, lazily computed)
//!   - `.name` (string, just the path)
//!
//! Plus `Bun.write(destination, data)` for the common write-file case.

use std::fs;

use bun_jsc::{Callback, Context, Value};

pub fn install(ctx: &Context, bun: &bun_jsc::Object<'_>) {
    bind(ctx, bun, "file", |args| {
        let path = args.get(0).to_string();
        let ctx = args.context();
        let v = ctx.eval("({})", Some("[Bun.file]")).unwrap();
        let obj = v.to_object().unwrap();
        obj.set_property("name", &Value::new_string(ctx, &path)).unwrap();

        // size: lazy property would be ideal; for MVP we read metadata at
        // construction time. Missing files report size 0 (matches Bun).
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        obj.set_property("size", &Value::new_number(ctx, size as f64))
            .unwrap();
        obj.set_property("type", &Value::new_string(ctx, &guess_type(&path)))
            .unwrap();

        // Methods. Each captures `path` by clone.
        let p_text = path.clone();
        bind(ctx, &obj, "text", move |args| {
            let bytes = fs::read(&p_text).map_err(|e| e.to_string())?;
            let s = String::from_utf8_lossy(&bytes).into_owned();
            Ok(Value::new_string(args.context(), &s))
        });

        let p_json = path.clone();
        bind(ctx, &obj, "json", move |args| {
            let bytes = fs::read(&p_json).map_err(|e| e.to_string())?;
            let s = String::from_utf8_lossy(&bytes).into_owned();
            // Use JS-side JSON.parse so we get a proper JS object back.
            let ctx = args.context();
            let parser = ctx
                .eval("(s) => JSON.parse(s)", Some("[Bun.file.json]"))
                .unwrap()
                .to_object()
                .map_err(|e| e.to_string())?;
            parser
                .call(None, &[Value::new_string(ctx, &s)])
                .map_err(|e| e.to_string())
        });

        let p_bytes = path.clone();
        bind(ctx, &obj, "bytes", move |args| {
            let bytes = fs::read(&p_bytes).map_err(|e| e.to_string())?;
            // Zero-copy Uint8Array.
            Ok(Value::new_uint8_array(args.context(), bytes))
        });

        let p_ab = path.clone();
        bind(ctx, &obj, "arrayBuffer", move |args| {
            let bytes = fs::read(&p_ab).map_err(|e| e.to_string())?;
            // Build the typed array first, then access its `.buffer` so we
            // hand back an actual ArrayBuffer.
            let u8 = Value::new_uint8_array(args.context(), bytes);
            let getter = args
                .context()
                .eval("(u) => u.buffer", Some("[arrayBuffer]"))
                .unwrap()
                .to_object()
                .map_err(|e| e.to_string())?;
            getter.call(None, &[u8]).map_err(|e| e.to_string())
        });

        let p_exists = path.clone();
        bind(ctx, &obj, "exists", move |args| {
            // Bun's Bun.file().exists() reports true only for regular files
            // (and named pipes, sockets, etc), NOT for directories.
            let exists = std::fs::metadata(&p_exists)
                .map(|m| !m.is_dir())
                .unwrap_or(false);
            Ok(Value::new_bool(args.context(), exists))
        });

        // .toString() returns the file path — used by Bun.$ shell template
        // and other String(file) coercions.
        let p_str = path.clone();
        bind(ctx, &obj, "toString", move |args| {
            Ok(Value::new_string(args.context(), &p_str))
        });

        // .slice(start, end?, type?) — returns a new Bun.file-like object
        // whose body methods read the underlying file then slice the bytes.
        let p_slice = path.clone();
        bind(ctx, &obj, "slice", move |args| {
            let ctx = args.context();
            let start = if args.len() >= 1 { args.get(0).to_number() as i64 } else { 0 };
            let end_opt = if args.len() >= 2 && !args.get(1).is_undefined() && !args.get(1).is_null() {
                Some(args.get(1).to_number() as i64)
            } else {
                None
            };
            let mime = if args.len() >= 3 { args.get(2).to_string() } else { String::new() };
            let total = std::fs::metadata(&p_slice).map(|m| m.len() as i64).unwrap_or(0);
            let s = if start < 0 { (total + start).max(0) } else { start.min(total) };
            let e = end_opt.map(|x| if x < 0 { (total + x).max(0) } else { x.min(total) }).unwrap_or(total);
            let s_u = s as u64;
            let e_u = if e > s { e as u64 } else { s_u };
            let slice_len = (e_u - s_u) as usize;

            let sub_v = ctx.eval("({})", Some("[Bun.file.slice]")).map_err(|e| e.to_string())?;
            let sub = sub_v.to_object().map_err(|e| e.to_string())?;
            sub.set_property("size", &Value::new_number(ctx, slice_len as f64)).ok();
            sub.set_property("type", &Value::new_string(ctx, &mime)).ok();
            sub.set_property("name", &Value::new_string(ctx, &p_slice)).ok();

            let p_text = p_slice.clone();
            let (s_t, e_t) = (s_u, e_u);
            bind(ctx, &sub, "text", move |args| {
                let bytes = read_slice(&p_text, s_t, e_t).map_err(|e| e)?;
                let s = String::from_utf8_lossy(&bytes).into_owned();
                Ok(Value::new_string(args.context(), &s))
            });
            let p_bytes = p_slice.clone();
            let (s_b, e_b) = (s_u, e_u);
            bind(ctx, &sub, "bytes", move |args| {
                let bytes = read_slice(&p_bytes, s_b, e_b).map_err(|e| e)?;
                Ok(Value::new_uint8_array(args.context(), bytes))
            });
            let p_ab = p_slice.clone();
            let (s_a, e_a) = (s_u, e_u);
            bind(ctx, &sub, "arrayBuffer", move |args| {
                let bytes = read_slice(&p_ab, s_a, e_a).map_err(|e| e)?;
                let u8 = Value::new_uint8_array(args.context(), bytes);
                let getter = args
                    .context()
                    .eval("(u) => u.buffer", Some("[ab]"))
                    .unwrap()
                    .to_object()
                    .map_err(|e| e.to_string())?;
                getter.call(None, &[u8]).map_err(|e| e.to_string())
            });
            let p_json = p_slice.clone();
            let (s_j, e_j) = (s_u, e_u);
            bind(ctx, &sub, "json", move |args| {
                let bytes = read_slice(&p_json, s_j, e_j).map_err(|e| e)?;
                let s = String::from_utf8_lossy(&bytes).into_owned();
                let ctx = args.context();
                let parser = ctx
                    .eval("(s) => JSON.parse(s)", Some("[json]"))
                    .unwrap()
                    .to_object()
                    .map_err(|e| e.to_string())?;
                parser.call(None, &[Value::new_string(ctx, &s)]).map_err(|e| e.to_string())
            });
            Ok(sub_v)
        });

        Ok(v)
    });

    bind(ctx, bun, "write", |args| {
        let dest = args.get(0).to_string();
        let v = args.get(1);
        let bytes_owned;
        let bytes: &[u8] = if let Some(slice) = v.typed_array_bytes() {
            bytes_owned = slice.to_vec();
            &bytes_owned
        } else {
            bytes_owned = v.to_string().into_bytes();
            &bytes_owned
        };
        fs::write(&dest, bytes).map_err(|e| e.to_string())?;
        Ok(Value::new_number(args.context(), bytes.len() as f64))
    });
}

fn read_slice(path: &str, start: u64, end: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let len = end.saturating_sub(start) as usize;
    let mut buf = vec![0u8; len];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    Ok(buf)
}

fn guess_type(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.ends_with(".json") { return "application/json".into(); }
    if lower.ends_with(".html") || lower.ends_with(".htm") { return "text/html".into(); }
    if lower.ends_with(".css") { return "text/css".into(); }
    if lower.ends_with(".js") || lower.ends_with(".mjs") { return "application/javascript".into(); }
    if lower.ends_with(".ts") || lower.ends_with(".tsx") { return "application/typescript".into(); }
    if lower.ends_with(".png") { return "image/png".into(); }
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { return "image/jpeg".into(); }
    if lower.ends_with(".svg") { return "image/svg+xml".into(); }
    if lower.ends_with(".txt") || lower.ends_with(".md") { return "text/plain;charset=utf-8".into(); }
    "application/octet-stream".into()
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
