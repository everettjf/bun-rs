//! `globalThis.process` — argv, env, cwd, exit, platform, pid, versions.
//!
//! MVP subset; matches Node enough for `process.argv` / `process.env.PATH` /
//! `process.exit(code)` / `process.platform` / `process.cwd()`.

use std::sync::Mutex;
use std::sync::OnceLock;

use bun_jsc::{Callback, Context, Value};

pub fn install_process(ctx: &Context, argv: Vec<String>) {
    let proc_val = ctx
        .eval("({})", Some("[process]"))
        .expect("create plain object");
    let proc = proc_val.to_object().expect("to_object");

    // argv: array of strings
    let argv_val = build_string_array(ctx, &argv);
    proc.set_property("argv", &argv_val).expect("set argv");

    // env: plain object copy of std::env::vars()
    let env_val = build_env_object(ctx);
    proc.set_property("env", &env_val).expect("set env");

    // pid (number)
    let pid = std::process::id() as f64;
    proc.set_property("pid", &Value::new_number(ctx, pid))
        .expect("set pid");

    // platform (string) — matches Node values.
    let plat = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    proc.set_property("platform", &Value::new_string(ctx, plat))
        .expect("set platform");

    // arch
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    proc.set_property("arch", &Value::new_string(ctx, arch))
        .expect("set arch");

    // cwd()
    let cwd_cb = Callback::new(ctx, "cwd", |args| {
        let dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Value::new_string(args.context(), &dir))
    });
    proc.set_property("cwd", &cwd_cb.value_in(ctx)).expect("set cwd");
    std::mem::forget(cwd_cb);

    // exit(code?)
    let exit_cb = Callback::new(ctx, "exit", |args| {
        let code = if args.len() == 0 {
            0
        } else {
            args.get(0).to_number() as i32
        };
        // Flush stdio so the user actually sees their last log line.
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(code);
    });
    proc.set_property("exit", &exit_cb.value_in(ctx)).expect("set exit");
    std::mem::forget(exit_cb);

    // stdout / stderr / stdin — minimal Writable/Readable shape.
    let stdout = make_stream(ctx, /* is_stderr */ false);
    proc.set_property("stdout", &stdout).expect("set stdout");
    let stderr = make_stream(ctx, /* is_stderr */ true);
    proc.set_property("stderr", &stderr).expect("set stderr");
    let stdin = make_stdin(ctx);
    proc.set_property("stdin", &stdin).expect("set stdin");

    // process.hrtime / hrtime.bigint — high-resolution time
    let hrtime_cb = Callback::new(ctx, "hrtime", |args| {
        let ctx = args.context();
        let now = std::time::Instant::now()
            .duration_since(std::time::Instant::now() - std::time::Duration::from_secs(0));
        // Better: use UNIX_EPOCH for absolute monotonic-ish reference.
        let total_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let _ = now;
        let secs = (total_ns / 1_000_000_000) as f64;
        let nanos = (total_ns % 1_000_000_000) as f64;
        let arr = ctx.eval("[]", Some("[hrtime]")).unwrap();
        let arr_o = arr.to_object().unwrap();
        arr_o
            .set_property("0", &Value::new_number(ctx, secs))
            .unwrap();
        arr_o
            .set_property("1", &Value::new_number(ctx, nanos))
            .unwrap();
        arr_o
            .set_property("length", &Value::new_number(ctx, 2.0))
            .unwrap();
        Ok(arr)
    });
    proc.set_property("hrtime", &hrtime_cb.value_in(ctx))
        .expect("set hrtime");
    std::mem::forget(hrtime_cb);

    // nextTick(fn) — schedule on microtask queue.
    ctx.eval(
        "globalThis.__bun_nextTick = (fn, ...args) => queueMicrotask(() => fn(...args));",
        Some("[nextTick-helper]"),
    )
    .unwrap();
    let next_tick_fn = ctx
        .global_object()
        .get_property("__bun_nextTick")
        .expect("get nextTick");
    proc.set_property("nextTick", &next_tick_fn)
        .expect("set nextTick");

    // versions.bun (string) — useful for compat checks.
    let versions = ctx
        .eval("({})", Some("[process.versions]"))
        .expect("create versions obj");
    let versions_obj = versions.to_object().expect("to_object");
    versions_obj
        .set_property(
            "bun",
            &Value::new_string(ctx, env!("CARGO_PKG_VERSION")),
        )
        .expect("set versions.bun");
    proc.set_property("versions", &versions).expect("set versions");

    // Stash argv for future read-back (e.g. process.argv0).
    set_argv(argv);

    let global = ctx.global_object();
    global
        .set_property("process", &proc.as_value())
        .expect("set globalThis.process");
}

fn build_string_array<'ctx>(ctx: &'ctx Context, items: &[String]) -> Value<'ctx> {
    let arr_val = ctx
        .eval("[]", Some("[process.argv]"))
        .expect("make array");
    let arr = arr_val.to_object().expect("to_object");
    for (i, s) in items.iter().enumerate() {
        let key = i.to_string();
        arr.set_property(&key, &Value::new_string(ctx, s))
            .expect("set argv elem");
    }
    arr.set_property("length", &Value::new_number(ctx, items.len() as f64))
        .expect("set length");
    arr.as_value()
}

fn build_env_object<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let obj_val = ctx.eval("({})", Some("[process.env]")).expect("make obj");
    let obj = obj_val.to_object().expect("to_object");
    for (k, v) in std::env::vars() {
        obj.set_property(&k, &Value::new_string(ctx, &v))
            .unwrap_or_else(|_| {
                // Ignore invalid keys (containing `\0` etc.)
            });
    }
    obj.as_value()
}

fn make_stream<'ctx>(ctx: &'ctx Context, is_stderr: bool) -> Value<'ctx> {
    let v = ctx.eval("({})", Some("[process.stream]")).unwrap();
    let obj = v.to_object().unwrap();

    let write_cb = Callback::new(ctx, "write", move |args| {
        let v = args.get(0);
        let bytes: Vec<u8> = match v.typed_array_bytes() {
            Some(b) => b.to_vec(),
            None => v.to_string().into_bytes(),
        };
        use std::io::Write;
        let _ = if is_stderr {
            std::io::stderr().write_all(&bytes)
        } else {
            std::io::stdout().write_all(&bytes)
        };
        Ok(Value::new_bool(args.context(), true))
    });
    obj.set_property("write", &write_cb.value_in(ctx)).unwrap();
    std::mem::forget(write_cb);

    obj.set_property("isTTY", &Value::new_bool(ctx, is_tty(is_stderr)))
        .unwrap();
    obj.set_property(
        "columns",
        &Value::new_number(ctx, term_columns().unwrap_or(80) as f64),
    )
    .unwrap();
    obj.set_property("rows", &Value::new_number(ctx, 24.0))
        .unwrap();

    // No-op end() so user libs that close streams don't crash.
    let end_cb = Callback::new(ctx, "end", |args| {
        Ok(Value::new_undefined(args.context()))
    });
    obj.set_property("end", &end_cb.value_in(ctx)).unwrap();
    std::mem::forget(end_cb);

    v
}

fn make_stdin<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    // Minimal stub — full TTY input would need readline or raw mode.
    // For now expose isTTY + read() that returns null (end-of-stream).
    let v = ctx.eval("({})", Some("[process.stdin]")).unwrap();
    let obj = v.to_object().unwrap();
    obj.set_property("isTTY", &Value::new_bool(ctx, is_tty(false)))
        .unwrap();
    let read_cb = Callback::new(ctx, "read", |args| {
        // Returning null means "no data available" in Node's semantics.
        Ok(Value::new_null(args.context()))
    });
    obj.set_property("read", &read_cb.value_in(ctx)).unwrap();
    std::mem::forget(read_cb);
    v
}

#[cfg(unix)]
fn is_tty(stderr: bool) -> bool {
    unsafe {
        extern "C" {
            fn isatty(fd: i32) -> i32;
        }
        // stderr=fd 2, stdout=fd 1
        isatty(if stderr { 2 } else { 1 }) != 0
    }
}

#[cfg(not(unix))]
fn is_tty(_: bool) -> bool {
    false
}

#[cfg(unix)]
fn term_columns() -> Option<u32> {
    unsafe {
        #[repr(C)]
        struct Winsize {
            row: u16,
            col: u16,
            xpix: u16,
            ypix: u16,
        }
        extern "C" {
            fn ioctl(fd: i32, req: u64, ...) -> i32;
        }
        let mut ws: Winsize = std::mem::zeroed();
        // TIOCGWINSZ — platform-specific magic number.
        const TIOCGWINSZ: u64 = if cfg!(target_os = "macos") {
            0x40087468
        } else {
            0x5413
        };
        if ioctl(1, TIOCGWINSZ, &mut ws) == 0 && ws.col > 0 {
            return Some(ws.col as u32);
        }
        None
    }
}

#[cfg(not(unix))]
fn term_columns() -> Option<u32> {
    None
}

fn argv_slot() -> &'static Mutex<Vec<String>> {
    static SLOT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Vec::new()))
}

fn set_argv(v: Vec<String>) {
    if let Ok(mut g) = argv_slot().lock() {
        *g = v;
    }
}
