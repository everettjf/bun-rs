//! `harness` — compatibility shim for Bun's official test suite.
//!
//! Bun's tests import platform / environment helpers from a relative
//! `harness.ts` file via the bare specifier "harness". To run those tests
//! against bun-rs, we provide an in-runtime stub that exposes the most
//! common symbols. Real bun-rs apps don't see this; it's only loaded on
//! `import "harness"` (which production code shouldn't do).

use bun_jsc::{Context, Value};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    let bun_rs = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "bun-rs".into());
    let exe_escaped = bun_rs.replace('\\', "\\\\").replace('"', "\\\"");
    let tmp = std::env::temp_dir().to_string_lossy().into_owned();
    let tmp_escaped = tmp.replace('\\', "\\\\").replace('"', "\\\"");

    let plat = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };

    let src = format!(
        r#"({{
            // Platform flags
            isWindows: {is_win},
            isMacOS: {is_mac},
            isLinux: {is_linux},
            isFreeBSD: {is_freebsd},
            isPosix: {is_posix},
            isArm64: {is_arm64},
            isX64: {is_x64},
            isCI: !!process.env.CI,
            isDebug: false,
            isASAN: false,
            isBroken: false,
            isGlibcVersionAtLeast: () => false,
            isIPv6: () => false,
            ospath: (p) => String(p).split("\\").join("/"),

            // Runtime info
            bunExe: () => {exe:?},
            nodeExe: () => "node",
            bunEnv: Object.fromEntries(Object.entries(process.env)),
            shellExe: () => "/bin/sh",
            invalidTls: {{}},

            // Temp dirs
            tempDir: (label, files) => {{
                const fs = require("node:fs"); const path = require("node:path");
                void fs; void path;
                const base = "{tmp}" + "/bunrs-harness-" + (label || "x") + "-" + Date.now() + "-" + Math.random().toString(36).slice(2);
                require("node:fs").mkdirSync(base, {{ recursive: true }});
                if (files) for (const [name, content] of Object.entries(files)) {{
                    const p = require("node:path").join(base, name);
                    require("node:fs").mkdirSync(require("node:path").dirname(p), {{ recursive: true }});
                    require("node:fs").writeFileSync(p, content);
                }}
                const dispose = () => {{ try {{ require("node:fs").rmSync(base, {{ recursive: true, force: true }}); }} catch {{}} }};
                const obj = {{ path: base, toString: () => base, valueOf: () => base }};
                Object.defineProperty(obj, Symbol.dispose, {{ value: dispose, configurable: true }});
                Object.defineProperty(obj, Symbol.asyncDispose, {{ value: async () => dispose(), configurable: true }});
                return obj;
            }},
            tmpdirSync: (label) => {{
                const dir = "{tmp}" + "/bunrs-tmp-" + (label || "x") + "-" + Date.now() + "-" + Math.random().toString(36).slice(2);
                require("node:fs").mkdirSync(dir, {{ recursive: true }});
                return dir;
            }},
            tempDirWithFiles: (label, files) => {{
                const dir = "{tmp}" + "/bunrs-tmp-" + label + "-" + Date.now() + "-" + Math.random().toString(36).slice(2);
                require("node:fs").mkdirSync(dir, {{ recursive: true }});
                for (const [name, content] of Object.entries(files)) {{
                    const p = require("node:path").join(dir, name);
                    require("node:fs").mkdirSync(require("node:path").dirname(p), {{ recursive: true }});
                    require("node:fs").writeFileSync(p, typeof content === "string" ? content : JSON.stringify(content));
                }}
                return dir;
            }},
            joinP: (...parts) => parts.join("/"),

            // Misc
            gc: () => {{}},
            gcTick: async () => {{ await new Promise(r => setTimeout(r, 0)); }},
            dumpStats: () => {{}},
            withoutAggressiveGC: (fn) => fn(),
            withAggressiveGC: (fn) => fn(),
            getMaxFD: () => 65536,
            getMaxNumberOfFileDescriptors: () => 65536,
            mkfifo: () => {{ throw new Error("mkfifo not supported"); }},
            getSocketPath: () => "/tmp/bunrs-sock-" + Date.now(),
            isIntelMacOS: false,
            isMusl: () => false,
            isPosixOS: () => true,
            shellExe2: () => "/bin/sh",
            getSecret: () => undefined,
            cwd: () => process.cwd(),
            nodeExePath: () => "/usr/bin/env node",
            packageDirectoryRecursive: (_p) => null,
            Bun: globalThis.Bun,
            randomPort: () => 30000 + Math.floor(Math.random() * 30000),
            expectMaxObjectTypeCount: () => {{}},
            normalizeBunSnapshot: (s) => String(s),
            exampleSite: "https://example.com",
            disableAggressiveGCScope: () => {{ return {{ [Symbol.dispose]() {{}} }}; }},
            describeOSVersion: () => "{plat}",
            isExecutable: (_p) => {{ try {{ require("node:fs").accessSync(_p, 1); return true; }} catch {{ return false; }} }},
            bunRun: () => {{ throw new Error("bunRun not implemented in bun-rs harness shim"); }},
            bunRunAsScript: () => {{ throw new Error("bunRunAsScript not implemented"); }},
            runBunInstall: () => {{ throw new Error("runBunInstall not implemented"); }},
        }})"#,
        exe = exe_escaped,
        tmp = tmp_escaped,
        is_win = if plat == "win32" { "true" } else { "false" },
        is_mac = if plat == "darwin" { "true" } else { "false" },
        is_linux = if plat == "linux" { "true" } else { "false" },
        is_freebsd = if plat == "freebsd" { "true" } else { "false" },
        is_posix = if plat != "win32" { "true" } else { "false" },
        is_arm64 = if arch == "arm64" { "true" } else { "false" },
        is_x64 = if arch == "x64" { "true" } else { "false" },
    );

    let v = ctx.eval(&src, Some("[harness]")).expect("build harness");
    // Tests do `import { ... } from "harness"` — destructuring from the
    // namespace, no default needed. But provide default just in case.
    let _ = v.to_object().and_then(|o| {
        o.set_property("default", &v)?;
        Ok(())
    });
    v
}
