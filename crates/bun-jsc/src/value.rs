use std::marker::PhantomData;
use std::ptr;

use bun_jsc_sys as sys;

use crate::{Context, JsException, JsString, Object};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValueKind {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Object,
    Symbol,
}

/// A JS value tied to a [`Context`].
#[derive(Copy, Clone)]
pub struct Value<'ctx> {
    pub(crate) ctx: &'ctx Context,
    pub(crate) raw: sys::JSValueRef,
}

impl<'ctx> Value<'ctx> {
    pub(crate) unsafe fn from_raw(ctx: &'ctx Context, raw: sys::JSValueRef) -> Self {
        Self { ctx, raw }
    }

    pub fn as_raw(&self) -> sys::JSValueRef {
        self.raw
    }

    pub fn context(&self) -> &'ctx Context {
        self.ctx
    }

    pub fn new_undefined(ctx: &'ctx Context) -> Self {
        let raw = unsafe { sys::JSValueMakeUndefined(ctx.as_raw()) };
        Self { ctx, raw }
    }

    pub fn new_null(ctx: &'ctx Context) -> Self {
        let raw = unsafe { sys::JSValueMakeNull(ctx.as_raw()) };
        Self { ctx, raw }
    }

    pub fn new_bool(ctx: &'ctx Context, b: bool) -> Self {
        let raw = unsafe { sys::JSValueMakeBoolean(ctx.as_raw(), b) };
        Self { ctx, raw }
    }

    pub fn new_number(ctx: &'ctx Context, n: f64) -> Self {
        let raw = unsafe { sys::JSValueMakeNumber(ctx.as_raw(), n) };
        Self { ctx, raw }
    }

    pub fn new_string(ctx: &'ctx Context, s: &str) -> Self {
        let js = JsString::new(s);
        let raw = unsafe { sys::JSValueMakeString(ctx.as_raw(), js.as_raw()) };
        Self { ctx, raw }
    }

    pub fn kind(&self) -> ValueKind {
        let t = unsafe { sys::JSValueGetType(self.ctx.as_raw(), self.raw) };
        match t {
            sys::JSType::Undefined => ValueKind::Undefined,
            sys::JSType::Null => ValueKind::Null,
            sys::JSType::Boolean => ValueKind::Boolean,
            sys::JSType::Number => ValueKind::Number,
            sys::JSType::String => ValueKind::String,
            sys::JSType::Object => ValueKind::Object,
            sys::JSType::Symbol => ValueKind::Symbol,
        }
    }

    pub fn is_undefined(&self) -> bool {
        unsafe { sys::JSValueIsUndefined(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_null(&self) -> bool {
        unsafe { sys::JSValueIsNull(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_nullish(&self) -> bool {
        self.is_undefined() || self.is_null()
    }

    pub fn is_boolean(&self) -> bool {
        unsafe { sys::JSValueIsBoolean(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_number(&self) -> bool {
        unsafe { sys::JSValueIsNumber(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_string(&self) -> bool {
        unsafe { sys::JSValueIsString(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_object(&self) -> bool {
        unsafe { sys::JSValueIsObject(self.ctx.as_raw(), self.raw) }
    }

    pub fn to_bool(&self) -> bool {
        unsafe { sys::JSValueToBoolean(self.ctx.as_raw(), self.raw) }
    }

    pub fn to_number(&self) -> f64 {
        unsafe { sys::JSValueToNumber(self.ctx.as_raw(), self.raw, ptr::null_mut()) }
    }

    /// Coerce to string (JS `String(v)` semantics).
    pub fn to_string(&self) -> String {
        unsafe {
            let s = sys::JSValueToStringCopy(self.ctx.as_raw(), self.raw, ptr::null_mut());
            if s.is_null() {
                return String::new();
            }
            JsString::adopt(s).to_string()
        }
    }

    pub fn to_object(self) -> Result<Object<'ctx>, JsException> {
        let mut exc: sys::JSValueRef = ptr::null();
        let obj = unsafe { sys::JSValueToObject(self.ctx.as_raw(), self.raw, &mut exc) };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(unsafe { Object::from_raw(self.ctx, obj) })
    }

    /// Encode the value as a JSON string. `indent` controls pretty-printing.
    pub fn to_json(&self, indent: u32) -> Result<String, JsException> {
        let mut exc: sys::JSValueRef = ptr::null();
        let s = unsafe {
            sys::JSValueCreateJSONString(self.ctx.as_raw(), self.raw, indent, &mut exc)
        };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        if s.is_null() {
            return Ok(String::new());
        }
        Ok(JsString::adopt(s).to_string())
    }

    /// Pin the value across drains. Caller must call `unprotect` later.
    pub fn protect(&self) {
        unsafe { sys::JSValueProtect(self.ctx.as_raw(), self.raw) }
    }

    pub fn unprotect(&self) {
        unsafe { sys::JSValueUnprotect(self.ctx.as_raw(), self.raw) }
    }
}

impl<'ctx> std::fmt::Debug for Value<'ctx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Value({:?}, \"{}\")", self.kind(), self.to_string())
    }
}

// Silence unused PhantomData warning if needed in the future.
#[allow(dead_code)]
struct _Lifetime<'a>(PhantomData<&'a ()>);
