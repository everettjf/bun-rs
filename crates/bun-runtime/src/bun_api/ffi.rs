//! `bun:ffi` — minimal foreign-function interface via libffi.
//!
//! API (subset of Bun's):
//!   import { dlopen, FFIType } from "bun:ffi";
//!   const lib = dlopen("/path/to.dylib", {
//!     add: { args: [FFIType.i32, FFIType.i32], returns: FFIType.i32 },
//!     greet: { args: [FFIType.cstring], returns: FFIType.cstring },
//!   });
//!   lib.symbols.add(2, 3);     // 5
//!   lib.close();
//!
//! Supported types: void / i8 / u8 / i16 / u16 / i32 / u32 / i64 / u64 /
//! f32 / f64 / pointer / cstring (in/out).
//!
//! Limitations: no struct args, no variadic, no callback function pointers,
//! no auto-marshalling for non-trivial buffers. Strings ARE handled both
//! directions (cstring args are Rust-owned CStrings; cstring returns are
//! borrowed as JS strings — caller must not free).

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::rc::Rc;

use bun_jsc::{Callback, Context, Value};
use libffi::middle::{Arg, Cif, CodePtr, Type};
use libloading::Library;

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[bun:ffi]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    // FFIType enum.
    let types_v = ctx.eval("({})", Some("[FFIType]")).unwrap();
    let types_obj = types_v.to_object().unwrap();
    for (name, val) in FFI_TYPES.iter() {
        let _ = types_obj.set_property(name, &Value::new_string(ctx, val));
    }
    exports.set_property("FFIType", &types_v).unwrap();

    bind(ctx, &exports, "dlopen", |args| {
        let path = args.get(0).to_string();
        let spec_v = args.get(1);
        if !spec_v.is_object() {
            return Err("dlopen: second arg must be the symbol map".into());
        }
        let spec = spec_v.to_object().map_err(|e| e.to_string())?;
        let lib = unsafe { Library::new(&path) }.map_err(|e| e.to_string())?;
        let lib_rc: Rc<Library> = Rc::new(lib);

        let result = args.context().eval("({ symbols: {}, close: undefined })", Some("[ffi-result]")).unwrap();
        let result_obj = result.to_object().map_err(|e| e.to_string())?;
        let symbols_v = result_obj.get_property("symbols").map_err(|e| e.to_string())?;
        let symbols = symbols_v.to_object().map_err(|e| e.to_string())?;

        for name in spec.property_names() {
            let def_v = spec.get_property(&name).map_err(|e| e.to_string())?;
            if !def_v.is_object() { continue; }
            let def = def_v.to_object().map_err(|e| e.to_string())?;
            let args_arr_v = def.get_property("args").unwrap_or(Value::new_undefined(args.context()));
            let returns_v = def.get_property("returns").unwrap_or(Value::new_undefined(args.context()));
            let arg_types = parse_type_list(&args_arr_v);
            let ret_type = type_from_string(&returns_v.to_string());
            let symbol_name = CString::new(name.as_str()).map_err(|e| e.to_string())?;
            let ptr = unsafe {
                let raw_lib: &Library = &lib_rc;
                let s: libloading::Symbol<'_, *const c_void> = raw_lib
                    .get(symbol_name.as_bytes_with_nul())
                    .map_err(|e| format!("dlsym {}: {e}", name))?;
                *s
            };
            let code_ptr = CodePtr::from_ptr(ptr);
            let _ = lib_rc.clone();

            let arg_types_clone = arg_types.clone();
            let ret_type_clone = ret_type;
            let lib_for_cb = lib_rc.clone();
            let stub = Callback::new(args.context(), &name, move |args| {
                // Keep lib alive across calls.
                let _keep = &lib_for_cb;
                let cif_args: Vec<Type> = arg_types_clone.iter().map(|t| t.to_libffi()).collect();
                let cif_ret = ret_type_clone.to_libffi();
                let cif = Cif::new(cif_args, cif_ret);
                let result = call_with_args(&cif, code_ptr, &arg_types_clone, ret_type_clone, &args)?;
                Ok(result)
            });
            symbols.set_property(&name, &stub.value_in(args.context())).unwrap();
            std::mem::forget(stub);
        }

        let lib_for_close = Rc::new(RefCell::new(Some(lib_rc)));
        let close_cb = Callback::new(args.context(), "close", move |args| {
            *lib_for_close.borrow_mut() = None;
            Ok(Value::new_undefined(args.context()))
        });
        result_obj
            .set_property("close", &close_cb.value_in(args.context()))
            .unwrap();
        std::mem::forget(close_cb);

        Ok(result)
    });

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

const FFI_TYPES: &[(&str, &str)] = &[
    ("void", "void"),
    ("i8", "i8"), ("u8", "u8"),
    ("i16", "i16"), ("u16", "u16"),
    ("i32", "i32"), ("u32", "u32"),
    ("i64", "i64"), ("u64", "u64"),
    ("f32", "f32"), ("f64", "f64"),
    ("pointer", "pointer"), ("ptr", "pointer"),
    ("cstring", "cstring"),
    ("bool", "i32"),
];

#[derive(Copy, Clone, Debug)]
enum FT {
    Void, I8, U8, I16, U16, I32, U32, I64, U64, F32, F64, Ptr, Cstr,
}

impl FT {
    fn to_libffi(self) -> Type {
        match self {
            FT::Void => Type::void(),
            FT::I8 => Type::i8(),
            FT::U8 => Type::u8(),
            FT::I16 => Type::i16(),
            FT::U16 => Type::u16(),
            FT::I32 => Type::i32(),
            FT::U32 => Type::u32(),
            FT::I64 => Type::i64(),
            FT::U64 => Type::u64(),
            FT::F32 => Type::f32(),
            FT::F64 => Type::f64(),
            FT::Ptr | FT::Cstr => Type::pointer(),
        }
    }
}

fn type_from_string(s: &str) -> FT {
    match s.to_lowercase().as_str() {
        "void" => FT::Void,
        "i8" | "char" => FT::I8,
        "u8" => FT::U8,
        "i16" | "short" => FT::I16,
        "u16" => FT::U16,
        "i32" | "int" => FT::I32,
        "u32" | "uint" => FT::U32,
        "i64" | "long" => FT::I64,
        "u64" | "ulong" => FT::U64,
        "f32" | "float" => FT::F32,
        "f64" | "double" => FT::F64,
        "pointer" | "ptr" => FT::Ptr,
        "cstring" | "string" => FT::Cstr,
        _ => FT::I32, // sensible fallback
    }
}

fn parse_type_list(v: &Value<'_>) -> Vec<FT> {
    if !v.is_object() {
        return vec![];
    }
    let Ok(obj) = v.to_object() else { return vec![] };
    let len = obj
        .get_property("length")
        .map(|l| l.to_number() as u32)
        .unwrap_or(0);
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        if let Ok(el) = obj.get_property_at(i) {
            out.push(type_from_string(&el.to_string()));
        }
    }
    out
}

/// Pack each JS argument into the appropriate native storage, then call.
fn call_with_args<'a>(
    cif: &Cif,
    func: CodePtr,
    arg_types: &[FT],
    ret_type: FT,
    args: &bun_jsc::CallbackArgs<'a>,
) -> Result<Value<'a>, String> {
    // Storage for each arg's native representation.
    let mut storage_i8: Vec<i8> = Vec::with_capacity(arg_types.len());
    let mut storage_u8: Vec<u8> = Vec::with_capacity(arg_types.len());
    let mut storage_i16: Vec<i16> = Vec::with_capacity(arg_types.len());
    let mut storage_u16: Vec<u16> = Vec::with_capacity(arg_types.len());
    let mut storage_i32: Vec<i32> = Vec::with_capacity(arg_types.len());
    let mut storage_u32: Vec<u32> = Vec::with_capacity(arg_types.len());
    let mut storage_i64: Vec<i64> = Vec::with_capacity(arg_types.len());
    let mut storage_u64: Vec<u64> = Vec::with_capacity(arg_types.len());
    let mut storage_f32: Vec<f32> = Vec::with_capacity(arg_types.len());
    let mut storage_f64: Vec<f64> = Vec::with_capacity(arg_types.len());
    let mut storage_ptr: Vec<*const c_void> = Vec::with_capacity(arg_types.len());
    let mut owned_cstrings: Vec<CString> = Vec::new();

    // Pre-allocate all storage first so addresses don't move.
    for (i, ty) in arg_types.iter().enumerate() {
        let arg = args.get(i);
        match ty {
            FT::I8 => storage_i8.push(arg.to_number() as i8),
            FT::U8 => storage_u8.push(arg.to_number() as u8),
            FT::I16 => storage_i16.push(arg.to_number() as i16),
            FT::U16 => storage_u16.push(arg.to_number() as u16),
            FT::I32 => storage_i32.push(arg.to_number() as i32),
            FT::U32 => storage_u32.push(arg.to_number() as u32),
            FT::I64 => storage_i64.push(arg.to_number() as i64),
            FT::U64 => storage_u64.push(arg.to_number() as u64),
            FT::F32 => storage_f32.push(arg.to_number() as f32),
            FT::F64 => storage_f64.push(arg.to_number()),
            FT::Ptr => {
                let n = arg.to_number() as usize;
                storage_ptr.push(n as *const c_void);
            }
            FT::Cstr => {
                let s = arg.to_string();
                let c = CString::new(s).map_err(|e| e.to_string())?;
                let p = c.as_ptr() as *const c_void;
                owned_cstrings.push(c);
                storage_ptr.push(p);
            }
            FT::Void => {}
        }
    }

    // Build Arg slice referencing the stable storage.
    let mut ffi_args: Vec<Arg> = Vec::with_capacity(arg_types.len());
    let (mut i8i, mut u8i, mut i16i, mut u16i, mut i32i, mut u32i, mut i64i, mut u64i,
         mut f32i, mut f64i, mut ptri) = (0usize, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    for ty in arg_types {
        match ty {
            FT::I8 => { ffi_args.push(Arg::new(&storage_i8[i8i])); i8i += 1; }
            FT::U8 => { ffi_args.push(Arg::new(&storage_u8[u8i])); u8i += 1; }
            FT::I16 => { ffi_args.push(Arg::new(&storage_i16[i16i])); i16i += 1; }
            FT::U16 => { ffi_args.push(Arg::new(&storage_u16[u16i])); u16i += 1; }
            FT::I32 => { ffi_args.push(Arg::new(&storage_i32[i32i])); i32i += 1; }
            FT::U32 => { ffi_args.push(Arg::new(&storage_u32[u32i])); u32i += 1; }
            FT::I64 => { ffi_args.push(Arg::new(&storage_i64[i64i])); i64i += 1; }
            FT::U64 => { ffi_args.push(Arg::new(&storage_u64[u64i])); u64i += 1; }
            FT::F32 => { ffi_args.push(Arg::new(&storage_f32[f32i])); f32i += 1; }
            FT::F64 => { ffi_args.push(Arg::new(&storage_f64[f64i])); f64i += 1; }
            FT::Ptr | FT::Cstr => { ffi_args.push(Arg::new(&storage_ptr[ptri])); ptri += 1; }
            FT::Void => {}
        }
    }

    let ctx = args.context();
    let result: Value<'a> = unsafe {
        match ret_type {
            FT::Void => { let _: () = cif.call(func, &ffi_args); Value::new_undefined(ctx) }
            FT::I8 => { let r: i8 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::U8 => { let r: u8 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::I16 => { let r: i16 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::U16 => { let r: u16 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::I32 => { let r: i32 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::U32 => { let r: u32 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::I64 => { let r: i64 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::U64 => { let r: u64 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::F32 => { let r: f32 = cif.call(func, &ffi_args); Value::new_number(ctx, r as f64) }
            FT::F64 => { let r: f64 = cif.call(func, &ffi_args); Value::new_number(ctx, r) }
            FT::Ptr => {
                let r: *const c_void = cif.call(func, &ffi_args);
                Value::new_number(ctx, r as usize as f64)
            }
            FT::Cstr => {
                let r: *const i8 = cif.call(func, &ffi_args);
                if r.is_null() {
                    Value::new_null(ctx)
                } else {
                    let cs = std::ffi::CStr::from_ptr(r);
                    Value::new_string(ctx, &cs.to_string_lossy())
                }
            }
        }
    };
    // Keep cstrings alive until after the call.
    drop(owned_cstrings);
    Ok(result)
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
