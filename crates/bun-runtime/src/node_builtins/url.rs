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

#[cfg(test)]
mod tests {
    use super::*;

    // ── decode_percent ────────────────────────────────────────────

    #[test]
    fn decode_passthrough() {
        assert_eq!(decode_percent("hello"), "hello");
        assert_eq!(decode_percent(""), "");
    }

    #[test]
    fn decode_simple_percent() {
        assert_eq!(decode_percent("hello%20world"), "hello world");
        assert_eq!(decode_percent("%2F"), "/");
        assert_eq!(decode_percent("%2f"), "/"); // lowercase hex
    }

    #[test]
    fn decode_utf8_multibyte() {
        // "é" in UTF-8 is C3 A9
        assert_eq!(decode_percent("caf%C3%A9"), "café");
    }

    #[test]
    fn decode_malformed_kept_verbatim() {
        // Trailing % with no two hex digits → kept as-is.
        assert_eq!(decode_percent("100%"), "100%");
        // % followed by non-hex digits → kept as-is.
        assert_eq!(decode_percent("%ZZ"), "%ZZ");
        // Just one hex digit after % → kept as-is.
        assert_eq!(decode_percent("%2"), "%2");
    }

    // ── hex ───────────────────────────────────────────────────────

    #[test]
    fn hex_digit_table() {
        assert_eq!(hex(b'0'), Some(0));
        assert_eq!(hex(b'9'), Some(9));
        assert_eq!(hex(b'a'), Some(10));
        assert_eq!(hex(b'f'), Some(15));
        assert_eq!(hex(b'A'), Some(10));
        assert_eq!(hex(b'F'), Some(15));
        assert_eq!(hex(b'g'), None);
        assert_eq!(hex(b'!'), None);
    }

    // ── encode_percent ────────────────────────────────────────────

    #[test]
    fn encode_passes_through_unreserved() {
        // ALPHA / DIGIT / "-" / "." / "_" / "~" / "/" all stay.
        assert_eq!(
            encode_percent("abcXYZ0123/._-~"),
            "abcXYZ0123/._-~"
        );
    }

    #[test]
    fn encode_encodes_space_and_special() {
        assert_eq!(encode_percent("hello world"), "hello%20world");
        assert_eq!(encode_percent("?"), "%3F");
        assert_eq!(encode_percent("&="), "%26%3D");
    }

    #[test]
    fn encode_utf8_multibyte() {
        // "é" → C3 A9 → "%C3%A9"
        assert_eq!(encode_percent("café"), "caf%C3%A9");
        // emoji (4-byte UTF-8): 🦀 = F0 9F A6 80
        assert_eq!(encode_percent("🦀"), "%F0%9F%A6%80");
    }

    #[test]
    fn encode_decode_round_trip() {
        for s in &[
            "hello world",
            "/path/to/thing?x=1&y=2",
            "café 🦀 v1.0",
            "",
        ] {
            assert_eq!(decode_percent(&encode_percent(s)), *s);
        }
    }
}
