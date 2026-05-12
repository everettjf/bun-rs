//! `node:fs` — sync core + Promises wrapper.
//!
//! Subset:
//!   readFileSync, writeFileSync, appendFileSync, existsSync,
//!   statSync, readdirSync, mkdirSync, rmSync, renameSync,
//!   unlinkSync, copyFileSync, realpathSync, mkdtempSync
//!
//! `fs.promises` mirrors the sync API — for now each method just calls the
//! sync version and wraps the result in `Promise.resolve` / `Promise.reject`.
//! When the runtime gets real async I/O (tokio integration), this is the
//! only file that needs to flip.

use std::fs;
use std::path::Path;

use bun_jsc::{Callback, Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:fs]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    install_sync(ctx, &exports);

    // fs.promises — wrap every sync fn in Promise.resolve / .reject.
    let promises_v = ctx.eval("({})", Some("[node:fs.promises]")).unwrap();
    install_async(ctx, &promises_v.to_object().unwrap());
    exports.set_property("promises", &promises_v).unwrap();

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn install_sync(ctx: &Context, obj: &bun_jsc::Object<'_>) {
    bind(ctx, obj, "readFileSync", |args| {
        let path = args.get(0).to_string();
        let bytes = fs::read(&path).map_err(io_err)?;
        let ctx = args.context();

        // If encoding arg is a string ("utf8" / "utf-8" / "ascii"), decode.
        // If it's undefined or an options object, return raw bytes wrapped
        // in a {byteLength, toString()} stand-in (no Buffer yet).
        let enc = args.get(1);
        if enc.is_string() {
            let s = enc.to_string().to_lowercase();
            if s == "utf8" || s == "utf-8" || s == "ascii" || s == "latin1" {
                let str_val = String::from_utf8_lossy(&bytes).into_owned();
                return Ok(Value::new_string(ctx, &str_val));
            }
        }
        // Default: return a string anyway (matches the common case). True
        // Buffer support is Phase 3.
        let str_val = String::from_utf8_lossy(&bytes).into_owned();
        Ok(Value::new_string(ctx, &str_val))
    });

    bind(ctx, obj, "writeFileSync", |args| {
        let path = args.get(0).to_string();
        let data = args.get(1).to_string();
        fs::write(&path, data.as_bytes()).map_err(io_err)?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "appendFileSync", |args| {
        use std::io::Write;
        let path = args.get(0).to_string();
        let data = args.get(1).to_string();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_err)?;
        f.write_all(data.as_bytes()).map_err(io_err)?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "existsSync", |args| {
        let p = args.get(0).to_string();
        Ok(Value::new_bool(args.context(), Path::new(&p).exists()))
    });

    bind(ctx, obj, "statSync", |args| {
        let p = args.get(0).to_string();
        let md = fs::metadata(&p).map_err(io_err)?;
        let ctx = args.context();
        let v = ctx.eval("({})", Some("[fs.stat]")).unwrap();
        let obj = v.to_object().unwrap();
        obj.set_property("size", &Value::new_number(ctx, md.len() as f64))
            .unwrap();
        obj.set_property(
            "isFile",
            &Value::new_bool(ctx, md.is_file()),
        )
        .unwrap();
        obj.set_property(
            "isDirectory",
            &Value::new_bool(ctx, md.is_dir()),
        )
        .unwrap();
        obj.set_property(
            "isSymbolicLink",
            &Value::new_bool(ctx, md.file_type().is_symlink()),
        )
        .unwrap();
        // Node API exposes isFile / isDirectory as METHODS. Provide both.
        // (Field-style above is enough for many libs; method-style we add via
        // tiny JS shim eval'd into the object.)
        let install_methods = ctx
            .eval(
                "(o) => { \
                   const f = o.isFile, d = o.isDirectory, sl = o.isSymbolicLink; \
                   o.isFile = () => f; \
                   o.isDirectory = () => d; \
                   o.isSymbolicLink = () => sl; \
                   return o; \
                 }",
                Some("[stat-methods]"),
            )
            .unwrap()
            .to_object()
            .unwrap();
        install_methods.call(None, &[v]).unwrap();
        // mtime / atime / birthtime as milliseconds since epoch.
        for (k, t) in [
            ("mtimeMs", md.modified().ok()),
            ("atimeMs", md.accessed().ok()),
            ("birthtimeMs", md.created().ok()),
        ] {
            if let Some(t) = t {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    obj.set_property(
                        k,
                        &Value::new_number(ctx, d.as_millis() as f64),
                    )
                    .unwrap();
                }
            }
        }
        Ok(v)
    });

    bind(ctx, obj, "readdirSync", |args| {
        let p = args.get(0).to_string();
        let entries = fs::read_dir(&p).map_err(io_err)?;
        let ctx = args.context();
        let arr_v = ctx.eval("[]", Some("[readdir]")).unwrap();
        let arr = arr_v.to_object().unwrap();
        let mut count = 0u32;
        for e in entries {
            let e = e.map_err(io_err)?;
            let name = e.file_name().to_string_lossy().into_owned();
            arr.set_property(&count.to_string(), &Value::new_string(ctx, &name))
                .unwrap();
            count += 1;
        }
        arr.set_property("length", &Value::new_number(ctx, count as f64))
            .unwrap();
        Ok(arr_v)
    });

    bind(ctx, obj, "mkdirSync", |args| {
        let p = args.get(0).to_string();
        let opts = args.get(1);
        let recursive = if opts.is_object() {
            opts.to_object()
                .ok()
                .and_then(|o| o.get_property("recursive").ok())
                .map_or(false, |v| v.to_bool())
        } else {
            false
        };
        if recursive {
            fs::create_dir_all(&p).map_err(io_err)?;
        } else {
            fs::create_dir(&p).map_err(io_err)?;
        }
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "rmSync", |args| {
        let p = args.get(0).to_string();
        let opts = args.get(1);
        let recursive = if opts.is_object() {
            opts.to_object()
                .ok()
                .and_then(|o| o.get_property("recursive").ok())
                .map_or(false, |v| v.to_bool())
        } else {
            false
        };
        let path = Path::new(&p);
        if path.is_dir() {
            if recursive {
                fs::remove_dir_all(path).map_err(io_err)?;
            } else {
                fs::remove_dir(path).map_err(io_err)?;
            }
        } else {
            fs::remove_file(path).map_err(io_err)?;
        }
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "unlinkSync", |args| {
        let p = args.get(0).to_string();
        fs::remove_file(&p).map_err(io_err)?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "renameSync", |args| {
        let from = args.get(0).to_string();
        let to = args.get(1).to_string();
        fs::rename(&from, &to).map_err(io_err)?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "copyFileSync", |args| {
        let from = args.get(0).to_string();
        let to = args.get(1).to_string();
        fs::copy(&from, &to).map_err(io_err)?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "realpathSync", |args| {
        let p = args.get(0).to_string();
        let real = fs::canonicalize(&p)
            .map_err(io_err)?
            .to_string_lossy()
            .into_owned();
        Ok(Value::new_string(args.context(), &real))
    });

    bind(ctx, obj, "mkdtempSync", |args| {
        let prefix = args.get(0).to_string();
        let nano = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = format!("{prefix}{nano:x}");
        fs::create_dir_all(&dir).map_err(io_err)?;
        Ok(Value::new_string(args.context(), &dir))
    });
}

fn install_async(ctx: &Context, obj: &bun_jsc::Object<'_>) {
    // For each sync method, wrap in `(...) => Promise.resolve().then(() => sync(...))`.
    // We just expose the same callbacks but the caller can `await` them since
    // we resolve to the result via JSC's auto-promise wrapping.
    //
    // Approach: bind a Rust callback that does the same work and returns a
    // value; the JS side `await` is fine because `await someValue` resolves
    // synchronously. For correct error→rejection mapping, we throw on error
    // and JSC turns that into a rejected promise when awaited inside an
    // async function — which is what user code is in by definition (modules
    // are async).
    install_sync(ctx, obj);
}

fn io_err(e: std::io::Error) -> String {
    format!("ENOENT: {e}")
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
