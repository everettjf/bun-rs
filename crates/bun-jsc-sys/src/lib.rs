//! Raw FFI bindings to the JavaScriptCore C API.
//!
//! Hand-written (no bindgen) for the ~40 symbols MVP needs. Everything is
//! `unsafe`; the safe wrapper lives in `bun-jsc`.
//!
//! Reference: macOS SDK `JavaScriptCore.framework/Headers/*.h`.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

// ── Opaque handles ───────────────────────────────────────────────────────────

pub enum OpaqueJSContext {}
pub enum OpaqueJSValue {}
pub enum OpaqueJSString {}
pub enum OpaqueJSClass {}
pub enum OpaqueJSPropertyNameArray {}
pub enum OpaqueJSContextGroup {}

pub type JSContextRef = *const OpaqueJSContext;
pub type JSGlobalContextRef = *mut OpaqueJSContext;
pub type JSValueRef = *const OpaqueJSValue;
pub type JSObjectRef = *mut OpaqueJSValue;
pub type JSStringRef = *mut OpaqueJSString;
pub type JSClassRef = *mut OpaqueJSClass;
pub type JSPropertyNameArrayRef = *mut OpaqueJSPropertyNameArray;
pub type JSContextGroupRef = *const OpaqueJSContextGroup;

pub type JSChar = u16;

// JSType enum mirrors JSValueRef.h.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum JSType {
    Undefined = 0,
    Null = 1,
    Boolean = 2,
    Number = 3,
    String = 4,
    Object = 5,
    Symbol = 6,
}

/// Property attribute bitset (matches JSObjectRef.h).
pub type JSPropertyAttributes = c_uint;
pub const kJSPropertyAttributeNone: JSPropertyAttributes = 0;
pub const kJSPropertyAttributeReadOnly: JSPropertyAttributes = 1 << 1;
pub const kJSPropertyAttributeDontEnum: JSPropertyAttributes = 1 << 2;
pub const kJSPropertyAttributeDontDelete: JSPropertyAttributes = 1 << 3;

pub type JSObjectCallAsFunctionCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    function: JSObjectRef,
    this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef;

pub type JSObjectFinalizeCallback = unsafe extern "C" fn(object: JSObjectRef);
pub type JSObjectInitializeCallback = unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef);
pub type JSObjectHasPropertyCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    propertyName: JSStringRef,
) -> bool;
pub type JSObjectGetPropertyCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    propertyName: JSStringRef,
    exception: *mut JSValueRef,
) -> JSValueRef;
pub type JSObjectSetPropertyCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    propertyName: JSStringRef,
    value: JSValueRef,
    exception: *mut JSValueRef,
) -> bool;
pub type JSObjectDeletePropertyCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    propertyName: JSStringRef,
    exception: *mut JSValueRef,
) -> bool;
pub type JSObjectGetPropertyNamesCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    propertyNames: *mut c_void,
);
pub type JSObjectCallAsConstructorCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    constructor: JSObjectRef,
    argumentCount: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSObjectRef;
pub type JSObjectHasInstanceCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    constructor: JSObjectRef,
    possibleInstance: JSValueRef,
    exception: *mut JSValueRef,
) -> bool;
pub type JSObjectConvertToTypeCallback = unsafe extern "C" fn(
    ctx: JSContextRef,
    object: JSObjectRef,
    ty: JSType,
    exception: *mut JSValueRef,
) -> JSValueRef;

pub type JSClassAttributes = c_uint;
pub const kJSClassAttributeNone: JSClassAttributes = 0;
pub const kJSClassAttributeNoAutomaticPrototype: JSClassAttributes = 1 << 1;

#[repr(C)]
pub struct JSStaticValue {
    pub name: *const c_char,
    pub getProperty: Option<JSObjectGetPropertyCallback>,
    pub setProperty: Option<JSObjectSetPropertyCallback>,
    pub attributes: JSPropertyAttributes,
}

#[repr(C)]
pub struct JSStaticFunction {
    pub name: *const c_char,
    pub callAsFunction: Option<JSObjectCallAsFunctionCallback>,
    pub attributes: JSPropertyAttributes,
}

/// Layout matches JSClassDefinition in `JSObjectRef.h`. Version 0 is the only
/// version JSC ships.
#[repr(C)]
pub struct JSClassDefinition {
    pub version: c_int,
    pub attributes: JSClassAttributes,
    pub className: *const c_char,
    pub parentClass: JSClassRef,
    pub staticValues: *const JSStaticValue,
    pub staticFunctions: *const JSStaticFunction,
    pub initialize: Option<JSObjectInitializeCallback>,
    pub finalize: Option<JSObjectFinalizeCallback>,
    pub hasProperty: Option<JSObjectHasPropertyCallback>,
    pub getProperty: Option<JSObjectGetPropertyCallback>,
    pub setProperty: Option<JSObjectSetPropertyCallback>,
    pub deleteProperty: Option<JSObjectDeletePropertyCallback>,
    pub getPropertyNames: Option<JSObjectGetPropertyNamesCallback>,
    pub callAsFunction: Option<JSObjectCallAsFunctionCallback>,
    pub callAsConstructor: Option<JSObjectCallAsConstructorCallback>,
    pub hasInstance: Option<JSObjectHasInstanceCallback>,
    pub convertToType: Option<JSObjectConvertToTypeCallback>,
}

impl JSClassDefinition {
    /// Equivalent of `kJSClassDefinitionEmpty`.
    pub const EMPTY: JSClassDefinition = JSClassDefinition {
        version: 0,
        attributes: kJSClassAttributeNone,
        className: std::ptr::null(),
        parentClass: std::ptr::null_mut(),
        staticValues: std::ptr::null(),
        staticFunctions: std::ptr::null(),
        initialize: None,
        finalize: None,
        hasProperty: None,
        getProperty: None,
        setProperty: None,
        deleteProperty: None,
        getPropertyNames: None,
        callAsFunction: None,
        callAsConstructor: None,
        hasInstance: None,
        convertToType: None,
    };
}

extern "C" {
    // --- Context ---
    pub fn JSGlobalContextCreate(globalObjectClass: JSClassRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
    pub fn JSGlobalContextRetain(ctx: JSGlobalContextRef) -> JSGlobalContextRef;
    pub fn JSContextGetGlobalObject(ctx: JSContextRef) -> JSObjectRef;
    pub fn JSContextGetGlobalContext(ctx: JSContextRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextSetInspectable(ctx: JSGlobalContextRef, inspectable: bool);

    // --- Evaluation ---
    pub fn JSEvaluateScript(
        ctx: JSContextRef,
        script: JSStringRef,
        thisObject: JSObjectRef,
        sourceURL: JSStringRef,
        startingLineNumber: c_int,
        exception: *mut JSValueRef,
    ) -> JSValueRef;

    pub fn JSCheckScriptSyntax(
        ctx: JSContextRef,
        script: JSStringRef,
        sourceURL: JSStringRef,
        startingLineNumber: c_int,
        exception: *mut JSValueRef,
    ) -> bool;

    pub fn JSGarbageCollect(ctx: JSContextRef);

    // --- Strings ---
    pub fn JSStringCreateWithUTF8CString(string: *const c_char) -> JSStringRef;
    pub fn JSStringCreateWithCharacters(chars: *const JSChar, numChars: usize) -> JSStringRef;
    pub fn JSStringRelease(string: JSStringRef);
    pub fn JSStringRetain(string: JSStringRef) -> JSStringRef;
    pub fn JSStringGetLength(string: JSStringRef) -> usize;
    pub fn JSStringGetCharactersPtr(string: JSStringRef) -> *const JSChar;
    pub fn JSStringGetMaximumUTF8CStringSize(string: JSStringRef) -> usize;
    pub fn JSStringGetUTF8CString(
        string: JSStringRef,
        buffer: *mut c_char,
        bufferSize: usize,
    ) -> usize;
    pub fn JSStringIsEqualToUTF8CString(a: JSStringRef, b: *const c_char) -> bool;

    // --- Value introspection ---
    pub fn JSValueGetType(ctx: JSContextRef, value: JSValueRef) -> JSType;
    pub fn JSValueIsUndefined(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsNull(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsBoolean(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsNumber(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsString(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsObject(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsArray(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsStrictEqual(ctx: JSContextRef, a: JSValueRef, b: JSValueRef) -> bool;

    // --- Value construction ---
    pub fn JSValueMakeUndefined(ctx: JSContextRef) -> JSValueRef;
    pub fn JSValueMakeNull(ctx: JSContextRef) -> JSValueRef;
    pub fn JSValueMakeBoolean(ctx: JSContextRef, boolean: bool) -> JSValueRef;
    pub fn JSValueMakeNumber(ctx: JSContextRef, number: f64) -> JSValueRef;
    pub fn JSValueMakeString(ctx: JSContextRef, string: JSStringRef) -> JSValueRef;

    // --- Value conversion ---
    pub fn JSValueToBoolean(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueToNumber(
        ctx: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> f64;
    pub fn JSValueToStringCopy(
        ctx: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSStringRef;
    pub fn JSValueToObject(
        ctx: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;

    pub fn JSValueProtect(ctx: JSContextRef, value: JSValueRef);
    pub fn JSValueUnprotect(ctx: JSContextRef, value: JSValueRef);

    // --- JSON ---
    pub fn JSValueMakeFromJSONString(ctx: JSContextRef, string: JSStringRef) -> JSValueRef;
    pub fn JSValueCreateJSONString(
        ctx: JSContextRef,
        value: JSValueRef,
        indent: c_uint,
        exception: *mut JSValueRef,
    ) -> JSStringRef;

    // --- Objects ---
    pub fn JSObjectMake(ctx: JSContextRef, jsClass: JSClassRef, data: *mut c_void) -> JSObjectRef;
    pub fn JSObjectMakeFunctionWithCallback(
        ctx: JSContextRef,
        name: JSStringRef,
        callAsFunction: JSObjectCallAsFunctionCallback,
    ) -> JSObjectRef;
    pub fn JSObjectMakeError(
        ctx: JSContextRef,
        argumentCount: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;

    pub fn JSObjectMakeDeferredPromise(
        ctx: JSContextRef,
        resolve: *mut JSObjectRef,
        reject: *mut JSObjectRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;

    pub fn JSObjectHasProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        propertyName: JSStringRef,
    ) -> bool;
    pub fn JSObjectGetProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        propertyName: JSStringRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectSetProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        propertyName: JSStringRef,
        value: JSValueRef,
        attributes: JSPropertyAttributes,
        exception: *mut JSValueRef,
    );
    pub fn JSObjectGetPropertyAtIndex(
        ctx: JSContextRef,
        object: JSObjectRef,
        propertyIndex: c_uint,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectSetPropertyAtIndex(
        ctx: JSContextRef,
        object: JSObjectRef,
        propertyIndex: c_uint,
        value: JSValueRef,
        exception: *mut JSValueRef,
    );

    pub fn JSObjectIsFunction(ctx: JSContextRef, object: JSObjectRef) -> bool;
    pub fn JSObjectCallAsFunction(
        ctx: JSContextRef,
        object: JSObjectRef,
        thisObject: JSObjectRef,
        argumentCount: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;

    pub fn JSObjectIsConstructor(ctx: JSContextRef, object: JSObjectRef) -> bool;
    pub fn JSObjectCallAsConstructor(
        ctx: JSContextRef,
        object: JSObjectRef,
        argumentCount: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;

    pub fn JSObjectGetPrivate(object: JSObjectRef) -> *mut c_void;
    pub fn JSObjectSetPrivate(object: JSObjectRef, data: *mut c_void) -> bool;

    pub fn JSClassCreate(definition: *const JSClassDefinition) -> JSClassRef;
    pub fn JSClassRetain(jsClass: JSClassRef) -> JSClassRef;
    pub fn JSClassRelease(jsClass: JSClassRef);

    pub fn JSObjectCopyPropertyNames(
        ctx: JSContextRef,
        object: JSObjectRef,
    ) -> JSPropertyNameArrayRef;
    pub fn JSPropertyNameArrayRelease(array: JSPropertyNameArrayRef);
    pub fn JSPropertyNameArrayGetCount(array: JSPropertyNameArrayRef) -> usize;
    pub fn JSPropertyNameArrayGetNameAtIndex(
        array: JSPropertyNameArrayRef,
        index: usize,
    ) -> JSStringRef;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::ptr;

    /// Smoke test: evaluate "1 + 1", get back a JSValueRef holding 2.0.
    /// This is the very first thing that must work before anything else does.
    #[test]
    fn evaluate_one_plus_one() {
        unsafe {
            let ctx = JSGlobalContextCreate(ptr::null_mut());
            assert!(!ctx.is_null(), "context creation");

            let src = CString::new("1 + 1").unwrap();
            let script = JSStringCreateWithUTF8CString(src.as_ptr());

            let mut exception: JSValueRef = ptr::null();
            let result = JSEvaluateScript(
                ctx,
                script,
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut exception,
            );
            JSStringRelease(script);

            assert!(exception.is_null(), "no exception");
            assert!(!result.is_null(), "got a value");
            assert!(JSValueIsNumber(ctx, result));

            let n = JSValueToNumber(ctx, result, ptr::null_mut());
            assert_eq!(n, 2.0);

            JSGlobalContextRelease(ctx);
        }
    }
}
