//! Rust closures as JS functions.
//!
//! Each callback is allocated as a `Box<Closure>` whose pointer goes into the
//! JS function object's private data slot. JSC invokes our trampoline →
//! we recover the closure pointer and run it. The class's finalizer drops
//! the box when the JS object is GC'd.

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::OnceLock;

use bun_jsc_sys as sys;

use crate::{Context, JsString, Object, Value};

/// Arguments passed to a Rust callback when invoked from JS.
pub struct CallbackArgs<'ctx> {
    ctx: &'ctx Context,
    args: &'ctx [sys::JSValueRef],
}

impl<'ctx> CallbackArgs<'ctx> {
    pub fn context(&self) -> &'ctx Context {
        self.ctx
    }

    pub fn len(&self) -> usize {
        self.args.len()
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    /// Get the i'th argument, or `undefined` if out of range.
    pub fn get(&self, i: usize) -> Value<'ctx> {
        match self.args.get(i) {
            Some(&raw) => unsafe { Value::from_raw(self.ctx, raw) },
            None => Value::new_undefined(self.ctx),
        }
    }

    pub fn all(&self) -> Vec<Value<'ctx>> {
        self.args
            .iter()
            .map(|&r| unsafe { Value::from_raw(self.ctx, r) })
            .collect()
    }
}

/// Type-erased closure pointer stored in JSC private data.
type ClosureFn = Box<dyn for<'a> Fn(CallbackArgs<'a>) -> Result<Value<'a>, String>>;

struct ClosureBox {
    f: ClosureFn,
}

/// A JS function backed by a Rust closure. `as_value` returns the underlying
/// JSC object; bind it onto globalThis or any other object.
pub struct Callback {
    raw: sys::JSObjectRef,
}

impl Callback {
    /// Create a callable JS function from a Rust closure.
    ///
    /// `name` is what `fn.name` returns in JS.
    pub fn new<F>(ctx: &Context, name: &str, f: F) -> Self
    where
        F: for<'a> Fn(CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
    {
        let class = callback_class();
        let bx = Box::new(ClosureBox { f: Box::new(f) });
        let data = Box::into_raw(bx) as *mut c_void;
        let raw = unsafe { sys::JSObjectMake(ctx.as_raw(), class, data) };
        assert!(!raw.is_null(), "JSObjectMake returned null");

        // Set `name` property so JS sees the right function name.
        if !name.is_empty() {
            let key = JsString::new("name");
            let val = JsString::new(name);
            let name_val = unsafe { sys::JSValueMakeString(ctx.as_raw(), val.as_raw()) };
            let mut exc: sys::JSValueRef = ptr::null();
            unsafe {
                sys::JSObjectSetProperty(
                    ctx.as_raw(),
                    raw,
                    key.as_raw(),
                    name_val,
                    sys::kJSPropertyAttributeReadOnly | sys::kJSPropertyAttributeDontEnum,
                    &mut exc,
                );
            }
        }

        Self { raw }
    }

    pub fn as_raw(&self) -> sys::JSObjectRef {
        self.raw
    }

    /// Get a [`Value`] view in a specific context.
    pub fn value_in<'ctx>(&self, ctx: &'ctx Context) -> Value<'ctx> {
        unsafe { Value::from_raw(ctx, self.raw as sys::JSValueRef) }
    }

    /// Get an [`Object`] view in a specific context.
    pub fn object_in<'ctx>(&self, ctx: &'ctx Context) -> Object<'ctx> {
        unsafe { Object::from_raw(ctx, self.raw) }
    }
}

// ── The shared "RustCallback" JSClass ────────────────────────────────────────
//
// One JSClass per process. It defines:
//   - callAsFunction → recover closure from private data, run it
//   - finalize       → drop the Box<ClosureBox> when GC reclaims the object

fn callback_class() -> sys::JSClassRef {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let v = CLASS.get_or_init(|| {
        let class_name = CString::new("RustCallback").unwrap();
        let mut def = sys::JSClassDefinition::EMPTY;
        def.className = class_name.as_ptr();
        def.callAsFunction = Some(trampoline);
        def.finalize = Some(finalize);
        // Keep class_name alive forever: leak it. The class itself is process-wide.
        let leaked_name = std::mem::ManuallyDrop::new(class_name);
        def.className = leaked_name.as_ptr();
        let raw = unsafe { sys::JSClassCreate(&def) };
        assert!(!raw.is_null(), "JSClassCreate failed");
        raw as usize
    });
    *v as sys::JSClassRef
}

unsafe extern "C" fn trampoline(
    ctx: sys::JSContextRef,
    function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    let priv_ptr = sys::JSObjectGetPrivate(function) as *const ClosureBox;
    if priv_ptr.is_null() {
        // Should never happen — bail out as undefined.
        return sys::JSValueMakeUndefined(ctx);
    }

    // Reconstruct a Context wrapper around the borrowed JSC context so we can
    // hand it to safe code. Crucially, we DO NOT take ownership — drop is a
    // no-op via std::mem::forget at the end.
    let borrowed_ctx = Context::from_borrowed(ctx);

    let args_slice: &[sys::JSValueRef] = if argument_count == 0 || arguments.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(arguments, argument_count)
    };

    let call_args = CallbackArgs {
        ctx: &borrowed_ctx,
        args: std::mem::transmute::<&[sys::JSValueRef], &[sys::JSValueRef]>(args_slice),
    };

    let closure = &(*priv_ptr).f;

    // Catch panics from user code so we don't unwind across the FFI boundary.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| closure(call_args)));

    let out = match result {
        Ok(Ok(v)) => v.as_raw(),
        Ok(Err(msg)) => {
            // Throw a JS Error with the user's message.
            let err = build_js_error(ctx, &msg);
            if !exception.is_null() {
                *exception = err;
            }
            sys::JSValueMakeUndefined(ctx)
        }
        Err(_) => {
            let err = build_js_error(ctx, "Rust callback panicked");
            if !exception.is_null() {
                *exception = err;
            }
            sys::JSValueMakeUndefined(ctx)
        }
    };

    std::mem::forget(borrowed_ctx);
    out
}

unsafe extern "C" fn finalize(object: sys::JSObjectRef) {
    let p = sys::JSObjectGetPrivate(object) as *mut ClosureBox;
    if !p.is_null() {
        let _ = Box::from_raw(p);
    }
}

unsafe fn build_js_error(ctx: sys::JSContextRef, msg: &str) -> sys::JSValueRef {
    let s = JsString::new(msg);
    let v = sys::JSValueMakeString(ctx, s.as_raw());
    let args = [v];
    let mut sub: sys::JSValueRef = ptr::null();
    sys::JSObjectMakeError(ctx, 1, args.as_ptr(), &mut sub) as sys::JSValueRef
}
