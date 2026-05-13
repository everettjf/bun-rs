//! `globalThis.console` — log/error/warn/info/debug.
//!
//! Output mirrors Node/Bun: stdout for `log`/`info`/`debug`/`trace`,
//! stderr for `warn`/`error`. Each argument is formatted with
//! [`Value::to_string`] and joined by a space; line terminated by `\n`.

use bun_jsc::{Callback, CallbackArgs, Context, Value};

pub fn install_console(ctx: &Context) {
    let console = build_console(ctx);
    let global = ctx.global_object();
    global
        .set_property("console", &console)
        .expect("set globalThis.console");
}

fn build_console<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    // Build an empty object and attach methods.
    let obj_val = ctx
        .eval("({})", Some("[console]"))
        .expect("create plain object");
    let obj = obj_val.to_object().expect("to_object");

    for (name, stderr) in [
        ("log", false),
        ("info", false),
        ("debug", false),
        ("trace", false),
        ("dir", false),
        ("warn", true),
        ("error", true),
    ] {
        let cb = Callback::new(ctx, name, move |args| {
            let line = format_args_line(&args);
            if stderr {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
            Ok(Value::new_undefined(args.context()))
        });
        obj.set_property(name, &cb.value_in(ctx))
            .expect("set console.method");
        std::mem::forget(cb);
    }

    // console.time / .timeEnd / .timeLog — timestamped labeled timers.
    // Stored in a JS Map on console itself so multiple labels are tracked.
    let _ = ctx.eval(
        r#"
        (function (c) {
            const timers = new Map();
            c.time = function (label) {
                timers.set(label || "default", performance.now());
            };
            c.timeEnd = function (label) {
                const k = label || "default";
                const t0 = timers.get(k);
                if (t0 === undefined) { console.warn("No such label: " + k); return; }
                const dt = performance.now() - t0;
                timers.delete(k);
                console.log(k + ": " + dt.toFixed(3) + "ms");
            };
            c.timeLog = function (label, ...rest) {
                const k = label || "default";
                const t0 = timers.get(k);
                if (t0 === undefined) { console.warn("No such label: " + k); return; }
                const dt = performance.now() - t0;
                console.log(k + ": " + dt.toFixed(3) + "ms", ...rest);
            };
            c.count = (function () {
                const counts = new Map();
                return function (label) {
                    const k = label || "default";
                    const n = (counts.get(k) || 0) + 1;
                    counts.set(k, n);
                    console.log(k + ": " + n);
                };
            })();
            c.countReset = function (label) { /* no-op */ };
            c.group = function (...args) { console.log(...args); };
            c.groupCollapsed = c.group;
            c.groupEnd = function () {};
            c.assert = function (cond, ...args) {
                if (!cond) console.error("Assertion failed:", ...args);
            };
            c.table = function (data) { console.log(data); };
            c.profile = function () {};
            c.profileEnd = function () {};
            c.timeStamp = function () {};
            c.clear = function () {};
        })(globalThis.console);
        "#,
        Some("[console-extra]"),
    );

    obj.as_value()
}

fn format_args_line(args: &CallbackArgs<'_>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(args.len());
    for i in 0..args.len() {
        parts.push(format_value(args.get(i)));
    }
    parts.join(" ")
}

fn format_value(v: Value<'_>) -> String {
    use bun_jsc::ValueKind;
    match v.kind() {
        ValueKind::Undefined => "undefined".to_string(),
        ValueKind::Null => "null".to_string(),
        // For objects, prefer JSON.stringify for readability.
        // Functions / Errors fall back to toString.
        ValueKind::Object => v.to_json(0).unwrap_or_else(|_| v.to_string()),
        _ => v.to_string(),
    }
}
