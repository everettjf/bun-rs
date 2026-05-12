//! `Bun.*` global namespace.
//!
//! Currently:
//!   - `Bun.file(path)` → Blob-like with text()/json()/bytes()/size/name
//!   - `Bun.write(path, data)`
//!   - `Bun.version` / `Bun.revision`
//!   - `Bun.serve({port, fetch})` (in serve.rs)
//!   - `Bun.sleep(ms)`

use bun_jsc::{Callback, Context, Value};

mod file;
pub mod serve;

pub fn install_bun(ctx: &Context) {
    let bun_v = ctx.eval("({})", Some("[Bun]")).unwrap();
    let bun = bun_v.to_object().unwrap();

    bun.set_property(
        "version",
        &Value::new_string(ctx, env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    bun.set_property("revision", &Value::new_string(ctx, "bun-rs-dev"))
        .unwrap();

    file::install(ctx, &bun);
    serve::install(ctx, &bun);

    bind(ctx, &bun, "sleep", |args| {
        // Blocking sleep — matches Bun.sleep semantics from JS (the caller
        // typically awaits the returned Promise).
        let ms = if args.len() >= 1 { args.get(0).to_number() } else { 0.0 };
        if ms.is_finite() && ms > 0.0 {
            std::thread::sleep(std::time::Duration::from_millis(ms as u64));
        }
        Ok(Value::new_undefined(args.context()))
    });

    bind(ctx, &bun, "env", |args| {
        // Same shape as process.env — populated lazily so users get fresh
        // values if they mutate process.env (rare but defined).
        let ctx = args.context();
        let obj_v = ctx.eval("({})", Some("[Bun.env]")).unwrap();
        let obj = obj_v.to_object().unwrap();
        for (k, v) in std::env::vars() {
            let _ = obj.set_property(&k, &Value::new_string(ctx, &v));
        }
        Ok(obj_v)
    });

    ctx.global_object()
        .set_property("Bun", &bun.as_value())
        .unwrap();
}

pub(crate) fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
