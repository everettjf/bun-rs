//! `node:buffer` — re-exports the Buffer global as a module.

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:buffer]")).unwrap();
    let exports = exports_v.to_object().unwrap();
    let buffer_class = ctx
        .global_object()
        .get_property("Buffer")
        .expect("Buffer global");
    exports.set_property("Buffer", &buffer_class).unwrap();
    // Bun's node:buffer also exports a Blob shim; we don't have Blob yet.
    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}
