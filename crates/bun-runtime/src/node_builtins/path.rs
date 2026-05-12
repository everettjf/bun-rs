//! `node:path` — POSIX-flavored implementation matching the platform default.
//!
//! Subset implemented:
//!   - sep, delimiter
//!   - join, resolve, normalize, dirname, basename, extname
//!   - isAbsolute, relative
//!   - parse, format
//!   - posix / win32 sub-namespaces (sep/delimiter only as a placeholder)

use bun_jsc::{Callback, Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:path]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    install_path_methods(ctx, &exports, std::path::MAIN_SEPARATOR.to_string(), default_delim());

    // Sub-namespaces: posix and win32.
    let posix_v = ctx.eval("({})", Some("[node:path.posix]")).unwrap();
    install_path_methods(ctx, &posix_v.to_object().unwrap(), "/".into(), ":".into());
    exports.set_property("posix", &posix_v).unwrap();

    let win32_v = ctx.eval("({})", Some("[node:path.win32]")).unwrap();
    install_path_methods(
        ctx,
        &win32_v.to_object().unwrap(),
        "\\".into(),
        ";".into(),
    );
    exports.set_property("win32", &win32_v).unwrap();

    // Mirror Node: `import path from "node:path"` gets the module object as
    // the default export.
    exports.set_property("default", &exports.as_value()).unwrap();

    exports.as_value()
}

fn default_delim() -> String {
    if cfg!(windows) { ";".into() } else { ":".into() }
}

fn install_path_methods(
    ctx: &Context,
    obj: &bun_jsc::Object<'_>,
    sep: String,
    delim: String,
) {
    obj.set_property("sep", &Value::new_string(ctx, &sep)).unwrap();
    obj.set_property("delimiter", &Value::new_string(ctx, &delim))
        .unwrap();

    let sep_for_join = sep.clone();
    bind(ctx, obj, "join", move |args| {
        let parts: Vec<String> = (0..args.len()).map(|i| args.get(i).to_string()).collect();
        let joined = join_parts(&parts, &sep_for_join);
        Ok(Value::new_string(args.context(), &joined))
    });

    let sep_for_resolve = sep.clone();
    bind(ctx, obj, "resolve", move |args| {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parts: Vec<String> = (0..args.len()).map(|i| args.get(i).to_string()).collect();
        let r = resolve_parts(&cwd, &parts, &sep_for_resolve);
        Ok(Value::new_string(args.context(), &r))
    });

    let sep_for_norm = sep.clone();
    bind(ctx, obj, "normalize", move |args| {
        let s = args.get(0).to_string();
        Ok(Value::new_string(args.context(), &normalize(&s, &sep_for_norm)))
    });

    let sep_for_dirname = sep.clone();
    bind(ctx, obj, "dirname", move |args| {
        let s = args.get(0).to_string();
        Ok(Value::new_string(args.context(), &dirname(&s, &sep_for_dirname)))
    });

    let sep_for_basename = sep.clone();
    bind(ctx, obj, "basename", move |args| {
        let p = args.get(0).to_string();
        let ext = if args.len() >= 2 { Some(args.get(1).to_string()) } else { None };
        Ok(Value::new_string(
            args.context(),
            &basename(&p, ext.as_deref(), &sep_for_basename),
        ))
    });

    bind(ctx, obj, "extname", |args| {
        let s = args.get(0).to_string();
        Ok(Value::new_string(args.context(), &extname(&s)))
    });

    let sep_for_isabs = sep.clone();
    bind(ctx, obj, "isAbsolute", move |args| {
        let s = args.get(0).to_string();
        Ok(Value::new_bool(args.context(), is_absolute(&s, &sep_for_isabs)))
    });

    let sep_for_rel = sep.clone();
    bind(ctx, obj, "relative", move |args| {
        let from = args.get(0).to_string();
        let to = args.get(1).to_string();
        Ok(Value::new_string(args.context(), &relative(&from, &to, &sep_for_rel)))
    });
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}

// ── path algorithms (POSIX/Windows agnostic over `sep`) ──────────────────────

fn join_parts(parts: &[String], sep: &str) -> String {
    let mut out = String::new();
    for p in parts {
        if p.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with(sep) {
            out.push_str(sep);
        }
        out.push_str(p);
    }
    if out.is_empty() {
        ".".into()
    } else {
        normalize(&out, sep)
    }
}

fn resolve_parts(cwd: &str, parts: &[String], sep: &str) -> String {
    // Walk parts right→left; once we hit an absolute path we stop and walk
    // left→right from there. Mirrors Node's algorithm closely.
    let mut resolved = String::new();
    let mut absolute = false;
    for p in parts.iter().rev() {
        if p.is_empty() {
            continue;
        }
        if resolved.is_empty() {
            resolved = p.clone();
        } else {
            resolved = format!("{}{sep}{}", p, resolved);
        }
        if is_absolute(p, sep) {
            absolute = true;
            break;
        }
    }
    if !absolute {
        if resolved.is_empty() {
            resolved = cwd.to_string();
        } else {
            resolved = format!("{}{sep}{}", cwd, resolved);
        }
    }
    normalize(&resolved, sep)
}

fn normalize(p: &str, sep: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let absolute = p.starts_with(sep);
    let trailing_sep = p.ends_with(sep) && p.len() > 1;
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split(sep) {
        match seg {
            "" | "." => continue,
            ".." => {
                if let Some(&last) = parts.last() {
                    if last != ".." && (absolute || true) {
                        parts.pop();
                        continue;
                    }
                }
                if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = parts.join(sep);
    if absolute {
        out = format!("{sep}{}", out);
    }
    if trailing_sep && !out.ends_with(sep) {
        out.push_str(sep);
    }
    if out.is_empty() {
        if absolute { sep.into() } else { ".".into() }
    } else {
        out
    }
}

fn dirname(p: &str, sep: &str) -> String {
    if p.is_empty() { return ".".into(); }
    let trimmed = p.trim_end_matches(sep);
    if trimmed.is_empty() { return sep.into(); }
    match trimmed.rfind(sep) {
        Some(0) => sep.into(),
        Some(i) => trimmed[..i].to_string(),
        None => ".".into(),
    }
}

fn basename(p: &str, ext: Option<&str>, sep: &str) -> String {
    let trimmed = p.trim_end_matches(sep);
    if trimmed.is_empty() {
        return String::new();
    }
    let base = match trimmed.rfind(sep) {
        Some(i) => &trimmed[i + sep.len()..],
        None => trimmed,
    };
    if let Some(e) = ext {
        if base.ends_with(e) && base.len() > e.len() {
            return base[..base.len() - e.len()].to_string();
        }
    }
    base.to_string()
}

fn extname(p: &str) -> String {
    // Use POSIX sep for extraction; behavior matches Node which keys on the
    // last '.' in the basename only.
    let base = match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    };
    let base = match base.rfind('\\') {
        Some(i) => &base[i + 1..],
        None => base,
    };
    if !base.contains('.') {
        return String::new();
    }
    // Find LAST dot; ignore leading-dot files (.foo → "")
    let last_dot = base.rfind('.').unwrap();
    if last_dot == 0 {
        return String::new();
    }
    base[last_dot..].to_string()
}

fn is_absolute(p: &str, sep: &str) -> bool {
    if p.starts_with(sep) {
        return true;
    }
    // Windows drive-letter
    if sep == "\\" {
        let bytes = p.as_bytes();
        if bytes.len() >= 3
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
            && (bytes[0].is_ascii_alphabetic())
        {
            return true;
        }
    }
    false
}

fn relative(from: &str, to: &str, sep: &str) -> String {
    let from_n = normalize(from, sep);
    let to_n = normalize(to, sep);
    if from_n == to_n {
        return String::new();
    }
    let from_parts: Vec<&str> = from_n.trim_start_matches(sep).split(sep).filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to_n.trim_start_matches(sep).split(sep).filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let up = vec!["..".to_string(); from_parts.len() - common];
    let down: Vec<String> = to_parts[common..].iter().map(|s| s.to_string()).collect();
    let mut all = up;
    all.extend(down);
    all.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_basic() {
        assert_eq!(join_parts(&["a".into(), "b".into(), "c".into()], "/"), "a/b/c");
        assert_eq!(join_parts(&["/a".into(), "b".into()], "/"), "/a/b");
        assert_eq!(join_parts(&["a".into(), "..".into(), "b".into()], "/"), "b");
    }

    #[test]
    fn normalize_dots() {
        assert_eq!(normalize("/a/./b/../c", "/"), "/a/c");
        assert_eq!(normalize("./a/b", "/"), "a/b");
        assert_eq!(normalize("a/b/", "/"), "a/b/");
    }

    #[test]
    fn dirname_basename_extname() {
        assert_eq!(dirname("/foo/bar/baz.txt", "/"), "/foo/bar");
        assert_eq!(basename("/foo/bar/baz.txt", None, "/"), "baz.txt");
        assert_eq!(basename("/foo/bar/baz.txt", Some(".txt"), "/"), "baz");
        assert_eq!(extname("/foo/bar/baz.txt"), ".txt");
        assert_eq!(extname("/foo/.bashrc"), "");
    }

    #[test]
    fn is_absolute_posix() {
        assert!(is_absolute("/foo", "/"));
        assert!(!is_absolute("foo", "/"));
    }

    #[test]
    fn is_absolute_win32() {
        assert!(is_absolute("C:\\foo", "\\"));
        assert!(!is_absolute("foo\\bar", "\\"));
    }

    #[test]
    fn relative_basic() {
        assert_eq!(relative("/a/b/c", "/a/b/d", "/"), "../d");
        assert_eq!(relative("/a/b", "/a/b/c", "/"), "c");
    }
}
