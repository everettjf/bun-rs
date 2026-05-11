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

fn argv_slot() -> &'static Mutex<Vec<String>> {
    static SLOT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Vec::new()))
}

fn set_argv(v: Vec<String>) {
    if let Ok(mut g) = argv_slot().lock() {
        *g = v;
    }
}
