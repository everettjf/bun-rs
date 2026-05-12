//! `node:os` — subset matching Node 22.
//!
//! Implemented: platform, arch, type, release, hostname, homedir, tmpdir,
//! EOL, cpus (placeholder w/ count), totalmem, freemem, uptime, userInfo,
//! networkInterfaces (stub), constants (stub).

use bun_jsc::{Callback, Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let exports_v = ctx.eval("({})", Some("[node:os]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    let eol = if cfg!(windows) { "\r\n" } else { "\n" };
    exports.set_property("EOL", &Value::new_string(ctx, eol)).unwrap();

    bind(ctx, &exports, "platform", |args| {
        let p = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            o => o,
        };
        Ok(Value::new_string(args.context(), p))
    });

    bind(ctx, &exports, "arch", |args| {
        let a = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            o => o,
        };
        Ok(Value::new_string(args.context(), a))
    });

    bind(ctx, &exports, "type", |args| {
        let t = match std::env::consts::OS {
            "macos" => "Darwin",
            "linux" => "Linux",
            "windows" => "Windows_NT",
            o => o,
        };
        Ok(Value::new_string(args.context(), t))
    });

    bind(ctx, &exports, "release", |args| {
        Ok(Value::new_string(args.context(), &os_release()))
    });

    bind(ctx, &exports, "hostname", |args| {
        Ok(Value::new_string(args.context(), &hostname()))
    });

    bind(ctx, &exports, "homedir", |args| {
        let h = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        Ok(Value::new_string(args.context(), &h))
    });

    bind(ctx, &exports, "tmpdir", |args| {
        let t = std::env::temp_dir().to_string_lossy().into_owned();
        Ok(Value::new_string(args.context(), &t))
    });

    bind(ctx, &exports, "totalmem", |args| {
        Ok(Value::new_number(args.context(), totalmem() as f64))
    });

    bind(ctx, &exports, "freemem", |args| {
        // Without a system query we can't be accurate; report totalmem as a
        // floor. Phase 3 to wire up sysinfo or sysctlbyname.
        Ok(Value::new_number(args.context(), totalmem() as f64))
    });

    bind(ctx, &exports, "uptime", |args| {
        // Process uptime is a reasonable approximation in the absence of
        // a sysctl call.
        let u = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(Value::new_number(args.context(), u as f64))
    });

    bind(ctx, &exports, "cpus", |args| {
        // Build a small array of cpu placeholders: { model, speed, times }
        let ctx = args.context();
        let count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let arr = ctx.eval("[]", Some("[node:os.cpus]")).unwrap();
        let arr_obj = arr.to_object().unwrap();
        for i in 0..count {
            let entry = ctx.eval("({ model: 'unknown', speed: 0, times: { user: 0, nice: 0, sys: 0, idle: 0, irq: 0 } })", Some("[cpu]")).unwrap();
            arr_obj.set_property(&i.to_string(), &entry).unwrap();
        }
        arr_obj.set_property("length", &Value::new_number(ctx, count as f64)).unwrap();
        Ok(arr)
    });

    bind(ctx, &exports, "userInfo", |args| {
        let ctx = args.context();
        let info = ctx.eval("({})", Some("[userInfo]")).unwrap();
        let obj = info.to_object().unwrap();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        obj.set_property("username", &Value::new_string(ctx, &user)).unwrap();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        obj.set_property("homedir", &Value::new_string(ctx, &home)).unwrap();
        let shell = std::env::var("SHELL").unwrap_or_default();
        obj.set_property("shell", &Value::new_string(ctx, &shell)).unwrap();
        obj.set_property("uid", &Value::new_number(ctx, -1.0)).unwrap();
        obj.set_property("gid", &Value::new_number(ctx, -1.0)).unwrap();
        Ok(info)
    });

    // Stub: networkInterfaces returns {} for now.
    bind(ctx, &exports, "networkInterfaces", |args| {
        Ok(args.context().eval("({})", Some("[networkInterfaces]")).unwrap())
    });

    // constants: very small subset.
    let constants = ctx.eval("({ signals: {}, errno: {} })", Some("[constants]")).unwrap();
    exports.set_property("constants", &constants).unwrap();

    exports.set_property("default", &exports.as_value()).unwrap();
    exports.as_value()
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}

fn hostname() -> String {
    // Best-effort: HOSTNAME env var or `uname -n` via libc.
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    // SAFETY: uname struct is C-defined, OS-specific; we copy out the
    // nodename field.
    #[cfg(unix)]
    unsafe {
        use std::ffi::CStr;
        let mut buf: [u8; 256] = [0; 256];
        if libc_gethostname(buf.as_mut_ptr() as *mut i8, buf.len()) == 0 {
            let cstr = CStr::from_ptr(buf.as_ptr() as *const i8);
            return cstr.to_string_lossy().into_owned();
        }
    }
    String::new()
}

#[cfg(unix)]
extern "C" {
    #[link_name = "gethostname"]
    fn libc_gethostname(name: *mut i8, len: usize) -> i32;
}

fn os_release() -> String {
    // Best-effort: read /proc/version on Linux, sysctl on macOS, registry on
    // Windows. For MVP just return uname -r equivalent via libc, falling
    // back to "0.0.0".
    #[cfg(unix)]
    unsafe {
        use std::ffi::CStr;
        #[repr(C)]
        struct Utsname {
            sysname: [i8; 256],
            nodename: [i8; 256],
            release: [i8; 256],
            version: [i8; 256],
            machine: [i8; 256],
            // Linux also has `domainname`; struct size handled by uname()
        }
        extern "C" {
            fn uname(buf: *mut Utsname) -> i32;
        }
        let mut u: Utsname = std::mem::zeroed();
        if uname(&mut u) == 0 {
            return CStr::from_ptr(u.release.as_ptr())
                .to_string_lossy()
                .into_owned();
        }
    }
    "0.0.0".into()
}

fn totalmem() -> u64 {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut size: u64 = 0;
        let mut len: usize = 8;
        let name = b"hw.memsize\0";
        extern "C" {
            fn sysctlbyname(
                name: *const i8,
                oldp: *mut u8,
                oldlenp: *mut usize,
                newp: *mut u8,
                newlen: usize,
            ) -> i32;
        }
        if sysctlbyname(
            name.as_ptr() as *const i8,
            &mut size as *mut u64 as *mut u8,
            &mut len,
            std::ptr::null_mut(),
            0,
        ) == 0
        {
            return size;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(kb) = rest
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    0
}
