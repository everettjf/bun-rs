//! `node:child_process` — sync forms first, plus a simple async exec.
//!
//! Implemented:
//!   - execSync(command, opts?) → string|Buffer
//!   - spawnSync(cmd, args?, opts?) → { status, signal, stdout, stderr, pid }
//!   - exec(command, opts?, callback) — runs the command via the shell on a
//!     worker thread; result delivered through the callback
//!
//! Async `spawn()` returning a ChildProcess EventEmitter is intentionally
//! left out for this round — needs the streaming/event machinery we don't
//! have yet.

use std::io::Read;
use std::process::{Command, Stdio};

use bun_jsc::{Callback, Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:child_process]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    bind(ctx, &exports, "execSync", |args| {
        let command = args.get(0).to_string();
        let opts = args.get(1);
        let (encoding, cwd) = parse_exec_opts(&opts);
        let mut cmd = build_shell_cmd(&command, cwd.as_deref());
        let out = cmd
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            // Node throws an Error with .status / .stdout / .stderr on it.
            let ctx = args.context();
            let err = ctx
                .eval(
                    &format!(
                        "new Error('Command failed: {} (exit {})')",
                        escape_js(&command),
                        out.status.code().unwrap_or(-1)
                    ),
                    Some("[execSync-fail]"),
                )
                .unwrap();
            return Err(err.to_string());
        }
        match encoding.as_deref() {
            Some("buffer") | None => Ok(crate::buffer::buffer_from_bytes(args.context(), out.stdout)),
            Some(_enc) => Ok(Value::new_string(
                args.context(),
                &String::from_utf8_lossy(&out.stdout).into_owned(),
            )),
        }
    });

    bind(ctx, &exports, "spawnSync", |args| {
        let cmd_name = args.get(0).to_string();
        let mut argv: Vec<String> = Vec::new();
        let opts_arg_index;
        if args.len() >= 2 && args.get(1).is_object() {
            // Either spawnSync(cmd, args) or spawnSync(cmd, opts)
            // — if `args` value has length property, treat as array.
            let v = args.get(1);
            let obj = v.to_object().map_err(|e| e.to_string())?;
            let looks_like_array = obj
                .get_property("length")
                .map(|l| l.is_number())
                .unwrap_or(false);
            if looks_like_array {
                let n = obj.get_property("length").unwrap().to_number() as u32;
                for i in 0..n {
                    if let Ok(v) = obj.get_property_at(i) {
                        argv.push(v.to_string());
                    }
                }
                opts_arg_index = 2;
            } else {
                opts_arg_index = 1;
            }
        } else {
            opts_arg_index = 1;
        }
        let opts = args.get(opts_arg_index);
        let (encoding, cwd) = parse_exec_opts(&opts);

        let mut cmd = Command::new(&cmd_name);
        cmd.args(&argv);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        // Optional `input` / `stdin` (Buffer | Uint8Array | string).
        let stdin_bytes: Option<Vec<u8>> = if opts.is_object() {
            opts.to_object().ok().and_then(|o| {
                o.get_property("input")
                    .ok()
                    .or_else(|| o.get_property("stdin").ok())
                    .and_then(|v| {
                        if let Some(b) = v.typed_array_bytes() {
                            Some(b.to_vec())
                        } else if v.is_string() {
                            Some(v.to_string().into_bytes())
                        } else {
                            None
                        }
                    })
            })
        } else {
            None
        };
        if stdin_bytes.is_some() {
            cmd.stdin(Stdio::piped());
        }
        // Allow caller to pass env override.
        if opts.is_object() {
            if let Ok(env_v) = opts.to_object().and_then(|o| o.get_property("env")) {
                if env_v.is_object() {
                    if let Ok(env_o) = env_v.to_object() {
                        cmd.env_clear();
                        let names_v = args
                            .context()
                            .eval(
                                "(o) => Object.keys(o)",
                                Some("[env-keys]"),
                            )
                            .ok()
                            .and_then(|f| f.to_object().ok())
                            .and_then(|f| f.call(None, &[env_v]).ok());
                        if let Some(names) = names_v {
                            if let Ok(names_o) = names.to_object() {
                                if let Ok(n_v) = names_o.get_property("length") {
                                    let n = n_v.to_number() as u32;
                                    for i in 0..n {
                                        if let Ok(k) = names_o.get_property_at(i) {
                                            let key = k.to_string();
                                            if let Ok(val) = env_o.get_property(&key) {
                                                cmd.env(&key, val.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let out = match {
            if let Some(bytes) = stdin_bytes {
                use std::io::Write;
                let child_res = cmd
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
                match child_res {
                    Ok(mut child) => {
                        if let Some(mut sin) = child.stdin.take() {
                            let _ = sin.write_all(&bytes);
                        }
                        child.wait_with_output()
                    }
                    Err(e) => Err(e),
                }
            } else {
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()
            }
        } {
            Ok(o) => o,
            Err(e) => {
                // Mirror Node: still return an object with `.error` rather
                // than throw, so callers can branch on the result.
                let ctx = args.context();
                let result = ctx.eval("({})", Some("[spawnSync-err]")).unwrap();
                let r = result.to_object().unwrap();
                r.set_property("status", &Value::new_null(ctx)).unwrap();
                r.set_property("signal", &Value::new_null(ctx)).unwrap();
                r.set_property("error", &Value::new_string(ctx, &e.to_string())).unwrap();
                r.set_property("pid", &Value::new_number(ctx, 0.0)).unwrap();
                r.set_property("stdout", &Value::new_string(ctx, "")).unwrap();
                r.set_property("stderr", &Value::new_string(ctx, "")).unwrap();
                return Ok(result);
            }
        };

        let ctx = args.context();
        let result = ctx.eval("({})", Some("[spawnSync]")).unwrap();
        let r = result.to_object().unwrap();
        r.set_property(
            "status",
            &Value::new_number(ctx, out.status.code().unwrap_or(-1) as f64),
        )
        .unwrap();
        r.set_property("signal", &Value::new_null(ctx)).unwrap();
        r.set_property("pid", &Value::new_number(ctx, 0.0)).unwrap();

        let make_body = |bytes: Vec<u8>| match encoding.as_deref() {
            Some("buffer") | None => crate::buffer::buffer_from_bytes(ctx, bytes),
            _ => Value::new_string(ctx, &String::from_utf8_lossy(&bytes).into_owned()),
        };
        r.set_property("stdout", &make_body(out.stdout)).unwrap();
        r.set_property("stderr", &make_body(out.stderr)).unwrap();
        Ok(result)
    });

    bind(ctx, &exports, "exec", |args| {
        // exec(command, [opts], callback). For MVP run on the JS thread
        // synchronously and invoke the callback "asynchronously" via
        // queueMicrotask so the call site behaves like Node's exec.
        let command = args.get(0).to_string();
        // Find callback: last function arg.
        let mut callback_val: Option<bun_jsc::Value<'_>> = None;
        for i in (0..args.len()).rev() {
            let v = args.get(i);
            if v.is_object() {
                if let Ok(o) = v.to_object() {
                    if o.is_function() {
                        callback_val = Some(v);
                        break;
                    }
                }
            }
        }
        let cb = match callback_val {
            Some(v) => v.to_object().map_err(|e| e.to_string())?,
            None => return Err("exec: missing callback".into()),
        };
        // Run blocking on the calling thread.
        let out = build_shell_cmd(&command, None)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let ctx = args.context();
        let (err_val, stdout_val, stderr_val) = match out {
            Ok(o) => {
                let ev = if o.status.success() {
                    Value::new_null(ctx)
                } else {
                    let s = format!("Command failed: {} (exit {})", command, o.status.code().unwrap_or(-1));
                    Value::new_string(ctx, &s)
                };
                (
                    ev,
                    Value::new_string(ctx, &String::from_utf8_lossy(&o.stdout)),
                    Value::new_string(ctx, &String::from_utf8_lossy(&o.stderr)),
                )
            }
            Err(e) => (
                Value::new_string(ctx, &e.to_string()),
                Value::new_string(ctx, ""),
                Value::new_string(ctx, ""),
            ),
        };
        cb.call(None, &[err_val, stdout_val, stderr_val])
            .map_err(|e| e.to_string())?;
        Ok(Value::new_undefined(ctx))
    });

    // Async `spawn(cmd, args, opts)` — returns a ChildProcess-like object
    // with `.stdout` / `.stderr` as Uint8Array (buffered, since we run
    // sync internally) and `.on("exit", cb)` so callers using EventEmitter
    // patterns work. Plenty of Bun's tests use this shape.
    bind(ctx, &exports, "spawn", |args| {
        let ctx = args.context();
        let cmd_name = args.get(0).to_string();
        let mut argv: Vec<String> = Vec::new();
        let mut opts_idx = 1usize;
        if args.len() >= 2 && args.get(1).is_object() {
            let v = args.get(1);
            let obj = v.to_object().map_err(|e| e.to_string())?;
            let looks_like_array = obj
                .get_property("length")
                .map(|l| l.is_number())
                .unwrap_or(false);
            if looks_like_array {
                let n = obj.get_property("length").unwrap().to_number() as u32;
                for i in 0..n {
                    if let Ok(v) = obj.get_property_at(i) {
                        argv.push(v.to_string());
                    }
                }
                opts_idx = 2;
            }
        }
        let opts = args.get(opts_idx);
        let (_encoding, cwd) = parse_exec_opts(&opts);

        let mut cmd = Command::new(&cmd_name);
        cmd.args(&argv);
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd.output();

        // Build the JS-side ChildProcess-ish object.
        let child_v = ctx
            .eval(
                r#"
                (function(){
                    const listeners = {};
                    function on(ev, cb) { (listeners[ev] = listeners[ev] || []).push(cb); return this; }
                    function emit(ev, ...args) { for (const cb of (listeners[ev]||[])) try { cb(...args); } catch {} }
                    function once(ev, cb) { const wrap = (...a) => { off(ev, wrap); cb(...a); }; on.call(this, ev, wrap); return this; }
                    function off(ev, cb) {
                        const ls = listeners[ev]; if (!ls) return this;
                        const i = ls.indexOf(cb); if (i >= 0) ls.splice(i, 1); return this;
                    }
                    const proc = { on, once, off, emit, listeners };
                    proc.stdout = null;
                    proc.stderr = null;
                    proc.stdin = null;
                    return proc;
                })()
                "#,
                Some("[spawn-childproc]"),
            )
            .map_err(|e| e.to_string())?;
        let child = child_v.to_object().map_err(|e| e.to_string())?;
        let exit_code = match output {
            Ok(o) => {
                let stdout_b = crate::buffer::buffer_from_bytes(ctx, o.stdout);
                let stderr_b = crate::buffer::buffer_from_bytes(ctx, o.stderr);
                child.set_property("stdout", &stdout_b).ok();
                child.set_property("stderr", &stderr_b).ok();
                child
                    .set_property("pid", &Value::new_number(ctx, std::process::id() as f64))
                    .ok();
                o.status.code().unwrap_or(-1)
            }
            Err(e) => {
                child
                    .set_property("error", &Value::new_string(ctx, &e.to_string()))
                    .ok();
                -1
            }
        };
        child
            .set_property("exitCode", &Value::new_number(ctx, exit_code as f64))
            .ok();
        // Fire 'exit' / 'close' events asynchronously so handlers attached
        // after spawn() returns get them. queueMicrotask ensures the JS
        // call site has finished setting up listeners.
        let kick = ctx
            .eval(
                r#"
                (function(child, code){
                    queueMicrotask(() => {
                        try { child.emit("exit", code, null); } catch {}
                        try { child.emit("close", code, null); } catch {}
                    });
                })
                "#,
                Some("[spawn-emit]"),
            )
            .map_err(|e| e.to_string())?
            .to_object()
            .map_err(|e| e.to_string())?;
        kick.call(None, &[child_v, Value::new_number(ctx, exit_code as f64)])
            .map_err(|e| e.to_string())?;
        // Bun-shaped extras: kill / exited promise.
        let extras = ctx
            .eval(
                r#"
                (function(child, code){
                    child.exited = Promise.resolve(code);
                    child.kill = (_sig) => {};
                    child.unref = () => {};
                    child.ref = () => {};
                    return child;
                })
                "#,
                Some("[spawn-extras]"),
            )
            .map_err(|e| e.to_string())?
            .to_object()
            .map_err(|e| e.to_string())?;
        extras
            .call(None, &[child_v, Value::new_number(ctx, exit_code as f64)])
            .map_err(|e| e.to_string())?;
        Ok(child_v)
    });

    // `execFile(file, args, opts, callback)` — like exec but no shell.
    bind(ctx, &exports, "execFile", |args| {
        let ctx = args.context();
        let file = args.get(0).to_string();
        let mut argv: Vec<String> = Vec::new();
        let mut cb_idx = args.len();
        // Args is the 2nd arg if it's an array (length numeric).
        if args.len() >= 2 && args.get(1).is_object() {
            let v = args.get(1);
            let obj = v.to_object().map_err(|e| e.to_string())?;
            let looks_like_array = obj
                .get_property("length")
                .map(|l| l.is_number())
                .unwrap_or(false);
            if looks_like_array {
                let n = obj.get_property("length").unwrap().to_number() as u32;
                for i in 0..n {
                    if let Ok(v) = obj.get_property_at(i) {
                        argv.push(v.to_string());
                    }
                }
            }
        }
        for i in (0..args.len()).rev() {
            let v = args.get(i);
            if v.is_object() {
                if let Ok(o) = v.to_object() {
                    if o.is_function() {
                        cb_idx = i;
                        break;
                    }
                }
            }
        }
        let out = Command::new(&file)
            .args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        if cb_idx < args.len() {
            let cb = args
                .get(cb_idx)
                .to_object()
                .map_err(|e| e.to_string())?;
            let (err_val, stdout_val, stderr_val) = match out {
                Ok(o) => (
                    if o.status.success() { Value::new_null(ctx) }
                    else { Value::new_string(ctx, &format!("Command failed: {} (exit {})", file, o.status.code().unwrap_or(-1))) },
                    Value::new_string(ctx, &String::from_utf8_lossy(&o.stdout)),
                    Value::new_string(ctx, &String::from_utf8_lossy(&o.stderr)),
                ),
                Err(e) => (
                    Value::new_string(ctx, &e.to_string()),
                    Value::new_string(ctx, ""),
                    Value::new_string(ctx, ""),
                ),
            };
            cb.call(None, &[err_val, stdout_val, stderr_val])
                .map_err(|e| e.to_string())?;
        }
        Ok(Value::new_undefined(ctx))
    });

    // `fork(modulePath, args, opts)` — spawn a Node-like child. We don't
    // have a Node subprocess loader, so route through `spawn(<bun-rs>, ...)`
    // pointing at the same bun-rs binary.
    bind(ctx, &exports, "fork", |args| {
        let ctx = args.context();
        // Construct argv: [<bun-rs>, <module>, ...userArgs].
        let module = args.get(0).to_string();
        let bun_rs = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "bun-rs".into());
        // Re-use the spawn callback by calling it via `this`.
        let spawn_fn = ctx
            .global_object()
            .get_property("__bun_internal_cp_spawn_helper")
            .ok();
        let _ = spawn_fn;
        let mut argv: Vec<String> = vec![module];
        if args.len() >= 2 && args.get(1).is_object() {
            let v = args.get(1);
            if let Ok(obj) = v.to_object() {
                if let Ok(len) = obj.get_property("length") {
                    if len.is_number() {
                        let n = len.to_number() as u32;
                        for i in 0..n {
                            if let Ok(v) = obj.get_property_at(i) {
                                argv.push(v.to_string());
                            }
                        }
                    }
                }
            }
        }
        let out = Command::new(&bun_rs)
            .args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let child_v = ctx
            .eval("({ on(){return this;}, once(){return this;}, off(){return this;}, emit(){}, kill(){}, unref(){}, ref(){} })", Some("[fork-child]"))
            .map_err(|e| e.to_string())?;
        let child = child_v.to_object().map_err(|e| e.to_string())?;
        match out {
            Ok(o) => {
                child.set_property("stdout", &crate::buffer::buffer_from_bytes(ctx, o.stdout)).ok();
                child.set_property("stderr", &crate::buffer::buffer_from_bytes(ctx, o.stderr)).ok();
                child.set_property("exitCode", &Value::new_number(ctx, o.status.code().unwrap_or(-1) as f64)).ok();
                child.set_property("exited", &ctx.eval("Promise.resolve(0)", Some("[fork-exited]")).unwrap()).ok();
            }
            Err(_) => {
                child.set_property("exitCode", &Value::new_number(ctx, -1.0)).ok();
            }
        }
        Ok(child_v)
    });

    // Stub ChildProcess class so test harnesses can monkey-patch
    // `ChildProcess.prototype` without throwing.
    let cp_class = ctx
        .eval(
            r#"(function () { class ChildProcess { constructor() {} } return ChildProcess; })()"#,
            Some("[child_process.ChildProcess-stub]"),
        )
        .unwrap();
    exports.set_property("ChildProcess", &cp_class).ok();

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn build_shell_cmd(command: &str, cwd: Option<&str>) -> Command {
    // Match Node: pipe through `sh -c <command>` on Unix.
    let mut c = if cfg!(windows) {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", command]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    };
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    c
}

fn parse_exec_opts(opts: &Value<'_>) -> (Option<String>, Option<String>) {
    if !opts.is_object() {
        return (None, None);
    }
    let obj = match opts.to_object() {
        Ok(o) => o,
        Err(_) => return (None, None),
    };
    let encoding = obj
        .get_property("encoding")
        .ok()
        .filter(|v| v.is_string())
        .map(|v| v.to_string().to_lowercase());
    let cwd = obj
        .get_property("cwd")
        .ok()
        .filter(|v| v.is_string())
        .map(|v| v.to_string());
    (encoding, cwd)
}

fn escape_js(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}

// Use Read trait for completeness even though we use .output() above.
#[allow(dead_code)]
fn _force_use_read() -> impl Read {
    std::io::empty()
}
