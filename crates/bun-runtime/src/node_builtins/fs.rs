//! `node:fs` — sync core + true async `fs.promises`.
//!
//! Sync subset:
//!   readFileSync, writeFileSync, appendFileSync, existsSync,
//!   statSync, readdirSync, mkdirSync, rmSync, renameSync,
//!   unlinkSync, copyFileSync, realpathSync, mkdtempSync
//!
//! `fs.promises` uses `spawn_blocking` on the async runtime so the JS
//! thread isn't blocked while file I/O is in flight. Each method follows
//! the same shape: extract args sync → spawn tokio task → post a closure
//! back to JS that resolves/rejects the deferred Promise.

use std::fs;
use std::path::Path;

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;

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

        // Encoding arg may be a string OR an options object `{ encoding }`.
        let enc_arg = args.get(1);
        let encoding: Option<String> = if enc_arg.is_string() {
            Some(enc_arg.to_string())
        } else if enc_arg.is_object() {
            enc_arg
                .to_object()
                .ok()
                .and_then(|o| o.get_property("encoding").ok())
                .filter(|v| v.is_string())
                .map(|v| v.to_string())
        } else {
            None
        };

        if let Some(enc) = encoding {
            let s = enc.to_lowercase();
            if s == "utf8" || s == "utf-8" {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                return Ok(Value::new_string(ctx, &text));
            }
            if s == "ascii" || s == "latin1" || s == "binary" {
                let text: String = bytes.iter().map(|&b| b as char).collect();
                return Ok(Value::new_string(ctx, &text));
            }
            if s == "hex" {
                let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
                return Ok(Value::new_string(ctx, &hex));
            }
            // Unknown encoding — fall through to Buffer.
        }

        // No encoding (or unrecognized) → return a Buffer (zero-copy).
        Ok(crate::buffer::buffer_from_bytes(ctx, bytes))
    });

    bind(ctx, obj, "writeFileSync", |args| {
        let path = args.get(0).to_string();
        let v = args.get(1);
        let bytes_owned;
        let bytes: &[u8] = if let Some(slice) = v.typed_array_bytes() {
            bytes_owned = slice.to_vec();
            &bytes_owned
        } else {
            bytes_owned = v.to_string().into_bytes();
            &bytes_owned
        };
        fs::write(&path, bytes).map_err(io_err)?;
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
    // readFile(path, opts?) → Buffer | string
    bind(ctx, obj, "readFile", |args| {
        let path = args.get(0).to_string();
        let enc = extract_encoding(&args.get(1));
        promise_from_blocking(
            args.context(),
            move || fs::read(&path).map_err(io_err),
            move |ctx, bytes| match enc.as_deref() {
                Some("utf8") | Some("utf-8") => {
                    Value::new_string(ctx, &String::from_utf8_lossy(&bytes))
                }
                Some("hex") => Value::new_string(
                    ctx,
                    &bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
                ),
                Some("ascii") | Some("latin1") | Some("binary") => {
                    Value::new_string(ctx, &bytes.iter().map(|&b| b as char).collect::<String>())
                }
                _ => crate::buffer::buffer_from_bytes(ctx, bytes),
            },
        )
    });

    // writeFile(path, data, opts?) → undefined
    bind(ctx, obj, "writeFile", |args| {
        let path = args.get(0).to_string();
        let data_v = args.get(1);
        let bytes: Vec<u8> = match data_v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => data_v.to_string().into_bytes(),
        };
        promise_from_blocking(
            args.context(),
            move || fs::write(&path, &bytes).map_err(io_err),
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // appendFile(path, data) → undefined
    bind(ctx, obj, "appendFile", |args| {
        use std::io::Write;
        let path = args.get(0).to_string();
        let data_v = args.get(1);
        let bytes: Vec<u8> = match data_v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => data_v.to_string().into_bytes(),
        };
        promise_from_blocking(
            args.context(),
            move || {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(&bytes))
                    .map_err(io_err)
            },
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // unlink(path) → undefined
    bind(ctx, obj, "unlink", |args| {
        let path = args.get(0).to_string();
        promise_from_blocking(
            args.context(),
            move || fs::remove_file(&path).map_err(io_err),
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // mkdir(path, opts?) → undefined (returns path when recursive in Node)
    bind(ctx, obj, "mkdir", |args| {
        let path = args.get(0).to_string();
        let recursive = args
            .get(1)
            .to_object()
            .ok()
            .and_then(|o| o.get_property("recursive").ok())
            .map(|v| v.to_bool())
            .unwrap_or(false);
        promise_from_blocking(
            args.context(),
            move || {
                if recursive {
                    fs::create_dir_all(&path).map_err(io_err)
                } else {
                    fs::create_dir(&path).map_err(io_err)
                }
            },
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // rm(path, opts?)
    bind(ctx, obj, "rm", |args| {
        let path = args.get(0).to_string();
        let recursive = args
            .get(1)
            .to_object()
            .ok()
            .and_then(|o| o.get_property("recursive").ok())
            .map(|v| v.to_bool())
            .unwrap_or(false);
        promise_from_blocking(
            args.context(),
            move || {
                let p = Path::new(&path);
                if p.is_dir() {
                    if recursive {
                        fs::remove_dir_all(p).map_err(io_err)
                    } else {
                        fs::remove_dir(p).map_err(io_err)
                    }
                } else {
                    fs::remove_file(p).map_err(io_err)
                }
            },
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // readdir(path) → string[]
    bind(ctx, obj, "readdir", |args| {
        let path = args.get(0).to_string();
        promise_from_blocking(
            args.context(),
            move || {
                let mut out: Vec<String> = Vec::new();
                for e in fs::read_dir(&path).map_err(io_err)? {
                    let e = e.map_err(io_err)?;
                    out.push(e.file_name().to_string_lossy().into_owned());
                }
                Ok(out)
            },
            |ctx, names| {
                let arr_v = ctx.eval("[]", Some("[readdir]")).unwrap();
                let arr = arr_v.to_object().unwrap();
                for (i, n) in names.iter().enumerate() {
                    let _ = arr.set_property(&i.to_string(), &Value::new_string(ctx, n));
                }
                let _ = arr.set_property("length", &Value::new_number(ctx, names.len() as f64));
                arr_v
            },
        )
    });

    // stat(path) → { size, mtimeMs, isFile(), isDirectory() }
    bind(ctx, obj, "stat", |args| {
        let path = args.get(0).to_string();
        promise_from_blocking(
            args.context(),
            move || fs::metadata(&path).map_err(io_err),
            |ctx, md| stat_obj(ctx, md),
        )
    });

    // copyFile(src, dst)
    bind(ctx, obj, "copyFile", |args| {
        let src = args.get(0).to_string();
        let dst = args.get(1).to_string();
        promise_from_blocking(
            args.context(),
            move || fs::copy(&src, &dst).map(|_| ()).map_err(io_err),
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // rename(from, to)
    bind(ctx, obj, "rename", |args| {
        let from = args.get(0).to_string();
        let to = args.get(1).to_string();
        promise_from_blocking(
            args.context(),
            move || fs::rename(&from, &to).map_err(io_err),
            |ctx, _| Value::new_undefined(ctx),
        )
    });

    // realpath(path) → string
    bind(ctx, obj, "realpath", |args| {
        let path = args.get(0).to_string();
        promise_from_blocking(
            args.context(),
            move || {
                fs::canonicalize(&path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(io_err)
            },
            |ctx, s| Value::new_string(ctx, &s),
        )
    });
}

/// Generic helper: build a deferred Promise, run `work` on tokio's blocking
/// pool, then post `finish` back to the JS thread to resolve / reject.
fn promise_from_blocking<'ctx, T, W, F>(
    ctx: &'ctx Context,
    work: W,
    finish: F,
) -> Result<Value<'ctx>, String>
where
    T: Send + 'static,
    W: FnOnce() -> Result<T, String> + Send + 'static,
    F: for<'a> FnOnce(&'a Context, T) -> Value<'a> + Send + 'static,
{
    let mut resolve: sys::JSObjectRef = std::ptr::null_mut();
    let mut reject: sys::JSObjectRef = std::ptr::null_mut();
    let mut exc: sys::JSValueRef = std::ptr::null();
    let promise = unsafe {
        sys::JSObjectMakeDeferredPromise(
            ctx.as_raw(),
            &mut resolve as *mut _,
            &mut reject as *mut _,
            &mut exc,
        )
    };
    if !exc.is_null() {
        return Err("Promise construction failed".into());
    }
    unsafe {
        sys::JSValueProtect(ctx.as_raw(), resolve as sys::JSValueRef);
        sys::JSValueProtect(ctx.as_raw(), reject as sys::JSValueRef);
    }
    let resolve_id = resolve as usize;
    let reject_id = reject as usize;

    crate::async_rt::note_started();
    crate::async_rt::spawn(async move {
        let result = tokio::task::spawn_blocking(work).await;
        let result = match result {
            Ok(r) => r,
            Err(e) => Err(format!("spawn_blocking failed: {e}")),
        };
        crate::async_rt::post_to_js(move |ctx| {
            let ctx_raw = ctx.as_raw();
            let resolve = resolve_id as sys::JSObjectRef;
            let reject = reject_id as sys::JSObjectRef;
            match result {
                Ok(v) => {
                    let js = finish(ctx, v);
                    unsafe {
                        let resolve_obj = bun_jsc::Object::from_raw_for_runtime(ctx, resolve);
                        let _ = resolve_obj.call(None, &[js]);
                    }
                }
                Err(msg) => {
                    let err = Value::new_string(ctx, &msg);
                    unsafe {
                        let reject_obj = bun_jsc::Object::from_raw_for_runtime(ctx, reject);
                        let _ = reject_obj.call(None, &[err]);
                    }
                }
            }
            unsafe {
                sys::JSValueUnprotect(ctx_raw, resolve as sys::JSValueRef);
                sys::JSValueUnprotect(ctx_raw, reject as sys::JSValueRef);
            }
            crate::async_rt::note_finished();
        });
    });

    Ok(unsafe { Value::from_raw_public(ctx, promise as sys::JSValueRef) })
}

fn extract_encoding(v: &Value<'_>) -> Option<String> {
    if v.is_string() {
        return Some(v.to_string().to_lowercase());
    }
    if v.is_object() {
        return v
            .to_object()
            .ok()
            .and_then(|o| o.get_property("encoding").ok())
            .filter(|v| v.is_string())
            .map(|v| v.to_string().to_lowercase());
    }
    None
}

/// Convert std::fs::Metadata into a JS object with both fields and methods,
/// matching the (subset of) `Stats` Node exposes.
fn stat_obj<'a>(ctx: &'a Context, md: fs::Metadata) -> Value<'a> {
    let v = ctx.eval("({})", Some("[fs.stat]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("size", &Value::new_number(ctx, md.len() as f64))
        .unwrap();
    obj.set_property("isFile", &Value::new_bool(ctx, md.is_file()))
        .unwrap();
    obj.set_property("isDirectory", &Value::new_bool(ctx, md.is_dir()))
        .unwrap();
    obj.set_property(
        "isSymbolicLink",
        &Value::new_bool(ctx, md.file_type().is_symlink()),
    )
    .unwrap();
    let install_methods = ctx
        .eval(
            "(o) => { const f=o.isFile,d=o.isDirectory,sl=o.isSymbolicLink; \
              o.isFile=()=>f; o.isDirectory=()=>d; o.isSymbolicLink=()=>sl; return o; }",
            Some("[stat-methods]"),
        )
        .unwrap()
        .to_object()
        .unwrap();
    let _ = install_methods.call(None, &[v]);
    for (k, t) in [
        ("mtimeMs", md.modified().ok()),
        ("atimeMs", md.accessed().ok()),
        ("birthtimeMs", md.created().ok()),
    ] {
        if let Some(t) = t {
            if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                let _ = obj.set_property(k, &Value::new_number(ctx, d.as_millis() as f64));
            }
        }
    }
    v
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
