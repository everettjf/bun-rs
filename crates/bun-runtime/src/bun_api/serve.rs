//! `Bun.serve({ port, fetch })` — hyper-backed, concurrent.
//!
//! Architecture:
//!   - tokio task: accept loop on a TcpListener.
//!   - per-connection task: hyper `serve_connection`, which dispatches
//!     each request to a hyper service.
//!   - the service builds a `RequestInfo`, allocates a respond-id,
//!     posts a JS task to the JS thread that:
//!         1. constructs `new Request(...)`,
//!         2. calls the user's `fetch(req)`,
//!         3. attaches `.then(resp => __bun_serve_respond(id, resp))`.
//!   - the service awaits a tokio oneshot Receiver corresponding to that
//!     id and writes the response back to the wire.
//!
//! This means handlers run on the JS thread (serialized at the call point)
//! but their responses are awaited concurrently — a slow `await fetch(...)`
//! inside one handler doesn't stall acceptance of new requests.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper::{Request as HyperRequest, Response as HyperResponse, StatusCode};

/// Map from respond-id → tokio oneshot Sender. Hyper service tasks consume
/// these by awaiting their Receiver.
static RESPONDERS: std::sync::OnceLock<
    Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HandlerResult>>>,
> = std::sync::OnceLock::new();

fn responders() -> &'static Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HandlerResult>>> {
    RESPONDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_RESPOND_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct HandlerResult {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Live servers — used by the event loop to decide whether to stay running.
struct ServerState {
    handler_raw: usize,
    stop_flag: Arc<AtomicBool>,
    ctx_raw: sys::JSGlobalContextRef,
}

thread_local! {
    static SERVERS: std::cell::RefCell<Vec<ServerState>> = std::cell::RefCell::new(Vec::new());
}

pub fn install(ctx: &Context, bun: &bun_jsc::Object<'_>) {
    // Install the response-write callbacks once (idempotent via property check).
    install_response_callbacks(ctx);

    super::bind(ctx, bun, "serve", |args| {
        let opts = args.get(0);
        if !opts.is_object() {
            return Err("Bun.serve requires an options object".into());
        }
        let opts_obj = opts.to_object().map_err(|e| e.to_string())?;
        let port = opts_obj
            .get_property("port")
            .map(|v| v.to_number() as u16)
            .unwrap_or(3000);

        // If the caller used `routes` instead of `fetch`, synthesize a
        // fetch handler that dispatches by URL path and HTTP method. This
        // matches Bun's `routes:` config shape (each value is either a
        // handler function, a `{ GET, POST, … }` map, a Response, or a
        // string/Bun.file body).
        let has_fetch = opts_obj
            .get_property("fetch")
            .map(|v| v.to_object().map(|o| o.is_function()).unwrap_or(false))
            .unwrap_or(false);
        let has_routes = opts_obj
            .get_property("routes")
            .map(|v| v.is_object())
            .unwrap_or(false);
        if !has_fetch && has_routes {
            let synth = args.context().eval(
                r#"(function(routes){
                    return async function (req) {
                        const url = new URL(req.url);
                        const path = url.pathname;
                        const method = (req.method || "GET").toUpperCase();
                        let route = routes[path];
                        if (route === undefined) {
                            // Try a one-level fall-through for `/`.
                            route = routes["/" + path.split("/").filter(Boolean)[0]];
                        }
                        if (route === undefined) return new Response("Not Found", { status: 404 });
                        if (typeof route === "function") return route(req);
                        if (route instanceof Response) return route;
                        if (typeof route === "string") return new Response(route);
                        if (route && typeof route === "object") {
                            const fn = route[method] || route.fetch;
                            if (typeof fn === "function") return fn(req);
                            return new Response("Method Not Allowed", { status: 405 });
                        }
                        return new Response("Not Found", { status: 404 });
                    };
                })"#,
                Some("[serve-routes-wrap]"),
            ).map_err(|e| e.to_string())?;
            let routes_v = opts_obj.get_property("routes").map_err(|e| e.to_string())?;
            let synth_o = synth.to_object().map_err(|e| e.to_string())?;
            let fetch_fn = synth_o.call(None, &[routes_v]).map_err(|e| e.to_string())?;
            opts_obj.set_property("fetch", &fetch_fn).map_err(|e| e.to_string())?;
        }

        let handler_val = opts_obj
            .get_property("fetch")
            .map_err(|e| e.to_string())?;
        let handler_obj = handler_val.to_object().map_err(|e| e.to_string())?;
        if !handler_obj.is_function() {
            return Err("Bun.serve: `fetch` must be a function".into());
        }

        // Optional TLS config: { tls: { key: "<pem>" | path, cert: "<pem>" | path } }
        let tls_config: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>> =
            if let Ok(tls_v) = opts_obj.get_property("tls") {
                if tls_v.is_object() {
                    Some(build_tls_config(&tls_v).map_err(|e| format!("Bun.serve: tls: {e}"))?)
                } else {
                    None
                }
            } else {
                None
            };

        // Protect the handler so we can hand it to the per-request JS tasks.
        unsafe {
            sys::JSValueProtect(args.context().as_raw(), handler_obj.as_raw() as sys::JSValueRef);
        }
        let handler_raw = handler_obj.as_raw() as usize;
        let stop_flag = Arc::new(AtomicBool::new(false));

        // Bind synchronously on the JS thread so we can return the resolved
        // port immediately AND avoid a race between "Bun.serve returned" and
        // "tokio actually listening". We hand the std listener to tokio.
        let std_listener = match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => return Err(format!("Bun.serve: bind port {port} failed: {e}")),
        };
        let resolved_port = std_listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(port);
        std_listener
            .set_nonblocking(true)
            .expect("set_nonblocking");

        // Spawn the accept loop on tokio.
        let stop_clone = stop_flag.clone();
        let tls_cfg = tls_config.clone();
        crate::async_rt::spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Bun.serve: from_std failed: {e}");
                    return;
                }
            };
            let acceptor = tls_cfg.as_ref().map(|c| tokio_rustls::TlsAcceptor::from(c.clone()));
            loop {
                if stop_clone.load(Ordering::SeqCst) { break; }
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _addr) = match accepted {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        let acceptor = acceptor.clone();
                        let stop = stop_clone.clone();
                        tokio::spawn(async move {
                            let _ = &stop;
                            let svc = service_fn(move |req| handle_request(handler_raw, req));
                            if let Some(acc) = acceptor {
                                let stream = match acc.accept(stream).await {
                                    Ok(s) => s,
                                    Err(_) => return,
                                };
                                let proto = stream.get_ref().1
                                    .alpn_protocol()
                                    .map(|p| p.to_vec())
                                    .unwrap_or_default();
                                let io = hyper_util::rt::TokioIo::new(stream);
                                if proto == b"h2" {
                                    let _ = hyper::server::conn::http2::Builder::new(
                                        hyper_util::rt::TokioExecutor::new(),
                                    )
                                    .serve_connection(io, svc).await;
                                } else {
                                    let _ = hyper::server::conn::http1::Builder::new()
                                        .serve_connection(io, svc).await;
                                }
                            } else {
                                let io = hyper_util::rt::TokioIo::new(stream);
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(io, svc).await;
                            }
                        });
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
                }
            }
        });

        // Register on the JS thread so the event loop stays alive.
        let ctx_raw = args.context().as_global_raw();
        SERVERS.with(|s| {
            s.borrow_mut().push(ServerState {
                handler_raw,
                stop_flag: stop_flag.clone(),
                ctx_raw,
            });
        });

        // Return a Server-like object.
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
        let stop_clone2 = stop_flag.clone();
        let stop_cb = Callback::new(ctx, "stop", move |args| {
            stop_clone2.store(true, Ordering::SeqCst);
            // Best-effort wake: connect to our own port so accept() unblocks.
            let _ = std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{resolved_port}").parse().unwrap(),
                std::time::Duration::from_millis(50),
            );
            Ok(Value::new_undefined(args.context()))
        });
        obj.set_property("stop", &stop_cb.value_in(ctx)).unwrap();
        std::mem::forget(stop_cb);

        // Make .url a URL object (Bun does this — tests access .url.host,
        // .url.protocol, etc.). Also install Symbol.dispose / .asyncDispose
        // so `using server = Bun.serve(...)` cleans up automatically. Also
        // shape introspectable methods (.fetch, .publish, .upgrade, .requestIP).
        let _ = ctx.eval(
            &format!(
                r#"(function(s){{
                    try {{ s.url = new URL("http://localhost:{port}/"); }} catch {{}}
                    Object.defineProperty(s, Symbol.dispose, {{ value: () => s.stop(true), configurable: true }});
                    Object.defineProperty(s, Symbol.asyncDispose, {{ value: async () => s.stop(true), configurable: true }});
                    s.ref = () => {{}};
                    s.unref = () => {{}};
                    s.reload = () => {{}};
                    s.development = false;
                    s.address = {{ family: "IPv4", address: "127.0.0.1", port: {port} }};
                    s.fetch = function (req) {{
                        try {{
                            // Type validation matching Bun: rejects primitives.
                            const t = typeof req;
                            if (t === "bigint") return Promise.reject(new TypeError("fetch() expects a string, but received BigInt"));
                            if (t === "symbol") return Promise.reject(new TypeError("fetch() expects a string, but received Symbol"));
                            if (t === "boolean") return Promise.reject(new TypeError("fetch() expects a string, but received Boolean"));
                            if (t === "number") return Promise.reject(new TypeError("fetch() expects a string, but received Number"));
                            const r = (typeof req === "string" || req instanceof URL) ? new Request(req) : req;
                            return Promise.resolve(new Response("", {{ status: 404 }}));
                        }} catch (e) {{
                            return Promise.reject(e);
                        }}
                    }};
                    s.publish = () => 0;
                    s.upgrade = () => false;
                    s.requestIP = () => null;
                    s.subscriberCount = () => 0;
                    s.pendingWebSockets = 0;
                    s.pendingRequests = 0;
                    return s;
                }})"#,
                port = resolved_port,
            ),
            Some("[serve-augment]"),
        )
        .and_then(|f| f.to_object().and_then(|o| o.call(None, &[v])));

        Ok(v)
    });
}

fn build_tls_config(
    tls_v: &Value<'_>,
) -> Result<std::sync::Arc<tokio_rustls::rustls::ServerConfig>, String> {
    use tokio_rustls::rustls::pki_types::CertificateDer;

    let obj = tls_v.to_object().map_err(|e| e.to_string())?;
    let key_v = obj.get_property("key").map_err(|e| e.to_string())?;
    let cert_v = obj.get_property("cert").map_err(|e| e.to_string())?;
    let key_pem = read_pem_or_path(&key_v)?;
    let cert_pem = read_pem_or_path(&cert_v)?;

    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("parse cert: {e}"))?;
    if certs.is_empty() {
        return Err("no certificates found in `cert`".into());
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .map_err(|e| format!("parse key: {e}"))?
        .ok_or_else(|| "no private key found in `key`".to_string())?;

    let mut config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("server config: {e}"))?;
    // ALPN: prefer h2 then http/1.1 so HTTP/2-capable clients upgrade.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(std::sync::Arc::new(config))
}

fn read_pem_or_path(v: &Value<'_>) -> Result<Vec<u8>, String> {
    if let Some(b) = v.typed_array_bytes() {
        return Ok(b.to_vec());
    }
    let mut s = v.to_string();
    // If the value looks like a PEM blob, use it directly.
    if s.contains("-----BEGIN ") {
        return Ok(s.into_bytes());
    }
    // file:// URL → strip scheme and decode percent-escapes.
    if s.starts_with("file://") {
        let raw = &s[7..];
        s = match urlencoding_decode(raw) {
            Ok(d) => d,
            Err(_) => raw.to_string(),
        };
    }
    std::fs::read(&s).map_err(|e| format!("read {s}: {e}"))
}

fn urlencoding_decode(s: &str) -> Result<String, std::str::Utf8Error> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = h { out.push(b); i += 3; continue; }
        }
        out.push(bytes[i]); i += 1;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

async fn handle_request(
    handler_raw: usize,
    req: HyperRequest<hyper::body::Incoming>,
) -> Result<HyperResponse<Full<Bytes>>, Infallible> {
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    let url = format!("http://localhost{}", uri.path_and_query().map(|p| p.as_str()).unwrap_or(""));
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect();
    let body_bytes = match req.collect().await {
        Ok(c) => c.to_bytes().to_vec(),
        Err(_) => Vec::new(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel::<HandlerResult>();
    let respond_id = NEXT_RESPOND_ID.fetch_add(1, Ordering::SeqCst);
    responders().lock().unwrap().insert(respond_id, tx);

    crate::async_rt::post_to_js(move |ctx| {
        let handler_v = unsafe {
            Value::from_raw_public(ctx, handler_raw as sys::JSValueRef)
        };
        let handler_obj = match handler_v.to_object() {
            Ok(o) => o,
            Err(_) => {
                resolve_error(respond_id, "handler invalid");
                return;
            }
        };

        let req_obj = match build_js_request(ctx, &method, &url, &headers, &body_bytes) {
            Ok(v) => v,
            Err(msg) => {
                resolve_error(respond_id, &msg);
                return;
            }
        };

        // Call handler(request). Result may be a Response or a Promise<Response>.
        let result = match handler_obj.call(None, &[req_obj]) {
            Ok(v) => v,
            Err(e) => {
                resolve_error(respond_id, &e.to_string());
                return;
            }
        };

        // Wrap with Promise.resolve so we always have a thenable.
        let wrap = ctx
            .eval(
                "((p, id) => Promise.resolve(p).then(r => globalThis.__bun_serve_respond(id, r), e => globalThis.__bun_serve_respond_error(id, e)))",
                Some("[serve-wrap]"),
            )
            .ok()
            .and_then(|v| v.to_object().ok());
        if let Some(wrap) = wrap {
            let _ = wrap.call(None, &[result, Value::new_number(ctx, respond_id as f64)]);
        }
    });

    let result = match rx.await {
        Ok(r) => r,
        Err(_) => HandlerResult {
            status: 500,
            headers: vec![],
            body: b"Bun.serve: handler dropped".to_vec(),
        },
    };

    let mut builder = HyperResponse::builder().status(StatusCode::from_u16(result.status).unwrap_or(StatusCode::OK));
    for (k, v) in &result.headers {
        builder = builder.header(k, v);
    }
    let body = Full::new(Bytes::from(result.body));
    Ok(builder.body(body).unwrap_or_else(|_| {
        HyperResponse::new(Full::new(Bytes::from_static(b"")))
    }))
}

fn resolve_error(id: u64, msg: &str) {
    let tx = responders().lock().unwrap().remove(&id);
    if let Some(tx) = tx {
        let _ = tx.send(HandlerResult {
            status: 500,
            headers: vec![],
            body: msg.as_bytes().to_vec(),
        });
    }
}

fn build_js_request<'ctx>(
    ctx: &'ctx Context,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Value<'ctx>, String> {
    let request_ctor = ctx
        .global_object()
        .get_property("Request")
        .and_then(|v| v.to_object())
        .map_err(|e| e.to_string())?;
    let init_v = ctx.eval("({})", Some("[serve-init]")).unwrap();
    let init = init_v.to_object().map_err(|e| e.to_string())?;
    init.set_property("method", &Value::new_string(ctx, method))
        .map_err(|e| e.to_string())?;
    if !body.is_empty() {
        // Pass as Uint8Array (zero-copy) so binary bodies survive intact.
        let bytes = body.to_vec();
        init.set_property("body", &Value::new_uint8_array(ctx, bytes))
            .map_err(|e| e.to_string())?;
    }
    let h_v = ctx.eval("({})", Some("[serve-req-headers]")).unwrap();
    let h = h_v.to_object().map_err(|e| e.to_string())?;
    for (k, v) in headers {
        let _ = h.set_property(k, &Value::new_string(ctx, v));
    }
    init.set_property("headers", &h_v).map_err(|e| e.to_string())?;
    let url_v = Value::new_string(ctx, url);
    request_ctor
        .construct(&[url_v, init_v])
        .map_err(|e| e.to_string())
}

fn install_response_callbacks(ctx: &Context) {
    // Idempotent: skip if already installed.
    if ctx
        .global_object()
        .get_property("__bun_serve_respond")
        .map(|v| !v.is_undefined())
        .unwrap_or(false)
    {
        return;
    }

    let respond_cb = Callback::new(ctx, "__bun_serve_respond", |args| {
        let id = args.get(0).to_number() as u64;
        let response_v = args.get(1);
        let response_obj = match response_v.to_object() {
            Ok(o) => o,
            Err(_) => {
                resolve_error(id, "Bun.serve: handler returned a non-Response value");
                return Ok(Value::new_undefined(args.context()));
            }
        };

        // Pull status + body + headers off the Response (polyfill stores
        // body as _body / _bodyBytes, headers as a Headers wrapping a Map).
        let status = response_obj
            .get_property("status")
            .map(|v| v.to_number() as u16)
            .unwrap_or(200);
        let body_bytes: Vec<u8> = response_obj
            .get_property("_bodyBytes")
            .ok()
            .and_then(|v| {
                if v.is_object() {
                    v.typed_array_bytes().map(|b| b.to_vec())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                response_obj
                    .get_property("_body")
                    .map(|v| v.to_string().into_bytes())
                    .unwrap_or_default()
            });
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Ok(h) = response_obj.get_property("headers") {
            if h.is_object() {
                let extract = args
                    .context()
                    .eval(
                        "(h) => { const out = []; for (const [k,v] of h) out.push([k, v]); return out; }",
                        Some("[serve-headers]"),
                    )
                    .ok()
                    .and_then(|v| v.to_object().ok());
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
        let tx = responders().lock().unwrap().remove(&id);
        if let Some(tx) = tx {
            let _ = tx.send(HandlerResult {
                status,
                headers,
                body: body_bytes,
            });
        }
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_serve_respond", &respond_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(respond_cb);

    let err_cb = Callback::new(ctx, "__bun_serve_respond_error", |args| {
        let id = args.get(0).to_number() as u64;
        let err = args.get(1);
        let msg = err.to_string();
        resolve_error(id, &msg);
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_serve_respond_error", &err_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(err_cb);
}

// ── Event-loop integration ──────────────────────────────────────────

pub fn any_active() -> bool {
    SERVERS.with(|s| {
        s.borrow()
            .iter()
            .any(|sv| !sv.stop_flag.load(Ordering::SeqCst))
    })
}

/// poll_one is now a no-op — concurrency is managed entirely by tokio.
/// Kept for API compatibility with the old code path.
pub fn poll_one(_ctx: &Context, _timeout: std::time::Duration) -> bool {
    false
}
