//! Safe Rust wrapper over the JavaScriptCore C API.
//!
//! Lifetimes: every [`Value`] / [`Object`] borrows its [`Context`].
//! The context owns the JS heap; dropping it releases everything reachable.
//!
//! Exception handling: any JSC call that can throw returns [`Result<T, JsException>`].
//! A `JsException` carries a *protected* value so it survives across drains.

#![allow(clippy::missing_safety_doc)]

use std::marker::PhantomData;
use std::ptr;

use bun_jsc_sys as sys;

mod string;
pub use string::JsString;

mod value;
pub use value::{Value, ValueKind};

mod object;
pub use object::Object;

mod exception;
pub use exception::JsException;

mod callback;
pub use callback::{Callback, CallbackArgs};

// ── Context ──────────────────────────────────────────────────────────────────

/// Owned JavaScriptCore global context. RAII; releases JSC heap on drop.
///
/// Not `Send`/`Sync` — JSC contexts must stay on the thread they were created on.
pub struct Context {
    raw: sys::JSGlobalContextRef,
    _not_send: PhantomData<*mut ()>,
}

impl Context {
    /// Create a fresh global JS context with a default global object.
    pub fn new() -> Self {
        let raw = unsafe { sys::JSGlobalContextCreate(ptr::null_mut()) };
        assert!(!raw.is_null(), "JSGlobalContextCreate returned null");
        Self {
            raw,
            _not_send: PhantomData,
        }
    }

    /// Build a non-owning `Context` view from a raw JSC pointer. Used by the
    /// callback trampoline — caller MUST `mem::forget` the result to skip the
    /// Drop (we don't own the ref).
    pub(crate) unsafe fn from_borrowed(ctx: sys::JSContextRef) -> Self {
        Self {
            raw: ctx as sys::JSGlobalContextRef,
            _not_send: PhantomData,
        }
    }

    /// Raw context for use with sys calls. Lives as long as `&self`.
    pub fn as_raw(&self) -> sys::JSContextRef {
        self.raw as sys::JSContextRef
    }

    /// Raw global context (mutable handle).
    pub fn as_global_raw(&self) -> sys::JSGlobalContextRef {
        self.raw
    }

    /// The global object (`globalThis` in JS).
    pub fn global_object(&self) -> Object<'_> {
        let raw = unsafe { sys::JSContextGetGlobalObject(self.as_raw()) };
        unsafe { Object::from_raw(self, raw) }
    }

    /// Enable Safari Web Inspector attachment (no-op outside macOS).
    pub fn set_inspectable(&self, inspectable: bool) {
        unsafe { sys::JSGlobalContextSetInspectable(self.raw, inspectable) }
    }

    /// Force a GC sweep. Mostly useful in tests.
    pub fn collect_garbage(&self) {
        unsafe { sys::JSGarbageCollect(self.as_raw()) }
    }

    /// Evaluate a script. `source_url` is what appears in stack traces.
    pub fn eval(&self, source: &str, source_url: Option<&str>) -> Result<Value<'_>, JsException> {
        let script = JsString::new(source);
        let url = source_url.map(JsString::new);
        let url_ref = url.as_ref().map_or(ptr::null_mut(), JsString::as_raw);
        let mut exception: sys::JSValueRef = ptr::null();
        let raw = unsafe {
            sys::JSEvaluateScript(
                self.as_raw(),
                script.as_raw(),
                ptr::null_mut(),
                url_ref,
                1,
                &mut exception,
            )
        };
        drop(script);
        drop(url);

        if !exception.is_null() {
            return Err(JsException::adopt(self, exception));
        }
        assert!(!raw.is_null(), "eval returned null without throwing");
        Ok(unsafe { Value::from_raw(self, raw) })
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { sys::JSGlobalContextRelease(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_returns_number() {
        let ctx = Context::new();
        let v = ctx.eval("40 + 2", None).unwrap();
        assert_eq!(v.to_number(), 42.0);
    }

    #[test]
    fn eval_returns_string() {
        let ctx = Context::new();
        let v = ctx.eval("'hello ' + 'world'", None).unwrap();
        assert_eq!(v.to_string(), "hello world");
    }

    #[test]
    fn eval_throws() {
        let ctx = Context::new();
        let err = ctx.eval("throw new Error('boom')", Some("test.js")).unwrap_err();
        let msg = err.message();
        assert!(msg.contains("boom"), "unexpected message: {msg}");
    }

    #[test]
    fn eval_propagates_syntax_error() {
        let ctx = Context::new();
        let err = ctx.eval("function (", None).unwrap_err();
        let m = err.message().to_lowercase();
        // JSC phrases this as "Unexpected end of script", but on some versions
        // it's "Unexpected token …" or similar. Just confirm we got *some* error.
        assert!(!m.is_empty(), "empty error message: {:?}", err);
    }

    #[test]
    fn global_object_set_and_read_back() {
        let ctx = Context::new();
        let global = ctx.global_object();
        let v = Value::new_number(&ctx, 7.0);
        global.set_property("answer", &v).unwrap();
        let got = ctx.eval("answer", None).unwrap();
        assert_eq!(got.to_number(), 7.0);
    }

    #[test]
    fn function_callback_round_trip() {
        let ctx = Context::new();
        let global = ctx.global_object();
        let cb = Callback::new(&ctx, "add", |args| {
            let a = args.get(0).to_number();
            let b = args.get(1).to_number();
            Ok(Value::new_number(args.context(), a + b))
        });
        global.set_property("add", &cb.value_in(&ctx)).unwrap();
        let v = ctx.eval("add(3, 4)", None).unwrap();
        assert_eq!(v.to_number(), 7.0);
    }
}
