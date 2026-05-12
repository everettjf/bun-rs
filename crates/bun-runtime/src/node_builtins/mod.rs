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
pub mod os;
pub mod path;
pub mod querystring;
pub mod stream;
pub mod url;
pub mod util;

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
    let builder: fn(&Context) -> Value<'_> = match name {
        "path" | "node:path" => path::build,
        "os" | "node:os" => os::build,
        "fs" | "node:fs" => fs::build,
        "buffer" | "node:buffer" => buffer::build,
        "events" | "node:events" => events::build,
        "util" | "node:util" => util::build,
        "crypto" | "node:crypto" => crypto::build,
        "child_process" | "node:child_process" => child_process::build,
        "assert" | "node:assert" => assert::build,
        "querystring" | "node:querystring" => querystring::build,
        "url" | "node:url" => url::build,
        "stream" | "node:stream" => stream::build,
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
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}
