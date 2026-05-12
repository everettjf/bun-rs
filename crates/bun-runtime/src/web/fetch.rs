//! `globalThis.__bun_fetch(url, init)` — blocking HTTP via ureq, returns the
//! raw response shape the JS polyfill turns into a `Response`.
//!
//! Blocking is fine for MVP; the entire JS thread waits during the request.
//! When tokio lands the call signature stays the same — only the
//! implementation flips to async + a deferred Promise.

use std::io::Read;

use bun_jsc::{Callback, Context, Value};

pub fn install(ctx: &Context) {
    let cb = Callback::new(ctx, "__bun_fetch", |args| {
        let url = args.get(0).to_string();

        // init: { method, headers, body }
        let init = args.get(1);
        let method = if init.is_object() {
            init.to_object()
                .ok()
                .and_then(|o| o.get_property("method").ok())
                .map(|v| v.to_string().to_uppercase())
                .unwrap_or_else(|| "GET".into())
        } else {
            "GET".into()
        };

        // Headers + body extracted up-front.
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

        let body_text: Option<String> = if init.is_object() {
            init.to_object()
                .ok()
                .and_then(|o| o.get_property("body").ok())
                .filter(|v| !v.is_undefined() && !v.is_null())
                .map(|v| v.to_string())
        } else {
            None
        };

        // ureq 3.x: one builder per method. We dispatch by method string.
        let mut resp = match method.as_str() {
            "GET" => {
                let mut b = ureq::get(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                b.call().map_err(|e| format!("fetch error: {e}"))?
            }
            "HEAD" => {
                let mut b = ureq::head(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                b.call().map_err(|e| format!("fetch error: {e}"))?
            }
            "DELETE" => {
                let mut b = ureq::delete(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                b.call().map_err(|e| format!("fetch error: {e}"))?
            }
            "POST" => {
                let mut b = ureq::post(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                let body = body_text.unwrap_or_default();
                b.send(body.as_bytes())
                    .map_err(|e| format!("fetch error: {e}"))?
            }
            "PUT" => {
                let mut b = ureq::put(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                let body = body_text.unwrap_or_default();
                b.send(body.as_bytes())
                    .map_err(|e| format!("fetch error: {e}"))?
            }
            "PATCH" => {
                let mut b = ureq::patch(&url);
                for (k, v) in &headers_kv {
                    b = b.header(k, v);
                }
                let body = body_text.unwrap_or_default();
                b.send(body.as_bytes())
                    .map_err(|e| format!("fetch error: {e}"))?
            }
            other => return Err(format!("unsupported method: {other}")),
        };

        let status = resp.status().as_u16() as u32;
        let mut headers_pairs: Vec<(String, String)> = Vec::new();
        for (name, val) in resp.headers() {
            if let Ok(s) = val.to_str() {
                headers_pairs.push((name.as_str().to_string(), s.to_string()));
            }
        }
        let mut body = Vec::new();
        resp.body_mut().as_reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
        let body_text = String::from_utf8_lossy(&body).into_owned();

        let ctx = args.context();
        let v = ctx.eval("({})", Some("[fetch-result]")).unwrap();
        let obj = v.to_object().unwrap();
        obj.set_property("status", &Value::new_number(ctx, status as f64))
            .unwrap();
        obj.set_property("url", &Value::new_string(ctx, &url)).unwrap();
        obj.set_property("body", &Value::new_string(ctx, &body_text))
            .unwrap();
        // bytes view — zero-copy Uint8Array so binary responses survive intact.
        obj.set_property("bytes", &Value::new_uint8_array(ctx, body))
            .unwrap();

        // headers as plain object
        let h_v = ctx.eval("({})", Some("[fetch-headers]")).unwrap();
        let h = h_v.to_object().unwrap();
        for (k, val) in headers_pairs {
            // Lowercase keys to match WHATWG Headers normalization.
            h.set_property(&k.to_lowercase(), &Value::new_string(ctx, &val))
                .unwrap();
        }
        obj.set_property("headers", &h_v).unwrap();

        Ok(v)
    });
    ctx.global_object()
        .set_property("__bun_fetch", &cb.value_in(ctx))
        .unwrap();
    std::mem::forget(cb);
}
