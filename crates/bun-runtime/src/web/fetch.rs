//! `globalThis.__bun_fetch(url, init)` — non-blocking HTTP.
//!
//! Returns a Promise that resolves with the raw response shape the polyfill
//! turns into a `Response`. Implementation:
//!
//!   1. `__bun_fetch` creates a deferred JSC Promise.
//!   2. It captures the resolve / reject functions (protected refs).
//!   3. A tokio task does the actual reqwest call.
//!   4. On completion, the task posts a closure back to the JS thread; that
//!      closure invokes resolve(result) or reject(error) on the JS side.
//!
//! This is correct even for very large/slow responses because the JS thread
//! stays free to fire timers, accept Bun.serve requests, etc.

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use std::ptr;

pub fn install(ctx: &Context) {
    let cb = Callback::new(ctx, "__bun_fetch", |args| {
        let url = args.get(0).to_string();
        let init = args.get(1);

        // Extract headers + body + method up front on the JS thread.
        let method = if init.is_object() {
            init.to_object()
                .ok()
                .and_then(|o| o.get_property("method").ok())
                .map(|v| v.to_string().to_uppercase())
                .unwrap_or_else(|| "GET".into())
        } else {
            "GET".into()
        };
        let headers_kv: Vec<(String, String)> = if init.is_object() {
            init.to_object()
                .ok()
                .and_then(|o| o.get_property("headers").ok())
                .filter(|v| v.is_object())
                .and_then(|v| v.to_object().ok())
                .map(|h| {
                    h.property_names()
                        .into_iter()
                        .filter_map(|k| {
                            h.get_property(&k).ok().map(|v| (k, v.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let body_bytes: Option<Vec<u8>> = if init.is_object() {
            init.to_object()
                .ok()
                .and_then(|o| o.get_property("body").ok())
                .filter(|v| !v.is_undefined() && !v.is_null())
                .map(|v| match v.typed_array_bytes() {
                    Some(b) => b.to_vec(),
                    None => v.to_string().into_bytes(),
                })
        } else {
            None
        };

        // Build deferred promise.
        let ctx_ref = args.context();
        let mut resolve: sys::JSObjectRef = ptr::null_mut();
        let mut reject: sys::JSObjectRef = ptr::null_mut();
        let mut exc: sys::JSValueRef = ptr::null();
        let promise = unsafe {
            sys::JSObjectMakeDeferredPromise(
                ctx_ref.as_raw(),
                &mut resolve as *mut _,
                &mut reject as *mut _,
                &mut exc,
            )
        };
        if !exc.is_null() {
            return Err("failed to construct Promise".into());
        }

        // Protect resolve/reject so they survive the tokio task latency.
        unsafe {
            sys::JSValueProtect(ctx_ref.as_raw(), resolve as sys::JSValueRef);
            sys::JSValueProtect(ctx_ref.as_raw(), reject as sys::JSValueRef);
        }
        // Cross-thread transit: send pointers as usize so the closure type
        // doesn't pick up `*mut OpaqueJSValue` (auto-traits propagate).
        let resolve_id = resolve as usize;
        let reject_id = reject as usize;

        // Bump the "in flight" gauge so the event loop knows to stay alive.
        crate::async_rt::note_started();

        let url_for_task = url.clone();
        crate::async_rt::spawn(async move {
            let client = match reqwest::Client::builder()
                .user_agent(concat!("bun-rs/", env!("CARGO_PKG_VERSION")))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    deliver(resolve_id, reject_id, Err(format!("client build: {e}")));
                    return;
                }
            };
            let method = method.parse::<reqwest::Method>().unwrap_or(reqwest::Method::GET);
            let mut req = client.request(method, &url_for_task);
            for (k, v) in &headers_kv {
                req = req.header(k.as_str(), v.as_str());
            }
            if let Some(b) = body_bytes {
                req = req.body(b);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    deliver(resolve_id, reject_id, Err(format!("fetch error: {e}")));
                    return;
                }
            };
            let status = resp.status().as_u16() as u32;
            let mut hdrs: Vec<(String, String)> = Vec::new();
            for (k, v) in resp.headers() {
                if let Ok(s) = v.to_str() {
                    hdrs.push((k.as_str().to_string(), s.to_string()));
                }
            }
            let bytes = match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    deliver(resolve_id, reject_id, Err(format!("body read: {e}")));
                    return;
                }
            };
            deliver(resolve_id, reject_id, Ok(FetchResult { status, url: url_for_task, headers: hdrs, body: bytes }));
        });

        Ok(unsafe { Value::from_raw_public(ctx_ref, promise as sys::JSValueRef) })
    });
    ctx.global_object()
        .set_property("__bun_fetch", &cb.value_in(ctx))
        .unwrap();
    std::mem::forget(cb);
}

struct FetchResult {
    status: u32,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Post a closure that invokes resolve(result) or reject(error) on the JS
/// thread. Releases the protect refs after invocation.
fn deliver(resolve_id: usize, reject_id: usize, result: Result<FetchResult, String>) {
    crate::async_rt::post_to_js(move |ctx| {
        let ctx_raw = ctx.as_raw();
        let resolve = resolve_id as sys::JSObjectRef;
        let reject = reject_id as sys::JSObjectRef;
        match result {
            Ok(r) => {
                // Build the same object shape the old blocking impl used so
                // the polyfill doesn't need to change.
                let body_text = String::from_utf8_lossy(&r.body).into_owned();
                let v = ctx.eval("({})", Some("[fetch-result]")).unwrap();
                let obj = v.to_object().unwrap();
                obj.set_property("status", &Value::new_number(ctx, r.status as f64))
                    .unwrap();
                obj.set_property("url", &Value::new_string(ctx, &r.url)).unwrap();
                obj.set_property("body", &Value::new_string(ctx, &body_text))
                    .unwrap();
                obj.set_property("bytes", &Value::new_uint8_array(ctx, r.body))
                    .unwrap();
                let h_v = ctx.eval("({})", Some("[fetch-headers]")).unwrap();
                let h = h_v.to_object().unwrap();
                for (k, val) in r.headers {
                    h.set_property(&k.to_lowercase(), &Value::new_string(ctx, &val))
                        .unwrap();
                }
                obj.set_property("headers", &h_v).unwrap();
                unsafe {
                    let resolve_obj = bun_jsc::Object::from_raw_for_runtime(ctx, resolve);
                    let _ = resolve_obj.call(None, &[v]);
                }
            }
            Err(msg) => unsafe {
                let err = bun_jsc::JsString::adopt(sys::JSStringCreateWithUTF8CString(
                    std::ffi::CString::new(msg).unwrap().into_raw(),
                ));
                let err_val = sys::JSValueMakeString(ctx_raw, err.as_raw());
                let err_v = Value::from_raw_public(ctx, err_val);
                let reject_obj = bun_jsc::Object::from_raw_for_runtime(ctx, reject);
                let _ = reject_obj.call(None, &[err_v]);
            },
        }
        unsafe {
            sys::JSValueUnprotect(ctx_raw, resolve as sys::JSValueRef);
            sys::JSValueUnprotect(ctx_raw, reject as sys::JSValueRef);
        }
        crate::async_rt::note_finished();
    });
}
