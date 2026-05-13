//! Built-in `node:*` modules. Each submodule builds and returns a fresh
//! JS object that mirrors the relevant Node API surface (subset).
//!
//! These are pre-registered at startup so `__bun_require("node:fs", _)`
//! resolves before touching the filesystem.

use std::cell::RefCell;
use std::collections::HashMap;

use bun_jsc::{Context, Value};
use bun_jsc_sys as sys;

pub mod assert;
pub mod buffer;
pub mod child_process;
pub mod crypto;
pub mod events;
pub mod fs;
pub mod http;
pub mod os;
pub mod path;
pub mod querystring;
pub mod readline;
pub mod stream;
pub mod url;
pub mod util;
pub mod zlib;

// Per-thread cache of built builtin exports. The Value's raw ref is kept
// alive via `JSValueProtect` for the duration of the process.
thread_local! {
    static BUILTINS: RefCell<HashMap<&'static str, sys::JSValueRef>> = RefCell::new(HashMap::new());
}

/// Return cached exports for `node:<name>`, building it on first access.
///
/// `None` means the name isn't a recognized builtin — caller should fall
/// through to file-system resolution.
pub fn load<'ctx>(ctx: &'ctx Context, name: &str) -> Option<Value<'ctx>> {
    // Sub-namespace modules: e.g. `fs/promises` is `require("fs").promises`,
    // `stream/web` is `require("stream").web`, etc. Carve these out before
    // the main dispatch so they aren't treated as unknown.
    if let Some(v) = load_submodule(ctx, name) {
        return Some(v);
    }
    let builder: fn(&Context) -> Value<'_> = match name {
        "path" | "node:path" => path::build,
        "path/posix" | "node:path/posix" => path::build,
        "path/win32" | "node:path/win32" => path::build,
        "os" | "node:os" => os::build,
        "fs" | "node:fs" => fs::build,
        "buffer" | "node:buffer" => buffer::build,
        "events" | "node:events" => events::build,
        "util" | "node:util" => util::build,
        "util/types" | "node:util/types" => util::build_types,
        "crypto" | "node:crypto" => crypto::build,
        "child_process" | "node:child_process" => child_process::build,
        "assert" | "node:assert" => assert::build,
        "assert/strict" | "node:assert/strict" => assert::build,
        "querystring" | "node:querystring" => querystring::build,
        "url" | "node:url" => url::build,
        "stream" | "node:stream" => stream::build,
        "readline" | "node:readline" => readline::build,
        "zlib" | "node:zlib" => zlib::build,
        "http" | "node:http" => http::build,
        "process" | "node:process" => build_process_module,
        "tty" | "node:tty" => build_tty_stub,
        "net" | "node:net" => build_net_stub,
        "string_decoder" | "node:string_decoder" => build_string_decoder,
        "module" | "node:module" => build_module_stub,
        "v8" | "node:v8" => build_v8_stub,
        "perf_hooks" | "node:perf_hooks" => build_perf_hooks_stub,
        "timers" | "node:timers" => build_timers_stub,
        "timers/promises" | "node:timers/promises" => build_timers_promises_stub,
        "constants" | "node:constants" => build_constants_stub,
        "worker_threads" | "node:worker_threads" => build_worker_threads_stub,
        "dns" | "node:dns" => build_dns_stub,
        "dns/promises" | "node:dns/promises" => build_dns_promises_stub,
        "dgram" | "node:dgram" => build_dgram_stub,
        "vm" | "node:vm" => build_vm_stub,
        "punycode" | "node:punycode" => build_punycode_stub,
        "tls" | "node:tls" => build_tls_stub,
        "trace_events" | "node:trace_events" => build_trace_events_stub,
        "inspector" | "node:inspector" => build_inspector_stub,
        "wasi" | "node:wasi" => build_wasi_stub,
        "https" | "node:https" => http::build,
        _ => return None,
    };
    let key = canonical_name(name);
    let cached = BUILTINS.with(|m| m.borrow().get(key).copied());
    if let Some(raw) = cached {
        return Some(unsafe { Value::from_raw_public(ctx, raw) });
    }
    let v = builder(ctx);
    let raw = v.as_raw();
    unsafe { sys::JSValueProtect(ctx.as_raw(), raw) };
    BUILTINS.with(|m| m.borrow_mut().insert(key, raw));
    Some(v)
}

fn canonical_name(s: &str) -> &'static str {
    // Stringly-typed because the lifetime of a temporary leaks here is fine —
    // builtin names are a small fixed set. Map to a 'static slice.
    match s {
        "path" | "node:path" => "path",
        "os" | "node:os" => "os",
        "fs" | "node:fs" => "fs",
        "buffer" | "node:buffer" => "buffer",
        "events" | "node:events" => "events",
        "util" | "node:util" => "util",
        "crypto" | "node:crypto" => "crypto",
        "child_process" | "node:child_process" => "child_process",
        "assert" | "node:assert" => "assert",
        "querystring" | "node:querystring" => "querystring",
        "url" | "node:url" => "url",
        "stream" | "node:stream" => "stream",
        "readline" | "node:readline" => "readline",
        "zlib" | "node:zlib" => "zlib",
        "http" | "node:http" => "http",
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

fn load_submodule<'ctx>(ctx: &'ctx Context, name: &str) -> Option<Value<'ctx>> {
    // Resolve `<parent>/<sub>` by loading `<parent>` and reading the matching
    // property off it.
    let (parent, sub) = match name {
        "fs/promises" | "node:fs/promises" => ("fs", "promises"),
        "stream/promises" | "node:stream/promises" => ("stream", "promises"),
        "stream/web" | "node:stream/web" => ("stream", "web"),
        "stream/consumers" | "node:stream/consumers" => ("stream", "consumers"),
        "readline/promises" | "node:readline/promises" => ("readline", "promises"),
        _ => return None,
    };
    let parent_val = load(ctx, parent)?;
    let obj = parent_val.to_object().ok()?;
    let v = obj.get_property(sub).ok()?;
    if v.is_undefined() {
        return Some(parent_val);
    }
    if let Ok(o) = v.to_object() {
        let _ = o.set_property("__esModule", &Value::new_bool(ctx, true));
    }
    Some(v)
}

fn build_process_module<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let process = ctx
        .global_object()
        .get_property("process")
        .unwrap_or_else(|_| Value::new_undefined(ctx));
    if let Ok(o) = process.to_object() {
        let _ = o.set_property("__esModule", &Value::new_bool(ctx, true));
        let _ = o.set_property("default", &process);
    }
    process
}

fn build_tty_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            isatty: (_fd) => false,
            ReadStream: function(){},
            WriteStream: function(){},
        })"#,
        Some("[node:tty]"),
    )
    .unwrap()
}

fn build_net_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            isIP: (s) => /^\d+\.\d+\.\d+\.\d+$/.test(s) ? 4 : /:/.test(s) ? 6 : 0,
            isIPv4: (s) => /^\d+\.\d+\.\d+\.\d+$/.test(s),
            isIPv6: (s) => /:/.test(s),
            Socket: function(){ throw new Error('net.Socket not implemented'); },
            Server: function(){ throw new Error('net.Server not implemented'); },
            createServer: () => { throw new Error('net.createServer not implemented'); },
            connect: () => { throw new Error('net.connect not implemented'); },
            createConnection: () => { throw new Error('net.createConnection not implemented'); },
        })"#,
        Some("[node:net]"),
    )
    .unwrap()
}

fn build_string_decoder<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            StringDecoder: class StringDecoder {
                constructor(enc) { this.enc = enc || "utf8"; this.dec = new TextDecoder(this.enc); }
                write(buf) { return this.dec.decode(buf, { stream: true }); }
                end(buf) { return buf ? this.dec.decode(buf) : this.dec.decode(); }
            },
        })"#,
        Some("[node:string_decoder]"),
    )
    .unwrap()
}

fn build_module_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            createRequire: (_filename) => globalThis.require,
            _cache: {},
            _resolveFilename: (request) => request,
            Module: function(){},
            builtinModules: [
                "assert","buffer","child_process","crypto","dgram","dns","events",
                "fs","http","https","net","os","path","querystring","readline",
                "stream","string_decoder","timers","tls","tty","url","util",
                "v8","worker_threads","zlib"
            ],
            isBuiltin: (n) => /^node:/.test(n) || ["fs","path","os","crypto","util","events","stream","buffer","url","http","https","zlib","child_process","assert","querystring","readline","worker_threads","perf_hooks","process","timers","tty","net","constants","string_decoder","punycode","module","v8","dns","dgram"].includes(n),
        })"#,
        Some("[node:module]"),
    )
    .unwrap()
}

fn build_v8_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            serialize: (_v) => new Uint8Array(0),
            deserialize: () => undefined,
            getHeapStatistics: () => ({ total_heap_size: 0, used_heap_size: 0, heap_size_limit: 0 }),
        })"#,
        Some("[node:v8]"),
    )
    .unwrap()
}

fn build_perf_hooks_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            performance: globalThis.performance || {
                now: () => Date.now(),
                timeOrigin: 0,
                mark: () => {},
                measure: () => {},
                clearMarks: () => {},
                clearMeasures: () => {},
            },
            PerformanceObserver: class { constructor(){}; observe(){}; disconnect(){} },
            constants: {},
        })"#,
        Some("[node:perf_hooks]"),
    )
    .unwrap()
}

fn build_timers_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            setTimeout: globalThis.setTimeout,
            setInterval: globalThis.setInterval,
            setImmediate: globalThis.setImmediate || ((fn, ...a) => setTimeout(fn, 0, ...a)),
            clearTimeout: globalThis.clearTimeout,
            clearInterval: globalThis.clearInterval,
            clearImmediate: globalThis.clearImmediate || globalThis.clearTimeout,
        })"#,
        Some("[node:timers]"),
    )
    .unwrap()
}

fn build_timers_promises_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            setTimeout: (ms, v) => new Promise(r => setTimeout(() => r(v), ms)),
            setImmediate: (v) => new Promise(r => setTimeout(() => r(v), 0)),
        })"#,
        Some("[node:timers/promises]"),
    )
    .unwrap()
}

fn build_constants_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2,
            O_CREAT: 0o100, O_EXCL: 0o200, O_NOCTTY: 0o400, O_TRUNC: 0o1000,
            O_APPEND: 0o2000, O_NONBLOCK: 0o4000, O_SYNC: 0o4010000,
            S_IFMT: 0o170000, S_IFREG: 0o100000, S_IFDIR: 0o040000,
            S_IFLNK: 0o120000, S_IFBLK: 0o060000, S_IFCHR: 0o020000,
            S_IFIFO: 0o010000, S_IFSOCK: 0o140000,
            S_IRWXU: 0o700, S_IRWXG: 0o070, S_IRWXO: 0o007,
            S_IRUSR: 0o400, S_IWUSR: 0o200, S_IXUSR: 0o100,
            S_IRGRP: 0o040, S_IWGRP: 0o020, S_IXGRP: 0o010,
            S_IROTH: 0o004, S_IWOTH: 0o002, S_IXOTH: 0o001,
            S_ISUID: 0o4000, S_ISGID: 0o2000, S_ISVTX: 0o1000,
            F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
            // Signal constants (subset).
            SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGABRT: 6,
            SIGFPE: 8, SIGKILL: 9, SIGSEGV: 11, SIGPIPE: 13, SIGALRM: 14,
            SIGTERM: 15, SIGUSR1: 30, SIGUSR2: 31, SIGCHLD: 20,
        })"#,
        Some("[node:constants]"),
    )
    .unwrap()
}

fn build_worker_threads_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            isMainThread: true,
            parentPort: null,
            workerData: undefined,
            threadId: 0,
            Worker: globalThis.Worker || function(){ throw new Error('Worker not implemented'); },
            MessageChannel: class { constructor(){ this.port1 = {}; this.port2 = {}; } },
            BroadcastChannel: class { constructor(){} postMessage(){} close(){} },
        })"#,
        Some("[node:worker_threads]"),
    )
    .unwrap()
}

fn build_dns_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            lookup: (host, opts, cb) => {
                const c = typeof opts === "function" ? opts : cb;
                queueMicrotask(() => c(null, "127.0.0.1", 4));
            },
            resolve: (h, c) => queueMicrotask(() => c(null, ["127.0.0.1"])),
            resolve4: (h, c) => queueMicrotask(() => c(null, ["127.0.0.1"])),
            resolve6: (h, c) => queueMicrotask(() => c(null, ["::1"])),
        })"#,
        Some("[node:dns]"),
    )
    .unwrap()
}

fn build_dns_promises_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            lookup: async (_h) => ({ address: "127.0.0.1", family: 4 }),
            resolve: async (_h) => ["127.0.0.1"],
            resolve4: async (_h) => ["127.0.0.1"],
            resolve6: async (_h) => ["::1"],
        })"#,
        Some("[node:dns/promises]"),
    )
    .unwrap()
}

fn build_vm_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            // Use JSC's eval / Function constructor for the sandbox features.
            runInNewContext(code, _ctx, _opts) {
                return Function("return (" + String(code) + ")")();
            },
            runInThisContext(code, _opts) {
                return Function("return (" + String(code) + ")")();
            },
            runInContext(code, _ctx, _opts) {
                return Function("return (" + String(code) + ")")();
            },
            createContext(obj) { return obj || {}; },
            isContext: (_x) => false,
            compileFunction(code, params, _opts) {
                return new Function(...(params || []), code);
            },
            Script: class Script {
                constructor(code, _opts) { this._code = String(code); }
                runInThisContext() { return Function("return (" + this._code + ")")(); }
                runInNewContext(_ctx) { return this.runInThisContext(); }
                runInContext(_ctx) { return this.runInThisContext(); }
                createCachedData() { return new Uint8Array(0); }
            },
            constants: {},
        })"#,
        Some("[node:vm]"),
    )
    .unwrap()
}

fn build_punycode_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            decode: (s) => String(s),
            encode: (s) => String(s),
            toASCII: (s) => String(s),
            toUnicode: (s) => String(s),
            ucs2: { decode: (s) => Array.from(String(s)).map(c => c.codePointAt(0)), encode: (a) => String.fromCodePoint(...a) },
        })"#,
        Some("[node:punycode]"),
    )
    .unwrap()
}

fn build_tls_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            createServer: () => { throw new Error("node:tls createServer not implemented"); },
            connect: () => { throw new Error("node:tls connect not implemented"); },
            createSecureContext: (opts) => opts || {},
            checkServerIdentity: () => undefined,
            DEFAULT_ECDH_CURVE: "auto",
            DEFAULT_MAX_VERSION: "TLSv1.3",
            DEFAULT_MIN_VERSION: "TLSv1.2",
            CLIENT_RENEG_LIMIT: 3,
            CLIENT_RENEG_WINDOW: 600,
            rootCertificates: [],
        })"#,
        Some("[node:tls]"),
    )
    .unwrap()
}

fn build_trace_events_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            createTracing: () => ({ enable() {}, disable() {} }),
            getEnabledCategories: () => "",
        })"#,
        Some("[node:trace_events]"),
    )
    .unwrap()
}

fn build_inspector_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            open: () => {}, close: () => {}, url: () => undefined,
            console: globalThis.console,
            Session: class { constructor(){}; connect(){}; disconnect(){}; on(){}; off(){}; post(){} },
        })"#,
        Some("[node:inspector]"),
    )
    .unwrap()
}

fn build_wasi_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            WASI: class { constructor(){}; start(){ return 0; } initialize(){} getImportObject(){return {};} },
        })"#,
        Some("[node:wasi]"),
    )
    .unwrap()
}

fn build_dgram_stub<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    ctx.eval(
        r#"({
            __esModule: true,
            createSocket: () => { throw new Error('dgram not implemented'); },
        })"#,
        Some("[node:dgram]"),
    )
    .unwrap()
}
