//! Module loader runtime: binds `__bun_require(spec, importer)` onto
//! `globalThis`, maintains a path-keyed cache, wraps each module body in an
//! IIFE, and recursively loads dependencies on demand.
//!
//! Phase 1: synchronous loader, static `import`/`export` only. No dynamic
//! `import()` or top-level await.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use bun_loader::Resolver;

use crate::timers::run_one_tick;

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, sys::JSValueRef>> = RefCell::new(HashMap::new());
    static RESOLVER: Resolver = Resolver::new();
}

/// Bind `globalThis.__bun_require(spec, importerPath)` to the Rust loader,
/// and install a JS helper that chains the module body promise to resolve
/// with the module's `__exports`.
pub fn install_module_loader(ctx: &Context) {
    let cb = Callback::new(ctx, "__bun_require", |args| {
        if args.len() < 1 {
            return Err("__bun_require requires (spec, importerPath)".to_string());
        }
        let spec = args.get(0).to_string();
        let importer = if args.len() >= 2 {
            args.get(1).to_string()
        } else {
            String::new()
        };
        let importer_path = if importer.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("anon.js")
        } else {
            PathBuf::from(importer)
        };
        load_module(args.context(), &spec, &importer_path).map_err(|e| e.to_string())
    });
    ctx.global_object()
        .set_property("__bun_require", &cb.value_in(ctx))
        .expect("install __bun_require");
    std::mem::forget(cb);

    // Helper used by load_module to map a body Promise to the module's
    // exports object after the body finishes evaluating. Defined in JS so
    // `then` chaining is native.
    ctx.eval(
        r#"
        globalThis.__bun_chain_exports = function(bodyPromise, exports) {
            return bodyPromise.then(() => exports);
        };
        "#,
        Some("[bun-chain-exports]"),
    )
    .expect("install __bun_chain_exports");
}

/// Public entry: load and evaluate `path` as the program's main module.
///
/// `load_module` returns a Promise (or, for cache hits, a plain value).
/// We must drive the event loop until it settles before returning to the CLI,
/// otherwise top-level `await` and dynamic `import()` wouldn't finish.
pub fn run_entry(ctx: &Context, path: &Path) -> Result<(), LoaderRuntimeError> {
    let abs = path
        .canonicalize()
        .map_err(|e| LoaderRuntimeError::Io(path.to_path_buf(), e))?;
    let result = load_module(ctx, abs.to_str().unwrap_or(""), &abs)?;
    await_promise(ctx, result).map_err(|e| LoaderRuntimeError::Eval {
        path: abs.clone(),
        message: e,
    })?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum LoaderRuntimeError {
    #[error(transparent)]
    Loader(#[from] bun_loader::LoaderError),
    #[error("could not read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("eval failed in {path}: {message}")]
    Eval { path: PathBuf, message: String },
    #[error(transparent)]
    Resolve(#[from] bun_loader::ResolveError),
}

/// Core: resolve spec → abs path → cache check → prepare → eval IIFE.
fn load_module<'ctx>(
    ctx: &'ctx Context,
    spec: &str,
    importer: &Path,
) -> Result<Value<'ctx>, LoaderRuntimeError> {
    // Routing: `node:foo` and bare `foo` (when foo is a Node builtin) go
    // through `node_builtins::load`. Returning here means the JS-side
    // `await __bun_require("node:foo", ...)` resolves with the builtin's
    // exports object directly, no file I/O.
    if let Some(name) = spec.strip_prefix("node:") {
        if let Some(v) = crate::node_builtins::load(ctx, name) {
            return Ok(v);
        }
        return Err(LoaderRuntimeError::Resolve(
            bun_loader::ResolveError::NotFound {
                spec: spec.to_string(),
                from: importer.to_path_buf(),
            },
        ));
    }
    if let Some(v) = crate::node_builtins::load(ctx, spec) {
        // Allow bare `import "path"` etc. as a convenience.
        return Ok(v);
    }

    // Absolute paths bypass resolution.
    let abs: PathBuf = if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        RESOLVER.with(|r| r.resolve(spec, importer))?
    };

    // Cache hit?
    let cached = CACHE.with(|c| c.borrow().get(&abs).copied());
    if let Some(raw) = cached {
        return Ok(unsafe { Value::from_raw_for_runtime(ctx, raw) });
    }

    // Make empty exports object and stash BEFORE running the body so cycles
    // see a partial.
    let exports_val = ctx
        .eval("({})", Some("[module-exports]"))
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;
    let exports_raw = exports_val.as_raw();
    unsafe {
        sys::JSValueProtect(ctx.as_raw(), exports_raw);
    }
    CACHE.with(|c| c.borrow_mut().insert(abs.clone(), exports_raw));

    // Prepare (read + transpile + rewrite ESM).
    let prepared = bun_loader::prepare(&abs)?;
    let parent = abs.parent().unwrap_or_else(|| Path::new("."));

    // Wrap in an async function. The body uses `await __bun_require(...)`
    // for static imports, and bare `__bun_require(...)` is itself a thenable
    // call so `import()` (rewritten to `__bun_require()`) works too.
    // `__bun_meta` carries `import.meta` (rewritten in the source).
    let wrapped = format!(
        "(async function (__exports, __bun_require, __filename, __dirname, __bun_meta) {{\n{}\n}})",
        prepared.rewritten
    );

    // Eval the wrapper to get a callable function.
    let factory_val = ctx
        .eval(&wrapped, abs.to_str())
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;
    let factory = factory_val
        .to_object()
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: format!("factory not a function: {e}"),
        })?;

    // Fetch the global __bun_require to pass into the module body.
    let global = ctx.global_object();
    let require_fn = global
        .get_property("__bun_require")
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;

    let filename = Value::new_string(ctx, abs.to_str().unwrap_or(""));
    let dirname = Value::new_string(ctx, parent.to_str().unwrap_or(""));
    let meta = build_import_meta(ctx, &abs);

    let body_promise = factory
        .call(None, &[exports_val, require_fn, filename, dirname, meta])
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;

    // Chain `bodyPromise.then(() => exports)` in JS so the caller's
    // `await __bun_require(...)` sees the exports object only after the body
    // has finished evaluating. For pure-sync bodies the chain resolves in the
    // next microtask checkpoint — which `await_promise` (called at entry)
    // forces via a no-op eval.
    let chain_fn = ctx
        .global_object()
        .get_property("__bun_chain_exports")
        .and_then(|v| v.to_object())
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;
    let chained = chain_fn
        .call(None, &[body_promise, exports_val])
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;

    Ok(chained)
}

fn build_import_meta<'ctx>(ctx: &'ctx Context, abs: &Path) -> Value<'ctx> {
    let url = path_to_file_url(abs);
    let obj_val = ctx
        .eval("({})", Some("[import-meta]"))
        .expect("create import.meta obj");
    let obj = obj_val.to_object().expect("to_object");
    obj.set_property("url", &Value::new_string(ctx, &url))
        .expect("set url");
    obj.set_property("main", &Value::new_bool(ctx, false))
        .expect("set main");
    // Spec also defines `dirname`/`filename` in Node ≥ 21.2.
    obj.set_property("dirname", &Value::new_string(ctx, abs.parent().map_or("", |p| p.to_str().unwrap_or(""))))
        .expect("set dirname");
    obj.set_property("filename", &Value::new_string(ctx, abs.to_str().unwrap_or("")))
        .expect("set filename");
    obj.as_value()
}

fn path_to_file_url(p: &Path) -> String {
    // Minimal RFC-8089 file URL builder: percent-encode unsafe bytes.
    let s = p.to_string_lossy();
    let mut out = String::from("file://");
    for c in s.chars() {
        match c {
            '/' | '.' | '-' | '_' | '~' => out.push(c),
            c if c.is_ascii_alphanumeric() => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}

/// Wait for a JS Promise to settle, driving the event loop in the meantime.
///
/// Strategy:
///   - Attach `.then(resolveCb, rejectCb)` to the promise, where the
///     callbacks store the outcome in a thread-local-shared `Rc<RefCell<…>>`.
///   - Spin: while outcome is `None`, fire timers (existing event loop).
///   - On resolve, return the resolved value. On reject, return its string.
///
/// Microtasks drain naturally between JS calls, so a promise resolved by
/// pure JS work settles before the loop even starts iterating.
pub fn await_promise<'ctx>(
    ctx: &'ctx Context,
    promise: Value<'ctx>,
) -> Result<Value<'ctx>, String> {
    // Fast path: the value isn't a Promise (or any thenable). Just return it.
    if !promise.is_object() {
        return Ok(promise);
    }
    let promise_obj = promise.to_object().map_err(|e| e.to_string())?;
    let then_val = match promise_obj.get_property("then") {
        Ok(v) => v,
        Err(_) => return Ok(promise),
    };
    if !then_val.is_object() {
        return Ok(promise);
    }
    let then_obj = then_val.to_object().map_err(|e| e.to_string())?;
    if !then_obj.is_function() {
        return Ok(promise);
    }

    #[derive(Clone)]
    enum Outcome {
        Resolved(sys::JSValueRef),
        Rejected(String),
    }
    let outcome: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));

    let resolve_clone = outcome.clone();
    let resolve_cb = Callback::new(ctx, "__bun_resolve", move |args| {
        let v = args.get(0);
        unsafe { sys::JSValueProtect(v.context().as_raw(), v.as_raw()) };
        *resolve_clone.borrow_mut() = Some(Outcome::Resolved(v.as_raw()));
        Ok(Value::new_undefined(args.context()))
    });

    let reject_clone = outcome.clone();
    let reject_cb = Callback::new(ctx, "__bun_reject", move |args| {
        let s = args.get(0).to_string();
        *reject_clone.borrow_mut() = Some(Outcome::Rejected(s));
        Ok(Value::new_undefined(args.context()))
    });

    then_obj
        .call(
            Some(promise_obj),
            &[resolve_cb.value_in(ctx), reject_cb.value_in(ctx)],
        )
        .map_err(|e| e.to_string())?;

    // The callbacks now live on the promise's reaction chain. JSC keeps
    // them alive; we leak our wrappers.
    std::mem::forget(resolve_cb);
    std::mem::forget(reject_cb);

    while outcome.borrow().is_none() {
        // Nudge microtask drain: a no-op script forces JSC to flush its
        // queue. Cheap and reliable.
        let _ = ctx.eval("undefined", Some("[microtask-drain]"));
        if outcome.borrow().is_some() {
            break;
        }
        // Pump async-runtime completions (e.g. resolved fetch).
        let async_did = crate::async_rt::drain_js_tasks(ctx) > 0;
        if async_did {
            continue;
        }
        // Pump pending Bun.serve requests — important for cases like
        // `await fetch("http://127.0.0.1:" + server.port)` where the
        // server can't respond unless we drain its queue.
        let server_did =
            crate::bun_api::serve::poll_one(ctx, std::time::Duration::from_millis(5));
        if server_did {
            continue;
        }
        // Otherwise fire next timer.
        if !run_one_tick(ctx) {
            // No timer, no async task we could deliver, no completion —
            // and yet the promise is pending. If async work is still in
            // flight we just yield briefly and try again.
            if crate::async_rt::has_pending_async()
                || crate::bun_api::serve::any_active()
            {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            return Err(
                "event loop deadlocked: promise pending with no work to do".to_string()
            );
        }
    }

    let taken = outcome.borrow_mut().take().unwrap();
    match taken {
        Outcome::Resolved(raw) => {
            // We protected on capture; caller doesn't need to keep us alive.
            // Leave the protect in place — the caller's Value lifetime is the
            // borrow of `ctx`, and any cache insertion has already happened
            // in load_module before we got here. For the entry, the resolved
            // value is `undefined`; ditto for pure side-effect modules.
            Ok(unsafe { Value::from_raw_public(ctx, raw) })
        }
        Outcome::Rejected(s) => Err(s),
    }
}

// We need a way to make a Value<'ctx> out of a raw pointer. `Value::from_raw`
// is pub(crate) inside bun-jsc, so we use a small helper on Value's adapter
// for use by `bun-runtime` only. Expose it via a wrapper module.
mod _adapter {
    use super::*;
    pub trait FromRawForRuntime<'ctx> {
        unsafe fn from_raw_for_runtime(ctx: &'ctx Context, raw: sys::JSValueRef) -> Self;
    }

    impl<'ctx> FromRawForRuntime<'ctx> for Value<'ctx> {
        unsafe fn from_raw_for_runtime(ctx: &'ctx Context, raw: sys::JSValueRef) -> Self {
            // bun-jsc exposes the raw pointer via as_raw, so we can mint a Value
            // by constructing one through a known-safe round trip: store globally
            // via JSObjectSetPropertyAtIndex on a hidden array... too convoluted.
            // Instead, use bun-jsc's public `Value::from_raw_public` helper.
            bun_jsc::Value::from_raw_public(ctx, raw)
        }
    }
}
use _adapter::FromRawForRuntime;
