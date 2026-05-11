//! Module loader runtime: binds `__bun_require(spec, importer)` onto
//! `globalThis`, maintains a path-keyed cache, wraps each module body in an
//! IIFE, and recursively loads dependencies on demand.
//!
//! Phase 1: synchronous loader, static `import`/`export` only. No dynamic
//! `import()` or top-level await.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use bun_loader::Resolver;

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, sys::JSValueRef>> = RefCell::new(HashMap::new());
    static RESOLVER: Resolver = Resolver::new();
}

/// Bind `globalThis.__bun_require(spec, importerPath)` to the Rust loader.
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
}

/// Public entry: load and evaluate `path` as the program's main module.
pub fn run_entry(ctx: &Context, path: &Path) -> Result<(), LoaderRuntimeError> {
    let abs = path
        .canonicalize()
        .map_err(|e| LoaderRuntimeError::Io(path.to_path_buf(), e))?;
    // Reuse the same machinery the JS-side `__bun_require` would.
    // No importer needed — we already have an absolute path.
    load_module(ctx, abs.to_str().unwrap_or(""), &abs)?;
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

    // Wrap in IIFE. Use a fresh ident to avoid colliding with module-level vars.
    let wrapped = format!(
        "(function (__exports, __bun_require, __filename, __dirname) {{\n{}\n}})",
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

    factory
        .call(None, &[exports_val, require_fn, filename, dirname])
        .map_err(|e| LoaderRuntimeError::Eval {
            path: abs.clone(),
            message: e.to_string(),
        })?;

    Ok(exports_val)
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
