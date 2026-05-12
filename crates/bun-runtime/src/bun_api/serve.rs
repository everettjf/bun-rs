//! `Bun.serve({port, fetch})` — minimal HTTP server.
//!
//! Architecture:
//!   - tiny_http listens on a background thread; each request is wrapped in
//!     a `PendingRequest` and pushed onto a thread-local queue (the JS
//!     thread is the only consumer, so a single-producer/single-consumer
//!     channel suffices).
//!   - After the entry module's promise settles, the runtime drains the
//!     queue: each request is marshalled into a JS `Request`, the user's
//!     `fetch` handler is invoked, the resulting `Response` is awaited
//!     synchronously (await_promise), and the response is shipped back via
//!     `tiny_http::Request::respond`.
//!
//! Limitations (deliberate):
//!   - No HTTPS, HTTP/2, websocket upgrade, or streaming bodies.
//!   - `server.stop()` works but `server.reload()` is a no-op.
//!   - Bodies are read fully into memory before the handler runs.
//!   - One-request-at-a-time; long-running handlers block other requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use bun_jsc::{Callback, Context, Value};

use crate::modules::await_promise;

/// One pending HTTP request, plus a slot the JS thread fills with the
/// response that the server thread will write back.
pub struct PendingRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub respond: Box<dyn FnOnce(u16, Vec<(String, String)>, Vec<u8>) + Send>,
}

struct ServerState {
    handler: sys_protect::ProtectedValue,
    pending: Receiver<PendingRequest>,
    stop_flag: Arc<AtomicBool>,
    port: u16,
}

mod sys_protect {
    use bun_jsc_sys as sys;

    /// A JS callback ref kept alive across the FFI boundary. `JSValueProtect`
    /// on creation; `JSValueUnprotect` on drop.
    pub struct ProtectedValue {
        pub raw: sys::JSValueRef,
        pub ctx: sys::JSGlobalContextRef,
    }

    impl ProtectedValue {
        pub fn new(ctx: sys::JSContextRef, raw: sys::JSValueRef) -> Self {
            unsafe {
                sys::JSValueProtect(ctx, raw);
                let g = sys::JSGlobalContextRetain(ctx as sys::JSGlobalContextRef);
                Self { raw, ctx: g }
            }
        }
    }
    impl Drop for ProtectedValue {
        fn drop(&mut self) {
            unsafe {
                sys::JSValueUnprotect(self.ctx as sys::JSContextRef, self.raw);
                sys::JSGlobalContextRelease(self.ctx);
            }
        }
    }
}

// Thread-local list of running servers, drained by the event loop.
thread_local! {
    static SERVERS: std::cell::RefCell<Vec<ServerState>> = std::cell::RefCell::new(Vec::new());
}

pub fn install(ctx: &Context, bun: &bun_jsc::Object<'_>) {
    super::bind(ctx, bun, "serve", |args| {
        let opts = args.get(0);
        if !opts.is_object() {
            return Err("Bun.serve requires an options object".to_string());
        }
        let opts_obj = opts.to_object().map_err(|e| e.to_string())?;

        let port = opts_obj
            .get_property("port")
            .map(|v| v.to_number() as u16)
            .unwrap_or(3000);
        let handler_val = opts_obj
            .get_property("fetch")
            .map_err(|e| e.to_string())?;
        if !handler_val.is_object() {
            return Err("Bun.serve: `fetch` must be a function".to_string());
        }
        let handler_obj = handler_val.to_object().map_err(|e| e.to_string())?;
        if !handler_obj.is_function() {
            return Err("Bun.serve: `fetch` must be a function".to_string());
        }

        // Spin up tiny_http on a worker thread; messages travel back through
        // a channel that the JS thread will drain in its event loop.
        let addr = format!("0.0.0.0:{port}");
        let server = match tiny_http::Server::http(&addr) {
            Ok(s) => s,
            Err(e) => return Err(format!("Bun.serve: bind {addr} failed: {e}")),
        };
        let resolved_port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let (tx, rx): (Sender<PendingRequest>, Receiver<PendingRequest>) = channel();
        let stop_for_thread = stop_flag.clone();

        // Wrap tiny_http::Request in a Box<dyn FnOnce(...)> so the JS thread
        // can respond. tiny_http's Request isn't Send-friendly because its
        // body reader holds a reference into the connection; we wrap with
        // a sync Mutex and move the request through.
        std::thread::spawn(move || {
            let server = Arc::new(server);
            for req in server.incoming_requests() {
                if stop_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                let method = req.method().as_str().to_uppercase();
                let url = format!("http://localhost:{resolved_port}{}", req.url());
                let headers = req
                    .headers()
                    .iter()
                    .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
                    .collect::<Vec<_>>();

                // tiny_http::Request is !Send because of the body reader, but
                // we own it here. Use a Mutex<Option<_>> as a hand-off slot.
                let req_slot: Arc<Mutex<Option<tiny_http::Request>>> = Arc::new(Mutex::new(None));
                let mut req = req;
                // Drain body before crossing thread boundary.
                let mut body = Vec::new();
                use std::io::Read;
                let _ = req.as_reader().read_to_end(&mut body);
                *req_slot.lock().unwrap() = Some(req);
                let req_slot_for_respond = req_slot.clone();

                let respond: Box<dyn FnOnce(u16, Vec<(String, String)>, Vec<u8>) + Send> =
                    Box::new(move |status, hdrs, body| {
                        let mut g = req_slot_for_respond.lock().unwrap();
                        let req = match g.take() {
                            Some(r) => r,
                            None => return,
                        };
                        drop(g);
                        let mut resp = tiny_http::Response::from_data(body)
                            .with_status_code(status as i32);
                        for (k, v) in hdrs {
                            if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                                resp = resp.with_header(h);
                            }
                        }
                        let _ = req.respond(resp);
                    });

                if tx
                    .send(PendingRequest {
                        method,
                        url,
                        headers,
                        body,
                        respond,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // Save server state on the JS thread.
        SERVERS.with(|s| {
            s.borrow_mut().push(ServerState {
                handler: sys_protect::ProtectedValue::new(
                    args.context().as_raw(),
                    handler_obj.as_raw() as bun_jsc_sys::JSValueRef,
                ),
                pending: rx,
                stop_flag: stop_flag.clone(),
                port: resolved_port,
            });
        });

        // Return a Server-like object: { port, hostname, stop, url }.
        let ctx = args.context();
        let v = ctx.eval("({})", Some("[Bun.serve]")).unwrap();
        let obj = v.to_object().unwrap();
        obj.set_property("port", &Value::new_number(ctx, resolved_port as f64))
            .unwrap();
        obj.set_property("hostname", &Value::new_string(ctx, "localhost"))
            .unwrap();
        obj.set_property(
            "url",
            &Value::new_string(ctx, &format!("http://localhost:{resolved_port}/")),
        )
        .unwrap();
        let stop_flag_for_stop = stop_flag.clone();
        let stop_cb = Callback::new(ctx, "stop", move |args| {
            stop_flag_for_stop.store(true, Ordering::SeqCst);
            // Best-effort: punch a tiny connection through so tiny_http
            // wakes from incoming_requests().
            let port_str = std::env::var("BUN_RS_STOP_PORT").unwrap_or_default();
            if !port_str.is_empty() {
                let _ = std::net::TcpStream::connect(format!("127.0.0.1:{port_str}"));
            }
            Ok(Value::new_undefined(args.context()))
        });
        obj.set_property("stop", &stop_cb.value_in(ctx)).unwrap();
        std::mem::forget(stop_cb);

        Ok(v)
    });
}

/// Are there any live servers? Used by the event loop to decide whether to
/// keep blocking.
pub fn any_active() -> bool {
    SERVERS.with(|s| {
        s.borrow()
            .iter()
            .any(|sv| !sv.stop_flag.load(Ordering::SeqCst))
    })
}

/// Drain one pending request across any server, blocking for up to
/// `timeout`. Returns `true` if it handled something.
pub fn poll_one(ctx: &Context, timeout: std::time::Duration) -> bool {
    // We just probe each receiver in turn. Cheap with few servers.
    let next = SERVERS.with(|s| {
        let mut all = s.borrow_mut();
        // Tidy: drop stopped servers.
        all.retain(|sv| !sv.stop_flag.load(Ordering::SeqCst));
        for sv in all.iter() {
            if let Ok(req) = sv.pending.try_recv() {
                return Some((req, sv.handler.raw));
            }
        }
        None
    });

    let (req, handler_raw) = match next {
        Some(x) => x,
        None => {
            std::thread::sleep(timeout.min(std::time::Duration::from_millis(20)));
            return false;
        }
    };

    handle_request(ctx, handler_raw, req);
    true
}

fn handle_request(ctx: &Context, handler_raw: bun_jsc_sys::JSValueRef, req: PendingRequest) {
    // Build a JS Request from the pending data.
    let request_v = match build_js_request(ctx, &req) {
        Ok(v) => v,
        Err(msg) => {
            (req.respond)(500, vec![], format!("Bun.serve internal error: {msg}").into_bytes());
            return;
        }
    };

    // Call handler(request).
    let handler_obj = unsafe { bun_jsc::Value::from_raw_public(ctx, handler_raw) };
    let handler = match handler_obj.to_object() {
        Ok(o) => o,
        Err(e) => {
            (req.respond)(500, vec![], e.to_string().into_bytes());
            return;
        }
    };
    let result = handler.call(None, &[request_v]);
    let response_v = match result {
        Ok(v) => v,
        Err(e) => {
            (req.respond)(500, vec![], e.to_string().into_bytes());
            return;
        }
    };

    // If handler returned a promise, await it.
    let response_v = match await_promise(ctx, response_v) {
        Ok(v) => v,
        Err(e) => {
            (req.respond)(500, vec![], e.into_bytes());
            return;
        }
    };

    // Pull fields off the JS Response.
    let resp_obj = match response_v.to_object() {
        Ok(o) => o,
        Err(e) => {
            (req.respond)(500, vec![], e.to_string().into_bytes());
            return;
        }
    };
    let status = resp_obj
        .get_property("status")
        .map(|v| v.to_number() as u16)
        .unwrap_or(200);
    let body_str = resp_obj
        .get_property("_body")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Ok(h) = resp_obj.get_property("headers") {
        if h.is_object() {
            // Headers polyfill stores entries on `_map` (a JS Map). We use the
            // public iteration via Map.prototype.entries through a helper.
            let extract = ctx
                .eval(
                    "(h) => { const out = []; for (const [k,v] of h) out.push([k, v]); return out; }",
                    Some("[serve-headers]"),
                )
                .unwrap()
                .to_object()
                .ok();
            if let Some(extract) = extract {
                if let Ok(arr) = extract.call(None, &[h]) {
                    if let Ok(arr_obj) = arr.to_object() {
                        let len = arr_obj
                            .get_property("length")
                            .map(|v| v.to_number() as u32)
                            .unwrap_or(0);
                        for i in 0..len {
                            if let Ok(entry) = arr_obj.get_property_at(i) {
                                if let Ok(eo) = entry.to_object() {
                                    let k = eo
                                        .get_property_at(0)
                                        .map(|v| v.to_string())
                                        .unwrap_or_default();
                                    let v = eo
                                        .get_property_at(1)
                                        .map(|v| v.to_string())
                                        .unwrap_or_default();
                                    headers.push((k, v));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (req.respond)(status, headers, body_str.into_bytes());
}

fn build_js_request<'ctx>(
    ctx: &'ctx Context,
    req: &PendingRequest,
) -> Result<Value<'ctx>, String> {
    // Use the Request polyfill installed by web::install_web.
    let global = ctx.global_object();
    let request_ctor = global
        .get_property("Request")
        .map_err(|e| e.to_string())?
        .to_object()
        .map_err(|e| e.to_string())?;

    let init_v = ctx.eval("({})", Some("[serve-init]")).unwrap();
    let init = init_v.to_object().map_err(|e| e.to_string())?;
    init.set_property("method", &Value::new_string(ctx, &req.method))
        .map_err(|e| e.to_string())?;
    if !req.body.is_empty() {
        let body = String::from_utf8_lossy(&req.body).into_owned();
        init.set_property("body", &Value::new_string(ctx, &body))
            .map_err(|e| e.to_string())?;
    }

    let headers_v = ctx.eval("({})", Some("[serve-req-headers]")).unwrap();
    let headers = headers_v.to_object().map_err(|e| e.to_string())?;
    for (k, v) in &req.headers {
        let _ = headers.set_property(k, &Value::new_string(ctx, v));
    }
    init.set_property("headers", &headers_v)
        .map_err(|e| e.to_string())?;

    let url_v = Value::new_string(ctx, &req.url);
    let req_obj = request_ctor
        .construct(&[url_v, init_v])
        .map_err(|e| e.to_string())?;
    Ok(req_obj)
}
