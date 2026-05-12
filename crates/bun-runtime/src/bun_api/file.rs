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
            let ctx = args.context();
            // Build Uint8Array via JS — JSC has Uint8Array as a builtin.
            // We feed bytes through an array literal; this is OK for MVP
            // sizes. True zero-copy would use JSObjectMakeTypedArrayWithBytesNoCopy.
            let arr_lit = format!(
                "new Uint8Array([{}])",
                bytes
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            Ok(ctx.eval(&arr_lit, Some("[Bun.file.bytes]")).unwrap())
        });

        let p_ab = path.clone();
        bind(ctx, &obj, "arrayBuffer", move |args| {
            let bytes = fs::read(&p_ab).map_err(|e| e.to_string())?;
            let ctx = args.context();
            let arr_lit = format!(
                "new Uint8Array([{}]).buffer",
                bytes
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            Ok(ctx.eval(&arr_lit, Some("[Bun.file.ab]")).unwrap())
        });

        let p_exists = path.clone();
        bind(ctx, &obj, "exists", move |args| {
            Ok(Value::new_bool(
                args.context(),
                std::path::Path::new(&p_exists).exists(),
            ))
        });

        Ok(v)
    });

    bind(ctx, bun, "write", |args| {
        let dest = args.get(0).to_string();
        let data = args.get(1).to_string();
        fs::write(&dest, data.as_bytes()).map_err(|e| e.to_string())?;
        Ok(Value::new_number(args.context(), data.len() as f64))
    });
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
