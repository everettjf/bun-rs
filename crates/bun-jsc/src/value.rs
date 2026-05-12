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

    /// Public escape hatch for crates that need to mint a `Value` from a raw
    /// pointer (e.g. the runtime's module loader pulling cached exports out
    /// of a `HashMap<_, JSValueRef>`). Caller is responsible for keeping the
    /// raw pointer alive — typically via `JSValueProtect`.
    pub unsafe fn from_raw_public(ctx: &'ctx Context, raw: sys::JSValueRef) -> Self {
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

    /// Build a JS `Uint8Array` over the given byte buffer (zero-copy).
    /// JSC takes ownership; the data is freed via a Rust trampoline when
    /// the typed array is GC'd.
    pub fn new_uint8_array(ctx: &'ctx Context, mut bytes: Vec<u8>) -> Self {
        // Hand JSC a stable pointer + length. We need to keep the original
        // capacity around so the deallocator can rebuild the Vec.
        let len = bytes.len();
        let cap = bytes.capacity();
        let ptr = bytes.as_mut_ptr();
        std::mem::forget(bytes);

        // Stash (cap) in a Box so the deallocator can find it. We could pack
        // it into the deallocator-context pointer directly (it's a usize on
        // 64-bit), but a Box is clearer.
        let cap_box: Box<usize> = Box::new(cap);
        let cap_ctx = Box::into_raw(cap_box) as *mut std::os::raw::c_void;

        unsafe extern "C" fn dealloc(bytes: *mut std::os::raw::c_void, ctx: *mut std::os::raw::c_void) {
            let cap = *Box::from_raw(ctx as *mut usize);
            // Reconstruct the Vec to drop it. `len` isn't preserved here;
            // we only need the buffer reclaimed, so set len = cap.
            // SAFETY: ptr was originally a Vec<u8> allocation with the
            // recorded capacity.
            let _ = Vec::from_raw_parts(bytes as *mut u8, cap, cap);
        }

        let mut exc: sys::JSValueRef = std::ptr::null();
        let arr = unsafe {
            sys::JSObjectMakeTypedArrayWithBytesNoCopy(
                ctx.as_raw(),
                sys::JSTypedArrayType::Uint8Array,
                ptr as *mut _,
                len,
                Some(dealloc),
                cap_ctx,
                &mut exc,
            )
        };
        if arr.is_null() {
            // JSC failed — recover the Vec and let it drop normally.
            unsafe {
                let _ = Vec::from_raw_parts(ptr, len, cap);
                let _ = Box::from_raw(cap_ctx as *mut usize);
            }
            // Fall back to an empty array.
            let empty = unsafe {
                sys::JSObjectMakeTypedArray(
                    ctx.as_raw(),
                    sys::JSTypedArrayType::Uint8Array,
                    0,
                    std::ptr::null_mut(),
                )
            };
            return Self {
                ctx,
                raw: empty as sys::JSValueRef,
            };
        }
        Self {
            ctx,
            raw: arr as sys::JSValueRef,
        }
    }

    /// Inspect: if this value is a Uint8Array (or any other typed array view
    /// over bytes), return a view of its backing storage. `None` otherwise.
    /// The slice is valid for as long as JSC keeps the typed array alive;
    /// since Value borrows the Context, that's the typical scope.
    pub fn typed_array_bytes(&self) -> Option<&[u8]> {
        unsafe {
            let mut exc: sys::JSValueRef = std::ptr::null();
            let ty = sys::JSValueGetTypedArrayType(self.ctx.as_raw(), self.raw, &mut exc);
            if !exc.is_null() {
                return None;
            }
            match ty {
                sys::JSTypedArrayType::Uint8Array
                | sys::JSTypedArrayType::Uint8ClampedArray
                | sys::JSTypedArrayType::Int8Array => {}
                _ => return None,
            }
            let obj = self.raw as sys::JSObjectRef;
            let ptr = sys::JSObjectGetTypedArrayBytesPtr(self.ctx.as_raw(), obj, std::ptr::null_mut());
            let len = sys::JSObjectGetTypedArrayLength(self.ctx.as_raw(), obj, std::ptr::null_mut());
            if ptr.is_null() {
                return None;
            }
            Some(std::slice::from_raw_parts(ptr as *const u8, len))
        }
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
