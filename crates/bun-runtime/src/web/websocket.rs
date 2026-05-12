//! `globalThis.WebSocket` — client only.
//!
//! JS surface (WHATWG WebSocket subset):
//!   new WebSocket(url, protocols?)
//!     .readyState (CONNECTING=0, OPEN=1, CLOSING=2, CLOSED=3)
//!     .url
//!     .protocol
//!     .send(data)       — string | Uint8Array | ArrayBuffer
//!     .close(code?, reason?)
//!     .onopen / .onmessage / .onerror / .onclose
//!     addEventListener("open" | "message" | "error" | "close", fn)
//!
//! Not implemented: BinaryType selection (we always deliver as Uint8Array
//! for binary frames), ping/pong customization, extensions / compression
//! negotiation beyond what tungstenite picks on its own.
//!
//! Implementation: each `new WebSocket(url)` allocates a unique connection
//! id and a tokio task that drives a `tokio_tungstenite` connection. The
//! task reads from a mpsc::UnboundedReceiver<OutMsg> for outgoing frames
//! and a `WebSocketStream` for incoming. All callback dispatch happens on
//! the JS thread via `async_rt::post_to_js`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bun_jsc::{Callback, Context, Value};
use bun_jsc_sys as sys;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

enum OutMsg {
    Text(String),
    Binary(Vec<u8>),
    Close(Option<(u16, String)>),
}

struct WsHandle {
    tx: mpsc::UnboundedSender<OutMsg>,
}

static SOCKETS: OnceLock<Mutex<HashMap<u64, WsHandle>>> = OnceLock::new();
fn sockets() -> &'static Mutex<HashMap<u64, WsHandle>> {
    SOCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn install(ctx: &Context) {
    // Rust-side: open/send/close globals consumed by the JS class.
    let open_cb = Callback::new(ctx, "__bun_ws_open", |args| {
        let url = args.get(0).to_string();
        let on_event_v = args.get(1);
        if !on_event_v.is_object() {
            return Err("__bun_ws_open: missing on_event callback".into());
        }
        let on_event_obj = on_event_v.to_object().map_err(|e| e.to_string())?;
        unsafe {
            sys::JSValueProtect(
                args.context().as_raw(),
                on_event_obj.as_raw() as sys::JSValueRef,
            );
        }
        let on_event_id = on_event_obj.as_raw() as usize;
        let (tx, mut rx) = mpsc::unbounded_channel::<OutMsg>();
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        sockets().lock().unwrap().insert(id, WsHandle { tx });

        crate::async_rt::note_started();
        crate::async_rt::spawn(async move {
            let connect = tokio_tungstenite::connect_async(&url).await;
            let (mut ws, _resp) = match connect {
                Ok(p) => p,
                Err(e) => {
                    dispatch_event(on_event_id, "error", EventPayload::Text(e.to_string()));
                    dispatch_event(on_event_id, "close", EventPayload::Close(1006, e.to_string()));
                    crate::async_rt::post_to_js(move |ctx| {
                        unsafe {
                            sys::JSValueUnprotect(
                                ctx.as_raw(),
                                on_event_id as sys::JSValueRef,
                            );
                        }
                        crate::async_rt::note_finished();
                    });
                    return;
                }
            };

            dispatch_event(on_event_id, "open", EventPayload::Empty);

            loop {
                tokio::select! {
                    out = rx.recv() => {
                        match out {
                            Some(OutMsg::Text(s)) => {
                                if ws.send(Message::Text(s.into())).await.is_err() { break; }
                            }
                            Some(OutMsg::Binary(b)) => {
                                if ws.send(Message::Binary(b.into())).await.is_err() { break; }
                            }
                            Some(OutMsg::Close(code_reason)) => {
                                let (code, reason) = code_reason
                                    .clone()
                                    .unwrap_or((1000u16, String::new()));
                                let frame = code_reason.map(|(c, r)| tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(c),
                                    reason: r.into(),
                                });
                                let _ = ws.send(Message::Close(frame)).await;
                                dispatch_event(on_event_id, "close", EventPayload::Close(code, reason));
                                break;
                            }
                            None => break,
                        }
                    }
                    incoming = ws.next() => {
                        match incoming {
                            Some(Ok(Message::Text(t))) => {
                                dispatch_event(on_event_id, "message", EventPayload::Text(t.to_string()));
                            }
                            Some(Ok(Message::Binary(b))) => {
                                dispatch_event(on_event_id, "message", EventPayload::Bytes(b.to_vec()));
                            }
                            Some(Ok(Message::Close(frame))) => {
                                let (code, reason) = match frame {
                                    Some(f) => (u16::from(f.code), f.reason.to_string()),
                                    None => (1005, String::new()),
                                };
                                dispatch_event(on_event_id, "close", EventPayload::Close(code, reason));
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                dispatch_event(on_event_id, "error", EventPayload::Text(e.to_string()));
                                dispatch_event(on_event_id, "close", EventPayload::Close(1006, e.to_string()));
                                break;
                            }
                            None => {
                                dispatch_event(on_event_id, "close", EventPayload::Close(1005, String::new()));
                                break;
                            }
                        }
                    }
                }
            }
            // Cleanup.
            sockets().lock().unwrap().remove(&id);
            crate::async_rt::post_to_js(move |ctx| {
                unsafe {
                    sys::JSValueUnprotect(ctx.as_raw(), on_event_id as sys::JSValueRef);
                }
                crate::async_rt::note_finished();
            });
        });

        Ok(Value::new_number(args.context(), id as f64))
    });
    ctx.global_object()
        .set_property("__bun_ws_open", &open_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(open_cb);

    let send_cb = Callback::new(ctx, "__bun_ws_send", |args| {
        let id = args.get(0).to_number() as u64;
        let data_v = args.get(1);
        let msg = if let Some(bytes) = data_v.typed_array_bytes() {
            OutMsg::Binary(bytes.to_vec())
        } else {
            OutMsg::Text(data_v.to_string())
        };
        if let Some(h) = sockets().lock().unwrap().get(&id) {
            let _ = h.tx.send(msg);
        }
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_ws_send", &send_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(send_cb);

    let close_cb = Callback::new(ctx, "__bun_ws_close", |args| {
        let id = args.get(0).to_number() as u64;
        let code = if args.len() >= 2 && !args.get(1).is_undefined() {
            Some(args.get(1).to_number() as u16)
        } else {
            None
        };
        let reason = if args.len() >= 3 && !args.get(2).is_undefined() {
            args.get(2).to_string()
        } else {
            String::new()
        };
        let msg = if let Some(c) = code {
            OutMsg::Close(Some((c, reason)))
        } else {
            OutMsg::Close(None)
        };
        if let Some(h) = sockets().lock().unwrap().get(&id) {
            let _ = h.tx.send(msg);
        }
        Ok(Value::new_undefined(args.context()))
    });
    ctx.global_object()
        .set_property("__bun_ws_close", &close_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(close_cb);

    // JS WebSocket class — small wrapper around the three globals above.
    ctx.eval(JS_POLYFILL, Some("[WebSocket-polyfill]"))
        .expect("install WebSocket polyfill");
}

enum EventPayload {
    Empty,
    Text(String),
    Bytes(Vec<u8>),
    Close(u16, String),
}

fn dispatch_event(on_event_id: usize, kind: &'static str, payload: EventPayload) {
    crate::async_rt::post_to_js(move |ctx| {
        let on_event = unsafe {
            bun_jsc::Object::from_raw_for_runtime(ctx, on_event_id as sys::JSObjectRef)
        };
        let kind_v = Value::new_string(ctx, kind);
        let payload_v: Value = match payload {
            EventPayload::Empty => Value::new_undefined(ctx),
            EventPayload::Text(s) => Value::new_string(ctx, &s),
            EventPayload::Bytes(b) => crate::buffer::buffer_from_bytes(ctx, b),
            EventPayload::Close(code, reason) => {
                let obj_v = ctx.eval("({})", Some("[close-payload]")).unwrap();
                let obj = obj_v.to_object().unwrap();
                obj.set_property("code", &Value::new_number(ctx, code as f64))
                    .unwrap();
                obj.set_property("reason", &Value::new_string(ctx, &reason))
                    .unwrap();
                obj_v
            }
        };
        let _ = on_event.call(None, &[kind_v, payload_v]);
    });
}

const JS_POLYFILL: &str = r#"
(function (g) {
  class WebSocket {
    constructor(url, protocols) {
      this.url = String(url);
      this.protocol = "";
      this.readyState = 0; // CONNECTING
      this._listeners = { open: [], message: [], error: [], close: [] };
      this.onopen = null;
      this.onmessage = null;
      this.onerror = null;
      this.onclose = null;
      this.binaryType = "uint8array"; // we always deliver Uint8Array for binary

      const onEvent = (kind, payload) => {
        if (kind === "open") {
          this.readyState = 1;
          const ev = { type: "open", target: this };
          this._fire("open", ev);
        } else if (kind === "message") {
          const ev = { type: "message", target: this, data: payload };
          this._fire("message", ev);
        } else if (kind === "error") {
          const ev = { type: "error", target: this, message: payload };
          this._fire("error", ev);
        } else if (kind === "close") {
          this.readyState = 3;
          const ev = { type: "close", target: this, code: payload.code, reason: payload.reason, wasClean: payload.code === 1000 };
          this._fire("close", ev);
        }
      };
      this._id = __bun_ws_open(this.url, onEvent);
    }
    _fire(kind, ev) {
      const handler = this["on" + kind];
      if (typeof handler === "function") {
        try { handler.call(this, ev); } catch (e) { console.error(e); }
      }
      for (const fn of this._listeners[kind].slice()) {
        try { fn.call(this, ev); } catch (e) { console.error(e); }
      }
    }
    addEventListener(type, fn) {
      if (this._listeners[type]) this._listeners[type].push(fn);
    }
    removeEventListener(type, fn) {
      const list = this._listeners[type];
      if (!list) return;
      const i = list.indexOf(fn);
      if (i !== -1) list.splice(i, 1);
    }
    send(data) {
      if (this.readyState !== 1) {
        throw new Error("WebSocket is not open (readyState " + this.readyState + ")");
      }
      __bun_ws_send(this._id, data);
    }
    close(code, reason) {
      if (this.readyState === 2 || this.readyState === 3) return;
      this.readyState = 2; // CLOSING
      __bun_ws_close(this._id, code, reason);
    }
  }
  WebSocket.CONNECTING = 0;
  WebSocket.OPEN = 1;
  WebSocket.CLOSING = 2;
  WebSocket.CLOSED = 3;
  g.WebSocket = WebSocket;
})(globalThis);
"#;
