//! `node:url` — re-exports the global URL / URLSearchParams plus a few
//! convenience helpers (fileURLToPath / pathToFileURL).

use bun_jsc::{Callback, Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:url]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    let url_class = ctx.global_object().get_property("URL").unwrap();
    exports.set_property("URL", &url_class).unwrap();
    let usp = ctx.global_object().get_property("URLSearchParams").unwrap();
    exports.set_property("URLSearchParams", &usp).unwrap();

    let f2p = Callback::new(ctx, "fileURLToPath", |args| {
        let s = args.get(0).to_string();
        let path = if let Some(rest) = s.strip_prefix("file://") {
            // Strip leading slash that file URLs include.
            let raw = if let Some(stripped) = rest.strip_prefix("/") {
                format!("/{}", stripped)
            } else {
                rest.to_string()
            };
            // Percent-decode.
            decode_percent(&raw)
        } else {
            s
        };
        Ok(Value::new_string(args.context(), &path))
    });
    exports.set_property("fileURLToPath", &f2p.value_in(ctx)).unwrap();
    std::mem::forget(f2p);

    let p2f = Callback::new(ctx, "pathToFileURL", |args| {
        let p = args.get(0).to_string();
        let url = format!("file://{}", encode_percent(&p));
        // Need to return a URL instance.
        let ctx = args.context();
        let url_ctor = ctx
            .global_object()
            .get_property("URL")
            .unwrap()
            .to_object()
            .unwrap();
        url_ctor
            .construct(&[Value::new_string(ctx, &url)])
            .map_err(|e| e.to_string())
    });
    exports.set_property("pathToFileURL", &p2f.value_in(ctx)).unwrap();
    std::mem::forget(p2f);

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn encode_percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '.' | '-' | '_' | '~' => out.push(c),
            c if c.is_ascii_alphanumeric() => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}
