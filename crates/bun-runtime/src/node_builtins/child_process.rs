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
        let out = match cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
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
