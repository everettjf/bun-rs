use std::ptr;

use bun_jsc_sys as sys;

use crate::{Context, JsException, JsString, Value};

/// JS object — a [`Value`] that holds the `Object` JS type.
#[derive(Copy, Clone)]
pub struct Object<'ctx> {
    ctx: &'ctx Context,
    raw: sys::JSObjectRef,
}

impl<'ctx> Object<'ctx> {
    pub(crate) unsafe fn from_raw(ctx: &'ctx Context, raw: sys::JSObjectRef) -> Self {
        Self { ctx, raw }
    }

    pub fn as_raw(&self) -> sys::JSObjectRef {
        self.raw
    }

    pub fn as_value(&self) -> Value<'ctx> {
        // A JSObjectRef can be cast to a JSValueRef (it's just a const-stripped pointer).
        unsafe { Value::from_raw(self.ctx, self.raw as sys::JSValueRef) }
    }

    pub fn get_property(&self, name: &str) -> Result<Value<'ctx>, JsException> {
        let key = JsString::new(name);
        let mut exc: sys::JSValueRef = ptr::null();
        let raw = unsafe {
            sys::JSObjectGetProperty(self.ctx.as_raw(), self.raw, key.as_raw(), &mut exc)
        };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(unsafe { Value::from_raw(self.ctx, raw) })
    }

    pub fn set_property(&self, name: &str, value: &Value<'_>) -> Result<(), JsException> {
        let key = JsString::new(name);
        let mut exc: sys::JSValueRef = ptr::null();
        unsafe {
            sys::JSObjectSetProperty(
                self.ctx.as_raw(),
                self.raw,
                key.as_raw(),
                value.as_raw(),
                sys::kJSPropertyAttributeNone,
                &mut exc,
            );
        }
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(())
    }

    pub fn get_property_at(&self, index: u32) -> Result<Value<'ctx>, JsException> {
        let mut exc: sys::JSValueRef = ptr::null();
        let raw = unsafe {
            sys::JSObjectGetPropertyAtIndex(self.ctx.as_raw(), self.raw, index, &mut exc)
        };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(unsafe { Value::from_raw(self.ctx, raw) })
    }

    pub fn is_function(&self) -> bool {
        unsafe { sys::JSObjectIsFunction(self.ctx.as_raw(), self.raw) }
    }

    pub fn is_constructor(&self) -> bool {
        unsafe { sys::JSObjectIsConstructor(self.ctx.as_raw(), self.raw) }
    }

    /// Call as `new self(...args)`. Returns the resulting object value.
    pub fn construct(&self, args: &[Value<'_>]) -> Result<Value<'ctx>, JsException> {
        let raw_args: Vec<sys::JSValueRef> = args.iter().map(|v| v.as_raw()).collect();
        let mut exc: sys::JSValueRef = ptr::null();
        let raw = unsafe {
            sys::JSObjectCallAsConstructor(
                self.ctx.as_raw(),
                self.raw,
                raw_args.len(),
                raw_args.as_ptr(),
                &mut exc,
            )
        };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(unsafe { Value::from_raw(self.ctx, raw as sys::JSValueRef) })
    }

    /// Call `self` as a function. `this` defaults to `undefined`.
    pub fn call(
        &self,
        this: Option<Object<'_>>,
        args: &[Value<'_>],
    ) -> Result<Value<'ctx>, JsException> {
        let raw_args: Vec<sys::JSValueRef> = args.iter().map(|v| v.as_raw()).collect();
        let mut exc: sys::JSValueRef = ptr::null();
        let this_raw = this.map_or(ptr::null_mut(), |o| o.as_raw());
        let raw = unsafe {
            sys::JSObjectCallAsFunction(
                self.ctx.as_raw(),
                self.raw,
                this_raw,
                raw_args.len(),
                raw_args.as_ptr(),
                &mut exc,
            )
        };
        if !exc.is_null() {
            return Err(JsException::adopt(self.ctx, exc));
        }
        Ok(unsafe { Value::from_raw(self.ctx, raw) })
    }

    /// Iterate enumerable property names of this object.
    pub fn property_names(&self) -> Vec<String> {
        unsafe {
            let arr = sys::JSObjectCopyPropertyNames(self.ctx.as_raw(), self.raw);
            if arr.is_null() {
                return vec![];
            }
            let count = sys::JSPropertyNameArrayGetCount(arr);
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let s = sys::JSPropertyNameArrayGetNameAtIndex(arr, i);
                if !s.is_null() {
                    let owned = sys::JSStringRetain(s);
                    out.push(JsString::adopt(owned).to_string());
                }
            }
            sys::JSPropertyNameArrayRelease(arr);
            out
        }
    }
}
