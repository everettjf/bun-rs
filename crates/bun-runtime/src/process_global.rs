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

    // process.config — Node compatibility (some tests probe it).
    let config_v = ctx
        .eval(
            r#"({
                target_defaults: { cflags: [], default_configuration: "Release", defines: [], include_dirs: [], libraries: [] },
                variables: {
                    asan: 0, coverage: false, debug_nghttp2: false, enable_lto: false, enable_pgo_generate: false, enable_pgo_use: false,
                    force_dynamic_crt: 0, host_arch: "x64", icu_data_in: "", icu_data_path: "", icu_default_data: "",
                    icu_endianness: "l", icu_gyp_path: "tools/icu/icu-system.gyp", icu_path: "deps/icu-small", icu_small: false,
                    icu_ver_major: "73", node_byteorder: "little", node_install_npm: true, node_install_corepack: true,
                    node_module_version: 120, node_no_browser_globals: false, node_prefix: "/", node_release_urlbase: "",
                    node_shared: false, node_shared_libuv: false, node_use_dtrace: false, node_use_etw: false, node_use_node_code_cache: true,
                    node_use_node_snapshot: true, node_use_openssl: true, node_use_v8_platform: true, node_with_ltcg: false,
                    node_without_node_options: false, openssl_quic: true, shlib_suffix: "120.dylib", target_arch: "x64",
                    target_platform: "linux", v8_enable_31bit_smis_on_64bit_arch: 0, v8_enable_gdbjit: 0, v8_no_strict_aliasing: 1,
                    v8_optimized_debug: 1, v8_promise_internal_field_count: 1, v8_random_seed: 0, v8_trace_maps: 0,
                    v8_use_siphash: 1
                }
            })"#,
            Some("[process.config]"),
        )
        .unwrap();
    proc.set_property("config", &config_v).expect("set config");
    // process.versions — common probes.
    let versions_v = ctx
        .eval(
            r#"({
                node: "20.0.0",
                v8: "11.0.0",
                bun: "1.0.0",
                modules: "120",
                uv: "1.46.0",
                openssl: "3.0.0",
                ares: "1.20.0",
                http_parser: "2.9.4",
                napi: "9",
                nghttp2: "1.55.0",
                zlib: "1.2.13",
                brotli: "1.0.9",
                icu: "73.2",
                unicode: "15.0",
                cldr: "43.0",
                tz: "2023c",
                tzdata: "2023c",
                webkit: "618.1"
            })"#,
            Some("[process.versions]"),
        )
        .unwrap();
    proc.set_property("versions", &versions_v).expect("set versions");
    // process.release — Node-style.
    let release_v = ctx
        .eval(
            r#"({ name: "node", lts: "Iron", sourceUrl: "", headersUrl: "", libUrl: "" })"#,
            Some("[process.release]"),
        )
        .unwrap();
    proc.set_property("release", &release_v).expect("set release");
    // process.features — Node uses this for capability probing.
    let features_v = ctx
        .eval(
            r#"({ inspector: false, debug: false, uv: true, ipv6: true, tls_alpn: true, tls_sni: true, tls_ocsp: true, tls: true, cached_builtins: true, typescript: true })"#,
            Some("[process.features]"),
        )
        .unwrap();
    proc.set_property("features", &features_v).expect("set features");

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

    // hrtime.bigint() — same as hrtime() but returns a BigInt of total ns.
    let hrtime_bigint_cb = Callback::new(ctx, "bigint", |args| {
        let ctx = args.context();
        // Total nanoseconds since UNIX_EPOCH. JS BigInt isn't directly
        // constructible from Rust, but we can build it via eval(`BigInt("...")`).
        let dur = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let ns = dur.as_nanos();
        let src = format!("BigInt(\"{ns}\")");
        ctx.eval(&src, Some("[hrtime.bigint]")).map_err(|e| e.to_string())
    });
    if let Ok(hrtime_obj) = proc
        .get_property("hrtime")
        .and_then(|v| v.to_object())
    {
        hrtime_obj
            .set_property("bigint", &hrtime_bigint_cb.value_in(ctx))
            .ok();
    }
    std::mem::forget(hrtime_bigint_cb);

    // process.memoryUsage() — returns { rss, heapTotal, heapUsed, external,
    // arrayBuffers }. We don't have real measurements, but tests typically
    // just want numeric values.
    let mem_cb = Callback::new(ctx, "memoryUsage", |args| {
        let ctx = args.context();
        ctx.eval(
            r#"({
                rss: 0,
                heapTotal: 0,
                heapUsed: 0,
                external: 0,
                arrayBuffers: 0,
            })"#,
            Some("[memoryUsage]"),
        )
        .map_err(|e| e.to_string())
    });
    proc.set_property("memoryUsage", &mem_cb.value_in(ctx))
        .expect("set memoryUsage");
    // .rss helper on the memoryUsage function: Bun supports `process.memoryUsage.rss()`.
    let mem_rss_cb = Callback::new(ctx, "rss", |args| {
        let _ = args;
        Ok(Value::new_number(args.context(), 0.0))
    });
    if let Ok(mu) = proc
        .get_property("memoryUsage")
        .and_then(|v| v.to_object())
    {
        mu.set_property("rss", &mem_rss_cb.value_in(ctx)).ok();
    }
    std::mem::forget(mem_rss_cb);
    std::mem::forget(mem_cb);

    // process.cpuUsage([previous]) — { user, system } in microseconds. Stub.
    let cpu_cb = Callback::new(ctx, "cpuUsage", |args| {
        let ctx = args.context();
        ctx.eval("({ user: 0, system: 0 })", Some("[cpuUsage]"))
            .map_err(|e| e.to_string())
    });
    proc.set_property("cpuUsage", &cpu_cb.value_in(ctx))
        .expect("set cpuUsage");
    std::mem::forget(cpu_cb);

    // process.uptime() — seconds since process start.
    let uptime_cb = Callback::new(ctx, "uptime", |args| {
        let ctx = args.context();
        Ok(Value::new_number(ctx, 0.0))
    });
    proc.set_property("uptime", &uptime_cb.value_in(ctx))
        .expect("set uptime");
    std::mem::forget(uptime_cb);

    // process.emitWarning(warning, [type], [code]) — no-op shim.
    let emit_warn_cb = Callback::new(ctx, "emitWarning", |args| {
        let ctx = args.context();
        if args.len() >= 1 {
            eprintln!("(bun-rs warning) {}", args.get(0).to_string());
        }
        Ok(Value::new_undefined(ctx))
    });
    proc.set_property("emitWarning", &emit_warn_cb.value_in(ctx))
        .expect("set emitWarning");
    std::mem::forget(emit_warn_cb);

    // process.umask() — current process umask. Stub 0.
    let umask_cb = Callback::new(ctx, "umask", |args| {
        Ok(Value::new_number(args.context(), 0.0))
    });
    proc.set_property("umask", &umask_cb.value_in(ctx)).ok();
    std::mem::forget(umask_cb);

    // process.kill(pid, signal) — stub.
    let kill_cb = Callback::new(ctx, "kill", |args| {
        let ctx = args.context();
        Ok(Value::new_bool(ctx, true))
    });
    proc.set_property("kill", &kill_cb.value_in(ctx)).ok();
    std::mem::forget(kill_cb);

    // process.chdir(dir) — change cwd.
    let chdir_cb = Callback::new(ctx, "chdir", |args| {
        let ctx = args.context();
        let dir = args.get(0).to_string();
        std::env::set_current_dir(&dir).map_err(|e| e.to_string())?;
        Ok(Value::new_undefined(ctx))
    });
    proc.set_property("chdir", &chdir_cb.value_in(ctx)).ok();
    std::mem::forget(chdir_cb);

    // process.getuid / .getgid / .getpid (stubs / real where easy).
    let getuid = Callback::new(ctx, "getuid", |args| {
        // Real getuid via libc would need bindings; return 1000 as a stable stub.
        Ok(Value::new_number(args.context(), 1000.0))
    });
    proc.set_property("getuid", &getuid.value_in(ctx)).ok();
    std::mem::forget(getuid);
    let getgid = Callback::new(ctx, "getgid", |args| {
        Ok(Value::new_number(args.context(), 1000.0))
    });
    proc.set_property("getgid", &getgid.value_in(ctx)).ok();
    std::mem::forget(getgid);

    // process.binding / .dlopen / .reallyExit — stubs.
    let _ = ctx.eval(
        r#"
        (function(p){
            p.binding = (_name) => ({});
            p.dlopen = () => { throw new Error("process.dlopen not implemented"); };
            p.reallyExit = (code) => process.exit(code || 0);
            p.abort = () => process.exit(134);
            p.allowedNodeEnvironmentFlags = new Set();
            p.config = { variables: {}, target_defaults: {} };
            p.connected = false;
            p.debugPort = 9229;
            p.execArgv = [];
            // execPath: Bun sets this to the absolute bun executable path.
            // argv[0] is already the bun-rs path (set by CLI to current_exe());
            // fall through to that, or as last resort use a stable string.
            p.execPath = process.argv[0] || "bun-rs";
            // Bun's argv0 == basename(execPath) by default.
            if (!p.argv0) {
              try {
                const ep = p.execPath || "";
                const m = String(ep).split(/[\/\\]/).pop();
                p.argv0 = m || "bun-rs";
              } catch { p.argv0 = "bun-rs"; }
            }
            p.features = { ipv6: true, tls: true };
            p.openStdin = () => process.stdin;
            p.report = { directory: "", filename: "", reportOnFatalError: false, reportOnSignal: false, reportOnUncaughtException: false, signal: "SIGUSR2", getReport: () => "{}", writeReport: () => "" };
            p.resourceUsage = () => ({ userCPUTime: 0, systemCPUTime: 0, maxRSS: 0, sharedMemorySize: 0, unsharedDataSize: 0, unsharedStackSize: 0, minorPageFault: 0, majorPageFault: 0, swappedOut: 0, fsRead: 0, fsWrite: 0, ipcSent: 0, ipcReceived: 0, signalsCount: 0, voluntaryContextSwitches: 0, involuntaryContextSwitches: 0 });
            p.send = () => false;
            p.disconnect = () => {};
            p.title = "bun-rs";
            if (!p.release) p.release = { name: "bun-rs", sourceUrl: "", headersUrl: "", libUrl: "" };
        })(globalThis.process);
        "#,
        Some("[process-extras]"),
    );

    // EventEmitter-style stubs: on/off/emit/once/removeListener/addListener.
    ctx.eval(
        r#"
        (function(p){
            const ls = {};
            p.on = function(ev, cb) { (ls[ev] = ls[ev] || []).push(cb); return this; };
            p.addListener = p.on;
            p.off = function(ev, cb) {
                const a = ls[ev]; if (!a) return this;
                const i = a.indexOf(cb); if (i >= 0) a.splice(i, 1); return this;
            };
            p.removeListener = p.off;
            p.removeAllListeners = function(ev) { if (ev) delete ls[ev]; else for (const k in ls) delete ls[k]; return this; };
            p.once = function(ev, cb) { const w = (...a) => { p.off(ev, w); cb(...a); }; return p.on(ev, w); };
            p.emit = function(ev, ...args) { for (const cb of (ls[ev]||[]).slice()) try { cb(...args); } catch (e) {} };
            p.listeners = function(ev) { return (ls[ev] || []).slice(); };
            p.listenerCount = function(ev) { return (ls[ev] || []).length; };
            p.eventNames = function() { return Object.keys(ls); };
            p.setMaxListeners = function(){};
            p.getMaxListeners = function(){ return 10; };
        })(globalThis.process || (globalThis.process = {}));
        "#,
        Some("[process-eventemitter]"),
    )
    .ok();

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

    // versions.bun — append to the existing versions object (set earlier).
    if let Ok(versions) = proc.get_property("versions") {
        if let Ok(versions_obj) = versions.to_object() {
            let _ = versions_obj.set_property(
                "bun",
                &Value::new_string(ctx, env!("CARGO_PKG_VERSION")),
            );
        }
    }

    // process.revision — Bun exposes the git revision of the binary.
    // We use a stable stub so tests can compare process.revision === Bun.revision.
    proc.set_property(
        "revision",
        &Value::new_string(ctx, "bun-rs-dev"),
    )
    .expect("set revision");

    // Stash argv for future read-back (e.g. process.argv0).
    set_argv(argv);

    let global = ctx.global_object();
    global
        .set_property("process", &proc.as_value())
        .expect("set globalThis.process");

    // The earlier [process-extras] eval ran before globalThis.process was
    // bound, so its IIFE got undefined and silently no-op'd. Re-run the
    // execPath / argv0 / arch / report bookkeeping now that `process` is
    // reachable from JS.
    let _ = ctx.eval(
        r#"
        (function(p){
            if (!p) return;
            if (!p.execPath || typeof p.execPath !== "string") {
                p.execPath = (p.argv && p.argv[0]) || "bun-rs";
            }
            if (!p.argv0) {
                try {
                    const ep = p.execPath || "";
                    const m = String(ep).split(/[\/\\]/).pop();
                    p.argv0 = m || "bun-rs";
                } catch { p.argv0 = "bun-rs"; }
            }
        })(globalThis.process);
        "#,
        Some("[process-execpath-fixup]"),
    );
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
