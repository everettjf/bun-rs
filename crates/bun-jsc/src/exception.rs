use bun_jsc_sys as sys;

use crate::{Context, JsString, Object, Value};

/// A thrown JS value. Protected (pinned in JSC's GC) for the duration of its
/// Rust lifetime so it survives even if we drain microtasks before reading it.
pub struct JsException {
    raw: sys::JSValueRef,
    // We can't borrow from a temporary in tests; instead store the global
    // context pointer (refcounted via retain) so we can read message/stack
    // even after the original Context goes out of scope.
    ctx: sys::JSGlobalContextRef,
}

impl JsException {
    /// Adopt a raw exception that JSC just handed us. Protects it immediately.
    pub(crate) fn adopt(ctx: &Context, raw: sys::JSValueRef) -> Self {
        unsafe {
            sys::JSValueProtect(ctx.as_raw(), raw);
            let g = sys::JSGlobalContextRetain(ctx.as_global_raw());
            Self { raw, ctx: g }
        }
    }

    fn raw_ctx(&self) -> sys::JSContextRef {
        self.ctx as sys::JSContextRef
    }

    /// Best-effort stringification of `error.message`, falling back to `String(err)`.
    pub fn message(&self) -> String {
        unsafe {
            if sys::JSValueIsObject(self.raw_ctx(), self.raw) {
                let obj = self.raw as sys::JSObjectRef;
                let key = JsString::new("message");
                let mut sub: sys::JSValueRef = std::ptr::null();
                let v = sys::JSObjectGetProperty(self.raw_ctx(), obj, key.as_raw(), &mut sub);
                if !sub.is_null() {
                    // Reading `.message` itself threw — fall through.
                } else if !v.is_null() && sys::JSValueIsString(self.raw_ctx(), v) {
                    let s =
                        sys::JSValueToStringCopy(self.raw_ctx(), v, std::ptr::null_mut());
                    if !s.is_null() {
                        return JsString::adopt(s).to_string();
                    }
                }
            }
            // Fallback: stringify the whole value.
            let s = sys::JSValueToStringCopy(self.raw_ctx(), self.raw, std::ptr::null_mut());
            if s.is_null() {
                return "<unknown error>".into();
            }
            JsString::adopt(s).to_string()
        }
    }

    /// Stack trace, if the thrown value has one (Error objects do).
    pub fn stack(&self) -> Option<String> {
        unsafe {
            if !sys::JSValueIsObject(self.raw_ctx(), self.raw) {
                return None;
            }
            let obj = self.raw as sys::JSObjectRef;
            let key = JsString::new("stack");
            let mut sub: sys::JSValueRef = std::ptr::null();
            let v = sys::JSObjectGetProperty(self.raw_ctx(), obj, key.as_raw(), &mut sub);
            if !sub.is_null() || v.is_null() {
                return None;
            }
            if sys::JSValueIsUndefined(self.raw_ctx(), v) {
                return None;
            }
            let s = sys::JSValueToStringCopy(self.raw_ctx(), v, std::ptr::null_mut());
            if s.is_null() {
                return None;
            }
            Some(JsString::adopt(s).to_string())
        }
    }

    /// Bind to a context to use as a regular Value.
    pub fn as_value<'a>(&'a self, ctx: &'a Context) -> Value<'a> {
        unsafe { Value::from_raw(ctx, self.raw) }
    }

    /// Re-construct an Object handle if the exception is an object.
    pub fn as_object<'a>(&'a self, ctx: &'a Context) -> Option<Object<'a>> {
        unsafe {
            if sys::JSValueIsObject(ctx.as_raw(), self.raw) {
                Some(Object::from_raw(ctx, self.raw as sys::JSObjectRef))
            } else {
                None
            }
        }
    }
}

impl std::fmt::Debug for JsException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JsException({:?})", self.message())
    }
}

impl std::fmt::Display for JsException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for JsException {}

impl Drop for JsException {
    fn drop(&mut self) {
        unsafe {
            sys::JSValueUnprotect(self.raw_ctx(), self.raw);
            sys::JSGlobalContextRelease(self.ctx);
        }
    }
}
