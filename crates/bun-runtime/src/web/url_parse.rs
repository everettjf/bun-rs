//! Rust-backed URL parser exposed as `globalThis.__bun_parse_url`.
//!
//! Returns either an object with the parsed components, or throws (so JS
//! `new URL(...)` throws TypeError on invalid inputs).

use bun_jsc::{Callback, Context, Value};

pub fn install(ctx: &Context) {
    let cb = Callback::new(ctx, "__bun_parse_url", |args| {
        let input = args.get(0).to_string();
        let base = if args.len() >= 2 && !args.get(1).is_undefined() && !args.get(1).is_null() {
            Some(args.get(1).to_string())
        } else {
            None
        };

        let parsed = match (base.as_deref(), url::Url::parse(&input)) {
            (_, Ok(u)) => u,
            (Some(b), Err(_)) => match url::Url::parse(b).and_then(|b| b.join(&input)) {
                Ok(u) => u,
                Err(_) => return Err(format!("Invalid URL: {input:?}")),
            },
            (None, Err(_)) => return Err(format!("Invalid URL: {input:?}")),
        };

        let ctx = args.context();
        let obj_v = ctx.eval("({})", Some("[url-parse]")).unwrap();
        let obj = obj_v.to_object().unwrap();
        obj.set_property("href", &Value::new_string(ctx, parsed.as_str()))
            .unwrap();
        obj.set_property("origin", &Value::new_string(ctx, &parsed.origin().ascii_serialization()))
            .unwrap();
        obj.set_property(
            "protocol",
            &Value::new_string(ctx, &format!("{}:", parsed.scheme())),
        )
        .unwrap();
        obj.set_property("hostname", &Value::new_string(ctx, parsed.host_str().unwrap_or("")))
            .unwrap();
        obj.set_property(
            "host",
            &Value::new_string(
                ctx,
                &match parsed.port() {
                    Some(p) => format!("{}:{}", parsed.host_str().unwrap_or(""), p),
                    None => parsed.host_str().unwrap_or("").to_string(),
                },
            ),
        )
        .unwrap();
        obj.set_property(
            "port",
            &Value::new_string(
                ctx,
                &parsed.port().map(|p| p.to_string()).unwrap_or_default(),
            ),
        )
        .unwrap();
        obj.set_property("pathname", &Value::new_string(ctx, parsed.path()))
            .unwrap();
        obj.set_property(
            "search",
            &Value::new_string(ctx, &parsed.query().map(|q| format!("?{q}")).unwrap_or_default()),
        )
        .unwrap();
        obj.set_property(
            "hash",
            &Value::new_string(ctx, &parsed.fragment().map(|h| format!("#{h}")).unwrap_or_default()),
        )
        .unwrap();
        obj.set_property("username", &Value::new_string(ctx, parsed.username()))
            .unwrap();
        obj.set_property(
            "password",
            &Value::new_string(ctx, parsed.password().unwrap_or("")),
        )
        .unwrap();
        Ok(obj_v)
    });
    ctx.global_object()
        .set_property("__bun_parse_url", &cb.value_in(ctx))
        .unwrap();
    std::mem::forget(cb);
}
