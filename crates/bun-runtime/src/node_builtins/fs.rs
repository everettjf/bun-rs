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
    install_streaming(ctx, &exports);
    install_fd_io(ctx, &exports);

    // fs.promises — wrap every sync fn in Promise.resolve / .reject.
    let promises_v = ctx.eval("({})", Some("[node:fs.promises]")).unwrap();
    install_async(ctx, &promises_v.to_object().unwrap());
    // open() in fs.promises returns a FileHandle stub.
    let _ = ctx.eval(
        r#"
        ((p, fs) => {
            p.open = async function(path, flags) {
                const fd = fs.openSync(path, flags || "r");
                return {
                    fd,
                    close: async () => fs.closeSync(fd),
                    [Symbol.asyncDispose]: async () => fs.closeSync(fd),
                    read: async (buffer, offset, length, position) => ({
                        bytesRead: fs.readSync(fd, buffer, offset, length, position),
                        buffer,
                    }),
                    write: async (buffer, offset, length, position) => ({
                        bytesWritten: fs.writeSync(fd, buffer, offset, length, position),
                        buffer,
                    }),
                    readFile: async (opts) => fs.readFileSync(path, opts),
                    writeFile: async (data, opts) => fs.writeFileSync(path, data, opts),
                    stat: async () => fs.statSync(path),
                    truncate: async (len) => fs.truncateSync(path, len),
                    sync: async () => fs.fsyncSync(fd),
                    chmod: async (mode) => fs.chmodSync(path, mode),
                    chown: async (uid, gid) => fs.chownSync(path, uid, gid),
                };
            };
            p.opendir = async (path) => fs.opendirSync ? fs.opendirSync(path) : { close: async () => {}, readSync: () => null };
            p.mkdtemp = async (prefix) => {
                const p2 = require("node:path");
                const os = require("node:os");
                const d = p2.join(os.tmpdir(), prefix + Math.random().toString(36).slice(2));
                fs.mkdirSync(d, { recursive: true });
                return d;
            };
        })
        "#,
        Some("[fs.promises.open]"),
    ).and_then(|f| f.to_object().and_then(|o| o.call(None, &[promises_v, exports_v])));
    exports.set_property("promises", &promises_v).unwrap();
    // fs.constants — file flags + permission bits.
    let constants_v = ctx
        .eval(
            r#"({
                O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2,
                O_CREAT: 0o100, O_EXCL: 0o200, O_NOCTTY: 0o400, O_TRUNC: 0o1000,
                O_APPEND: 0o2000, O_NONBLOCK: 0o4000, O_SYNC: 0o4010000,
                O_DIRECTORY: 0o20000, O_NOFOLLOW: 0o100000, O_CLOEXEC: 0o2000000,
                S_IFMT: 0o170000, S_IFREG: 0o100000, S_IFDIR: 0o040000,
                S_IFLNK: 0o120000, S_IFBLK: 0o060000, S_IFCHR: 0o020000,
                S_IFIFO: 0o010000, S_IFSOCK: 0o140000,
                S_IRWXU: 0o700, S_IRWXG: 0o070, S_IRWXO: 0o007,
                S_IRUSR: 0o400, S_IWUSR: 0o200, S_IXUSR: 0o100,
                S_IRGRP: 0o040, S_IWGRP: 0o020, S_IXGRP: 0o010,
                S_IROTH: 0o004, S_IWOTH: 0o002, S_IXOTH: 0o001,
                S_ISUID: 0o4000, S_ISGID: 0o2000, S_ISVTX: 0o1000,
                F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
                UV_FS_COPYFILE_EXCL: 1,
                UV_FS_COPYFILE_FICLONE: 2,
                UV_FS_COPYFILE_FICLONE_FORCE: 4,
                COPYFILE_EXCL: 1,
                COPYFILE_FICLONE: 2,
                COPYFILE_FICLONE_FORCE: 4,
            })"#,
            Some("[fs.constants]"),
        )
        .unwrap();
    exports.set_property("constants", &constants_v).unwrap();
    // Also expose the fd-IO + symlink/link/chown/... async wrappers on
    // fs.promises (they were added to `exports` via auto-wrap but not the
    // promises namespace).
    let _ = ctx.eval(
        r#"
        ((p, fs) => {
            const promisify = (name) => async (...args) => {
                if (typeof fs[name] === "function") return fs[name](...args);
                throw new Error("fs.promises." + name + " not implemented");
            };
            for (const k of ["symlink", "link", "chmod", "chown", "lchown", "lchmod", "utimes", "futimes", "readlink", "stat", "lstat", "rename", "rmdir", "rm", "mkdir", "unlink", "truncate", "access", "copyFile"]) {
                if (typeof p[k] !== "function" && typeof fs[k + "Sync"] === "function") {
                    p[k] = promisify(k + "Sync");
                } else if (typeof p[k] !== "function" && typeof fs[k] === "function") {
                    p[k] = promisify(k);
                }
            }
        })
        "#,
        Some("[fs.promises.copy-from-fs]"),
    ).and_then(|f| f.to_object().and_then(|o| o.call(None, &[promises_v, exports_v])));

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn install_fd_io(ctx: &Context, obj: &bun_jsc::Object<'_>) {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static FDS: OnceLock<Mutex<HashMap<i32, std::fs::File>>> = OnceLock::new();
    static NEXT_FD: OnceLock<Mutex<i32>> = OnceLock::new();
    fn fds() -> &'static Mutex<HashMap<i32, std::fs::File>> {
        FDS.get_or_init(|| Mutex::new(HashMap::new()))
    }
    fn next_fd() -> i32 {
        let m = NEXT_FD.get_or_init(|| Mutex::new(3));
        let mut g = m.lock().unwrap();
        let n = *g;
        *g += 1;
        n
    }

    bind(ctx, obj, "openSync", move |args| {
        let path = args.get(0).to_string();
        let flags = args.get(1).to_string();
        let mut opts = std::fs::OpenOptions::new();
        match flags.as_str() {
            "r" | "rs" => { opts.read(true); }
            "r+" | "rs+" => { opts.read(true).write(true); }
            "w" => { opts.write(true).create(true).truncate(true); }
            "wx" => { opts.write(true).create_new(true); }
            "w+" => { opts.read(true).write(true).create(true).truncate(true); }
            "wx+" => { opts.read(true).write(true).create_new(true); }
            "a" => { opts.append(true).create(true); }
            "ax" => { opts.append(true).create_new(true); }
            "a+" => { opts.read(true).append(true).create(true); }
            "ax+" => { opts.read(true).append(true).create_new(true); }
            _ => {
                // Numeric flags or unsupported — best effort.
                opts.read(true).write(true).create(true);
            }
        }
        let f = opts.open(&path).map_err(|e| e.to_string())?;
        let fd = next_fd();
        fds().lock().unwrap().insert(fd, f);
        Ok(Value::new_number(args.context(), fd as f64))
    });

    bind(ctx, obj, "closeSync", move |args| {
        let fd = args.get(0).to_number() as i32;
        fds().lock().unwrap().remove(&fd);
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "readSync", move |args| {
        // readSync(fd, buffer, offset, length, position) → bytesRead.
        let fd = args.get(0).to_number() as i32;
        let buf_v = args.get(1);
        let mut fdmap = fds().lock().unwrap();
        let file = fdmap.get_mut(&fd).ok_or("EBADF")?;
        let offset = if args.len() >= 3 { args.get(2).to_number() as usize } else { 0 };
        let length = if args.len() >= 4 { args.get(3).to_number() as usize } else { 0 };
        let position = if args.len() >= 5 {
            let p = args.get(4);
            if p.is_null() || p.is_undefined() { None } else { Some(p.to_number() as u64) }
        } else { None };

        let buf_bytes = buf_v.typed_array_bytes().ok_or("readSync: buffer must be a TypedArray")?;
        let buf_ptr = buf_bytes.as_ptr();
        let buf_len = buf_bytes.len();
        let end = offset.saturating_add(length).min(buf_len);
        if end < offset { return Err("read range out of bounds".into()); }
        let actual = end - offset;

        if let Some(pos) = position {
            file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        }
        // SAFETY: we know buf_bytes is the underlying typed-array storage.
        let n = unsafe {
            let slice = std::slice::from_raw_parts_mut(buf_ptr.add(offset) as *mut u8, actual);
            file.read(slice).map_err(|e| e.to_string())?
        };
        Ok(Value::new_number(args.context(), n as f64))
    });

    bind(ctx, obj, "writeSync", move |args| {
        // writeSync(fd, buffer | string [, offset, length, position])
        let fd = args.get(0).to_number() as i32;
        let mut fdmap = fds().lock().unwrap();
        let file = fdmap.get_mut(&fd).ok_or("EBADF")?;
        let data_v = args.get(1);
        let bytes = if let Some(b) = data_v.typed_array_bytes() {
            b.to_vec()
        } else {
            data_v.to_string().into_bytes()
        };
        let n = file.write(&bytes).map_err(|e| e.to_string())?;
        Ok(Value::new_number(args.context(), n as f64))
    });

    bind(ctx, obj, "fstatSync", move |args| {
        let fd = args.get(0).to_number() as i32;
        let fdmap = fds().lock().unwrap();
        let file = fdmap.get(&fd).ok_or("EBADF")?;
        let md = file.metadata().map_err(|e| e.to_string())?;
        let ctx = args.context();
        let stat_v = ctx.eval("({})", Some("[fstat]")).map_err(|e| e.to_string())?;
        let stat = stat_v.to_object().map_err(|e| e.to_string())?;
        stat.set_property("size", &Value::new_number(ctx, md.len() as f64)).ok();
        stat.set_property("isFile", &ctx.eval(if md.is_file() { "() => true" } else { "() => false" }, None).unwrap()).ok();
        stat.set_property("isDirectory", &ctx.eval(if md.is_dir() { "() => true" } else { "() => false" }, None).unwrap()).ok();
        stat.set_property("mode", &Value::new_number(ctx, 0.0)).ok();
        stat.set_property("uid", &Value::new_number(ctx, 0.0)).ok();
        stat.set_property("gid", &Value::new_number(ctx, 0.0)).ok();
        stat.set_property("blksize", &Value::new_number(ctx, 4096.0)).ok();
        stat.set_property("blocks", &Value::new_number(ctx, 0.0)).ok();
        Ok(stat_v)
    });

    bind(ctx, obj, "fsyncSync", |args| {
        // No-op (we don't track per-fd flush state).
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "ftruncateSync", move |args| {
        let fd = args.get(0).to_number() as i32;
        let len = args.get(1).to_number() as u64;
        let fdmap = fds().lock().unwrap();
        let file = fdmap.get(&fd).ok_or("EBADF")?;
        file.set_len(len).map_err(|e| e.to_string())?;
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, obj, "symlinkSync", |args| {
        let target = args.get(0).to_string();
        let link = args.get(1).to_string();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        return Err("symlinkSync not supported on Windows in bun-rs".into());
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "readlinkSync", |args| {
        let link = args.get(0).to_string();
        let target = std::fs::read_link(&link).map_err(|e| e.to_string())?;
        Ok(Value::new_string(args.context(), &target.to_string_lossy()))
    });
    bind(ctx, obj, "chmodSync", |args| {
        let _path = args.get(0).to_string();
        let _mode = args.get(1).to_number() as u32;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&_path, std::fs::Permissions::from_mode(_mode))
                .map_err(|e| e.to_string())?;
        }
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "chownSync", |args| {
        // Stub — chown via libc would need bindings; just succeed.
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "lchownSync", |args| {
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "lchmodSync", |args| {
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "utimesSync", |args| {
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "futimesSync", |args| {
        Ok(Value::new_undefined(args.context()))
    });
    bind(ctx, obj, "linkSync", |args| {
        let src = args.get(0).to_string();
        let dst = args.get(1).to_string();
        std::fs::hard_link(&src, &dst).map_err(|e| e.to_string())?;
        Ok(Value::new_undefined(args.context()))
    });

    // Async variants forwarding to *Sync with queueMicrotask callback dispatch.
    let _ = ctx.eval(
        r#"
        (function(fs){
            const syncToAsync = (name) => function(...args) {
                const cb = typeof args[args.length - 1] === "function" ? args.pop() : null;
                try {
                    const r = fs[name + "Sync"](...args);
                    if (cb) queueMicrotask(() => cb(null, r));
                    return r;
                } catch (e) {
                    if (cb) queueMicrotask(() => cb(e));
                    else throw e;
                }
            };
            for (const k of ["open", "close", "read", "write", "fstat", "fsync", "ftruncate", "symlink", "readlink", "chmod", "chown", "lchown", "lchmod", "utimes", "futimes", "link"]) {
                if (typeof fs[k] !== "function" && typeof fs[k + "Sync"] === "function") {
                    fs[k] = syncToAsync(k);
                }
            }
        })(arguments[0]);
        "#,
        Some("[fs-async-wrap]"),
    ).and_then(|f| f.to_object().and_then(|o| o.call(None, &[obj.as_value()])));
}

fn install_streaming(ctx: &Context, obj: &bun_jsc::Object<'_>) {
    // createReadStream(path, opts?) → Node Readable streaming the file in 16KB chunks.
    bind(ctx, obj, "createReadStream", |args| {
        let path = args.get(0).to_string();
        let highwater = args
            .get(1)
            .to_object()
            .ok()
            .and_then(|o| o.get_property("highWaterMark").ok())
            .map(|v| v.to_number() as usize)
            .unwrap_or(64 * 1024);

        // Build a Node Readable on the JS side via the globally-installed
        // __bun_NodeReadable class; we'll push chunks into it from Rust.
        let ctx = args.context();
        let builder = ctx
            .eval(
                r#"
                (function build(start) {
                    const r = new globalThis.__bun_NodeReadable({});
                    let started = false;
                    r._read = () => {
                        if (started) return;
                        started = true;
                        start(r);
                    };
                    return r;
                })
                "#,
                Some("[fs.createReadStream]"),
            )
            .map_err(|e| e.to_string())?
            .to_object()
            .map_err(|e| e.to_string())?;

        // Rust closure that, when invoked, spawns a tokio task that opens the
        // file and pumps chunks back. `start_obj` is a JS callback we hand to
        // the builder; when JS calls it (start(r)), we receive the Readable
        // and capture it for cross-thread use.
        let start_cb = Callback::new(ctx, "fs_create_read_stream_start", move |args| {
            let r_obj = args.get(0).to_object().map_err(|e| e.to_string())?;
            let r_raw = r_obj.as_raw();
            // Protect r so it stays alive across threads.
            unsafe {
                sys::JSValueProtect(args.context().as_raw(), r_raw as sys::JSValueRef);
            }
            let r_id = r_raw as usize;
            let path = path.clone();
            crate::async_rt::note_started();
            crate::async_rt::spawn(async move {
                let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    use std::io::Read;
                    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                    let mut buf = vec![0u8; highwater];
                    loop {
                        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                        if n == 0 {
                            push_to_readable(r_id, None);
                            return Ok(());
                        }
                        let chunk: Vec<u8> = buf[..n].to_vec();
                        push_to_readable(r_id, Some(chunk));
                    }
                })
                .await;
                if let Ok(Err(e)) = result {
                    push_error(r_id, e);
                }
                // Unprotect on the JS thread.
                crate::async_rt::post_to_js(move |ctx| {
                    let raw = r_id as sys::JSValueRef;
                    unsafe {
                        sys::JSValueUnprotect(ctx.as_raw(), raw);
                    }
                    crate::async_rt::note_finished();
                });
            });
            Ok(Value::new_undefined(args.context()))
        });
        let r = builder
            .call(None, &[start_cb.value_in(ctx)])
            .map_err(|e| e.to_string())?;
        std::mem::forget(start_cb);
        Ok(r)
    });

    // createWriteStream(path, opts?) → Node Writable; writes go through tokio.
    bind(ctx, obj, "createWriteStream", |args| {
        let path = args.get(0).to_string();
        let ctx = args.context();
        let builder = ctx
            .eval(
                r#"
                (function build(writeFn, closeFn) {
                    return new globalThis.__bun_NodeWritable({
                        write(chunk, enc, cb) { writeFn(chunk, cb); },
                        // close on end is handled via the wrapper.
                    });
                })
                "#,
                Some("[fs.createWriteStream]"),
            )
            .unwrap()
            .to_object()
            .unwrap();

        let file_slot: std::rc::Rc<std::cell::RefCell<Option<std::fs::File>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        {
            // Open the file synchronously on first construction. Truncate-by-default.
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            *file_slot.borrow_mut() = Some(f);
        }
        let file_slot_for_write = file_slot.clone();
        let write_cb = Callback::new(ctx, "fs_write_stream_write", move |args| {
            use std::io::Write;
            let chunk_v = args.get(0);
            let cb = args.get(1);
            let bytes: Vec<u8> = match chunk_v.typed_array_bytes() {
                Some(b) => b.to_vec(),
                None => chunk_v.to_string().into_bytes(),
            };
            let mut g = file_slot_for_write.borrow_mut();
            if let Some(f) = g.as_mut() {
                let res = f.write_all(&bytes);
                if let Ok(cb_obj) = cb.to_object() {
                    if cb_obj.is_function() {
                        let err = match res {
                            Ok(()) => Value::new_null(args.context()),
                            Err(e) => Value::new_string(args.context(), &e.to_string()),
                        };
                        let _ = cb_obj.call(None, &[err]);
                    }
                }
            }
            Ok(Value::new_undefined(args.context()))
        });

        let close_cb = Callback::new(ctx, "fs_write_stream_close", move |args| {
            *file_slot.borrow_mut() = None;
            Ok(Value::new_undefined(args.context()))
        });

        let r = builder
            .call(None, &[write_cb.value_in(ctx), close_cb.value_in(ctx)])
            .map_err(|e| e.to_string())?;
        std::mem::forget(write_cb);
        std::mem::forget(close_cb);
        Ok(r)
    });
}

/// Helpers called from tokio tasks. Each just queues work on the JS thread.

fn push_to_readable(r_id: usize, chunk: Option<Vec<u8>>) {
    crate::async_rt::post_to_js(move |ctx| {
        let raw = r_id as sys::JSObjectRef;
        let obj = unsafe { bun_jsc::Object::from_raw_for_runtime(ctx, raw) };
        let push_fn = match obj.get_property("push") {
            Ok(p) => match p.to_object() {
                Ok(o) => o,
                Err(_) => return,
            },
            Err(_) => return,
        };
        match chunk {
            Some(bytes) => {
                let u8 = crate::buffer::buffer_from_bytes(ctx, bytes);
                let _ = push_fn.call(Some(obj), &[u8]);
            }
            None => {
                let _ = push_fn.call(Some(obj), &[Value::new_null(ctx)]);
            }
        }
    });
}

fn push_error(r_id: usize, message: String) {
    crate::async_rt::post_to_js(move |ctx| {
        let raw = r_id as sys::JSObjectRef;
        let obj = unsafe { bun_jsc::Object::from_raw_for_runtime(ctx, raw) };
        let emit = match obj.get_property("emit").and_then(|v| v.to_object()) {
            Ok(o) => o,
            Err(_) => return,
        };
        let evt = Value::new_string(ctx, "error");
        let err = Value::new_string(ctx, &message);
        let _ = emit.call(Some(obj), &[evt, err]);
    });
}

fn install_sync(ctx: &Context, obj: &bun_jsc::Object<'_>) {
    bind(ctx, obj, "readFileSync", |args| {
        let path = args.get(0).to_string();
        let bytes = fs::read(&path).map_err(|e| io_err_path(e, &path))?;
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
    bind(ctx, obj, "rmdirSync", |args| {
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
            fs::remove_dir_all(&p).map_err(io_err)?;
        } else {
            fs::remove_dir(&p).map_err(io_err)?;
        }
        Ok(Value::new_undefined(args.context()))
    });
    // Async (callback or promise) wrappers for rm/rmdir.
    bind(ctx, obj, "rm", |args| {
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
        let path = std::path::PathBuf::from(&p);
        // If last arg is a callback, use callback form.
        let cb_idx = if args.len() >= 3 && args.get(2).is_object() && args.get(2).to_object().map(|o| o.is_function()).unwrap_or(false) {
            Some(2)
        } else if args.len() >= 2 && args.get(1).is_object() && args.get(1).to_object().map(|o| o.is_function()).unwrap_or(false) {
            Some(1)
        } else {
            None
        };
        let result = if path.is_dir() {
            if recursive { fs::remove_dir_all(&path) } else { fs::remove_dir(&path) }
        } else {
            fs::remove_file(&path)
        };
        let ctx = args.context();
        if let Some(i) = cb_idx {
            let cb = args.get(i);
            if let Ok(cb_obj) = cb.to_object() {
                match result {
                    Ok(()) => { let _ = cb_obj.call(None, &[Value::new_null(ctx)]); }
                    Err(e) => {
                        let err = ctx.eval(&format!("new Error({:?})", e.to_string()), Some("[fs.rm]"))
                            .unwrap_or_else(|_| Value::new_null(ctx));
                        let _ = cb_obj.call(None, &[err]);
                    }
                }
            }
            return Ok(Value::new_undefined(ctx));
        }
        match result {
            Ok(()) => ctx.eval("Promise.resolve()", Some("[fs.rm.promise]")).map_err(|e| e.to_string()),
            Err(e) => ctx.eval(&format!("Promise.reject(new Error({:?}))", e.to_string()), Some("[fs.rm.promise]")).map_err(|e| e.to_string()),
        }
    });
    bind(ctx, obj, "rmdir", |args| {
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
        let result = if recursive {
            fs::remove_dir_all(&p)
        } else {
            fs::remove_dir(&p)
        };
        let ctx = args.context();
        // Optional callback (Node style).
        let cb_idx = if args.len() >= 3 && args.get(2).to_object().map(|o| o.is_function()).unwrap_or(false) {
            Some(2)
        } else if args.len() >= 2 && args.get(1).to_object().map(|o| o.is_function()).unwrap_or(false) {
            Some(1)
        } else { None };
        if let Some(i) = cb_idx {
            if let Ok(cb_obj) = args.get(i).to_object() {
                match result {
                    Ok(()) => { let _ = cb_obj.call(None, &[Value::new_null(ctx)]); }
                    Err(e) => {
                        let err = ctx.eval(&format!("new Error({:?})", e.to_string()), Some("[fs.rmdir]")).unwrap_or_else(|_| Value::new_null(ctx));
                        let _ = cb_obj.call(None, &[err]);
                    }
                }
            }
            return Ok(Value::new_undefined(ctx));
        }
        match result {
            Ok(()) => ctx.eval("Promise.resolve()", Some("[fs.rmdir.promise]")).map_err(|e| e.to_string()),
            Err(e) => ctx.eval(&format!("Promise.reject(new Error({:?}))", e.to_string()), Some("[fs.rmdir.promise]")).map_err(|e| e.to_string()),
        }
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

    // fs.realpathSync.native(path) — Node attaches a libuv-backed version
    // as a method on the realpathSync function. Same semantics as the
    // plain realpathSync above; we just need the function to exist.
    if let Ok(rps) = obj.get_property("realpathSync") {
        if let Ok(rps_obj) = rps.to_object() {
            let _ = rps_obj.set_property("native", &rps);
        }
    }

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
    use std::io::ErrorKind;
    let (code, msg) = match e.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        ErrorKind::WouldBlock => ("EAGAIN", "resource temporarily unavailable"),
        ErrorKind::InvalidInput => ("EINVAL", "invalid argument"),
        ErrorKind::InvalidData => ("EILSEQ", "invalid or incomplete multibyte or wide character"),
        ErrorKind::TimedOut => ("ETIMEDOUT", "operation timed out"),
        ErrorKind::WriteZero => ("EIO", "I/O error"),
        ErrorKind::Interrupted => ("EINTR", "interrupted system call"),
        ErrorKind::Unsupported => ("ENOSYS", "function not implemented"),
        ErrorKind::OutOfMemory => ("ENOMEM", "cannot allocate memory"),
        _ => ("EIO", "I/O error"),
    };
    format!("{code}: {msg} ({e})")
}

fn io_err_path(e: std::io::Error, path: &str) -> String {
    use std::io::ErrorKind;
    let (code, msg) = match e.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        _ => return io_err(e),
    };
    format!("{code}: {msg}, '{path}'")
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
