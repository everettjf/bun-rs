//! Web platform APIs: URL, URLSearchParams, Headers, Request, Response, fetch.
//!
//! JS-side polyfills for the data classes; Rust handles parsing (`url` crate)
//! and HTTP (`ureq`). The polyfill is installed once via [`install_web`].

use bun_jsc::{Callback, Context, Value};

mod fetch;
mod url_parse;
mod websocket;

pub fn install_web(ctx: &Context) {
    // Rust helpers consumed by the JS polyfill.
    url_parse::install(ctx);
    fetch::install(ctx);
    websocket::install(ctx);

    // Streams first — the body polyfill below references ReadableStream
    // when wrapping fetch response bodies.
    ctx.eval(STREAMS, Some("[bun-streams-polyfill]"))
        .expect("install streams polyfill");

    // The polyfill (URL / URLSearchParams / Headers / Request / Response /
    // fetch wrapper). Keeping it in one big eval block makes initialization
    // trivial and predictable.
    ctx.eval(POLYFILL, Some("[bun-web-polyfill]"))
        .expect("install web polyfill");
}

const POLYFILL: &str = include_str!("polyfill.js");
const STREAMS: &str = include_str!("streams.js");

#[allow(dead_code)]
fn bind<F>(ctx: &Context, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    ctx.global_object()
        .set_property(name, &cb.value_in(ctx))
        .unwrap();
    std::mem::forget(cb);
}
