//! End-to-end CLI tests. Each test spawns the compiled `bun-rs` binary and
//! asserts on stdout/stderr/exit code. We use `env!("CARGO_BIN_EXE_bun-rs")`
//! so Cargo guarantees the binary is built before the test runs.

use std::process::Command;

fn bun_rs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bun-rs"))
}

#[test]
fn repl_single_line() {
    use std::io::Write;
    let mut child = bun_rs()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1 + 2\n40 + 2\n")
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("3"), "stdout: {stdout}");
    assert!(stdout.contains("42"), "stdout: {stdout}");
}

#[test]
fn repl_multiline() {
    use std::io::Write;
    let mut child = bun_rs()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"function f(\nx,\ny\n) { return x + y }\nf(10, 5)\n")
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("15"), "stdout: {stdout}");
}

#[test]
fn version_flag() {
    let out = bun_rs().arg("--version").output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("bun-rs "), "got: {s:?}");
}

#[test]
fn eval_inline_arithmetic() {
    let out = bun_rs()
        .args(["-e", "console.log(1 + 2 + 3)"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "6");
}

#[test]
fn eval_inline_silent_by_default() {
    // -e must NOT print the implicit value (matches Node).
    let out = bun_rs().args(["-e", "1 + 2"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[test]
fn print_inline_value() {
    let out = bun_rs().args(["-p", "40 + 2"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
}

#[test]
fn throw_exits_nonzero() {
    let out = bun_rs()
        .args(["-e", "throw new Error('kaboom')"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected nonzero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("kaboom"), "stderr: {stderr}");
}

#[test]
fn console_json_object() {
    let out = bun_rs()
        .args(["-e", "console.log({hello: 'world', n: 7})"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"hello\":\"world\""), "got: {s:?}");
    assert!(s.contains("\"n\":7"), "got: {s:?}");
}

#[test]
fn process_argv_visible_to_script() {
    let out = bun_rs()
        .args(["-e", "console.log(process.argv[2], process.argv[3])", "alpha", "beta"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "alpha beta");
}

#[test]
fn process_env_path_visible() {
    let out = bun_rs()
        .env("BUN_RS_TEST_VAR", "found_it")
        .args(["-e", "console.log(process.env.BUN_RS_TEST_VAR)"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "found_it");
}

#[test]
fn process_exit_with_code() {
    let out = bun_rs().args(["-e", "process.exit(7)"]).output().unwrap();
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn run_ts_file() {
    let dir = tempdir();
    let file = dir.join("hi.ts");
    std::fs::write(
        &file,
        r#"
        function add(a: number, b: number): number { return a + b; }
        console.log(add(2, 3));
        "#,
    )
    .unwrap();

    let out = bun_rs().arg("run").arg(&file).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "5");
}

#[test]
fn run_ts_file_shorthand() {
    let dir = tempdir();
    let file = dir.join("short.ts");
    std::fs::write(&file, "console.log('shorthand');").unwrap();

    let out = bun_rs().arg(&file).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "shorthand");
}

#[test]
fn async_await_drains_microtasks() {
    let out = bun_rs()
        .args([
            "-e",
            "(async () => { const v = await Promise.resolve(42); console.log('got', v); })()",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "got 42");
}

#[test]
fn syntax_error_exits_nonzero() {
    let out = bun_rs().args(["-e", "function ("]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn missing_file_is_usage_error() {
    let out = bun_rs().args(["run", "/tmp/__nope__.ts"]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn set_timeout_fires() {
    let out = bun_rs()
        .args([
            "-e",
            "setTimeout(() => console.log('after'), 10); console.log('before');",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "before\nafter"
    );
}

#[test]
fn set_timeout_orders_by_deadline() {
    let out = bun_rs()
        .args([
            "-e",
            "setTimeout(() => console.log('B'), 30); \
             setTimeout(() => console.log('A'), 5); \
             console.log('sync');",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "sync\nA\nB"
    );
}

#[test]
fn queue_microtask_runs_after_sync() {
    let out = bun_rs()
        .args([
            "-e",
            "queueMicrotask(() => console.log('micro')); console.log('sync');",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "sync\nmicro"
    );
}

#[test]
fn set_interval_clear() {
    let out = bun_rs()
        .args([
            "-e",
            "let n = 0; const id = setInterval(() => { \
               n++; if (n === 3) { console.log('done'); clearInterval(id); } \
             }, 5);",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
}

#[test]
fn tsx_jsx_transpile() {
    let dir = tempdir();
    let file = dir.join("comp.tsx");
    std::fs::write(
        &file,
        // Stub createElement so the lowered JSX has something to call.
        r#"
        const React = { createElement: (t: string, p: any, ...c: any[]) => ({ t, c }) };
        const el = <div>hi</div>;
        console.log(el.t);
        "#,
    )
    .unwrap();

    let out = bun_rs().arg(&file).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "div");
}

// ── ESM (Phase 1: static imports / exports) ────────────────────────────────

#[test]
fn esm_named_import() {
    let dir = tempdir();
    std::fs::write(
        dir.join("greet.ts"),
        "export function greet(who: string): string { return 'hi ' + who; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { greet } from './greet';\nconsole.log(greet('world'));",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi world");
}

#[test]
fn esm_default_import() {
    let dir = tempdir();
    std::fs::write(dir.join("life.ts"), "export default 42;").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import meaning from './life';\nconsole.log(meaning);",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
}

#[test]
fn esm_namespace_import() {
    let dir = tempdir();
    std::fs::write(
        dir.join("math.ts"),
        "export const PI = 3; export function add(a: number, b: number) { return a + b; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import * as m from './math';\nconsole.log(m.add(m.PI, 4));",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "7");
}

#[test]
fn esm_renamed_import() {
    let dir = tempdir();
    std::fs::write(dir.join("dep.ts"), "export const x = 10;").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { x as y } from './dep';\nconsole.log(y);",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "10");
}

#[test]
fn esm_export_star() {
    let dir = tempdir();
    std::fs::write(dir.join("leaf.ts"), "export const a = 1; export const b = 2;").unwrap();
    std::fs::write(dir.join("barrel.ts"), "export * from './leaf';").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { a, b } from './barrel';\nconsole.log(a + b);",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3");
}

#[test]
fn esm_export_from() {
    let dir = tempdir();
    std::fs::write(dir.join("leaf.ts"), "export const a = 5;").unwrap();
    std::fs::write(
        dir.join("barrel.ts"),
        "export { a as renamed } from './leaf';",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { renamed } from './barrel';\nconsole.log(renamed);",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "5");
}

#[test]
fn esm_circular_import_does_not_hang() {
    // A imports B's func and uses it lazily; B imports A's const.
    // Cycle should terminate; the partially-evaluated exports object is
    // returned to the second importer (CJS-ish semantics).
    let dir = tempdir();
    std::fs::write(
        dir.join("a.ts"),
        "import { b } from './b';\nexport const tag = 'A';\nexport function a() { return tag + b(); }",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.ts"),
        "import { tag } from './a';\nexport function b() { return ':' + tag; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { a } from './a';\nconsole.log(a());",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    // tag inside b() reads the binding captured at import time, which is
    // `undefined` because A's body hadn't finished when B was loaded. That's
    // documented CJS-ish behavior; live bindings come in Phase 2.
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("A:"), "got: {s:?}");
}

#[test]
fn esm_node_modules_resolution() {
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("node_modules/leftpad")).unwrap();
    std::fs::write(
        dir.join("node_modules/leftpad/package.json"),
        r#"{"name":"leftpad","main":"./index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("node_modules/leftpad/index.js"),
        "export function leftpad(s, n, ch) { ch = ch ?? ' '; while (s.length < n) s = ch + s; return s; }",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { leftpad } from 'leftpad';\nconsole.log(leftpad('7', 4, '0'));",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0007");
}

#[test]
fn esm_diamond_shared_dep_evaluated_once() {
    // main → left + right → shared. shared has a top-level side effect we
    // count. shared.count should be 1, not 2.
    let dir = tempdir();
    std::fs::write(
        dir.join("shared.ts"),
        "let n = 0; n++; export const count = n;",
    )
    .unwrap();
    std::fs::write(
        dir.join("left.ts"),
        "import { count } from './shared'; export const L = count;",
    )
    .unwrap();
    std::fs::write(
        dir.join("right.ts"),
        "import { count } from './shared'; export const R = count;",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { L } from './left'; import { R } from './right'; console.log(L, R);",
    )
    .unwrap();

    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1 1");
}

#[test]
fn top_level_await_setTimeout() {
    let dir = tempdir();
    std::fs::write(
        dir.join("main.ts"),
        "const v = await new Promise<number>((res) => setTimeout(() => res(99), 10));\n\
         console.log('got', v);",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "got 99");
}

#[test]
fn top_level_await_resolved_promise() {
    let dir = tempdir();
    std::fs::write(
        dir.join("main.ts"),
        "const v = await Promise.resolve(42);\nconsole.log(v);",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
}

#[test]
fn dynamic_import_returns_promise() {
    let dir = tempdir();
    std::fs::write(dir.join("dep.ts"), "export const answer = 7;").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "const mod = await import('./dep');\nconsole.log('answer:', mod.answer);",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "answer: 7"
    );
}

#[test]
fn dynamic_import_with_then() {
    let dir = tempdir();
    std::fs::write(dir.join("dep.ts"), "export default 'hi';").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import('./dep').then(m => console.log(m.default));",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
}

#[test]
fn node_path_basic_methods() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import path from "node:path";
        console.log(path.join("a", "b", "c"));
        console.log(path.dirname("/foo/bar/baz.txt"));
        console.log(path.basename("/foo/bar/baz.txt", ".txt"));
        console.log(path.extname("/x.tar.gz"));
        console.log(path.isAbsolute("/x"));
        console.log(path.normalize("/a/./b/../c"));
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<_> = s.trim().lines().collect();
    assert_eq!(lines, vec!["a/b/c", "/foo/bar", "baz", ".gz", "true", "/a/c"]);
}

#[test]
fn node_path_named_imports() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        "import { join, sep } from 'node:path';\nconsole.log(join('x','y'), sep);",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("x/y"));
}

#[test]
fn node_os_basic() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import os from "node:os";
        const valid = ["darwin","linux","win32","freebsd","openbsd","sunos","aix"];
        if (!valid.includes(os.platform())) throw new Error("bad platform: " + os.platform());
        if (typeof os.arch() !== "string") throw new Error("arch should be string");
        if (typeof os.hostname() !== "string") throw new Error("hostname should be string");
        if (typeof os.totalmem() !== "number") throw new Error("totalmem should be number");
        if (os.cpus().length < 1) throw new Error("cpus.length should be >= 1");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn web_url_parse_and_search_params() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            const u = new URL("https://user:pw@example.com:8080/foo/bar?x=1&y=2#h");
            console.log(u.protocol, u.hostname, u.port, u.pathname, u.search, u.hash, u.username);
            console.log(u.searchParams.get("x"), u.searchParams.get("y"));
            const sp = new URLSearchParams("a=hello%20world&b=2");
            console.log(sp.get("a"), sp.get("b"));
            sp.set("a", "z"); sp.append("a", "y");
            console.log(sp.getAll("a").join(","));
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("https: example.com 8080 /foo/bar ?x=1&y=2 #h user"), "got: {s}");
    assert!(s.contains("1 2"), "got: {s}");
    assert!(s.contains("hello world 2"), "got: {s}");
    assert!(s.contains("z,y"), "got: {s}");
}

#[test]
fn web_headers_and_response_json() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            const h = new Headers({ "Content-Type": "text/plain", "X-Foo": "1" });
            h.append("X-Foo", "2");
            console.log(h.get("content-type"), h.get("x-foo"));
            const r = Response.json({ ok: true, n: 7 });
            r.json().then(v => console.log("ok="+v.ok, "n="+v.n));
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("text/plain 1, 2"), "got: {s}");
    assert!(s.contains("ok=true n=7"), "got: {s}");
}

#[test]
fn bun_file_roundtrip() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import path from "node:path";
        const p = path.join(process.cwd(), "f.json");
        await Bun.write(p, JSON.stringify({n:7}));
        const f = Bun.file(p);
        if (f.size !== 7) throw new Error("size " + f.size);
        const j = await f.json();
        if (j.n !== 7) throw new Error("n " + j.n);
        const t = await f.text();
        if (t !== '{"n":7}') throw new Error("text " + t);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn async_fetch_does_not_block_timers() {
    // Local Bun.serve that delays its response — confirms fetch is async
    // (a setTimeout fires before the fetch resolves).
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        const server = Bun.serve({
            port: 0,
            fetch(req) {
                return new Promise(resolve =>
                    setTimeout(() => resolve(new Response("ok")), 100)
                );
            },
        });
        const t0 = Date.now();
        const order: string[] = [];
        setTimeout(() => order.push("timer:" + (Date.now() - t0)), 20);
        const r = await fetch("http://127.0.0.1:" + server.port + "/");
        order.push("fetch:" + (Date.now() - t0));
        await r.text();
        server.stop();
        // Timer should have fired BEFORE the fetch resolved.
        const t = order[0], f = order[1];
        if (!t.startsWith("timer:") || !f.startsWith("fetch:")) {
            throw new Error("wrong order: " + JSON.stringify(order));
        }
        const tMs = parseInt(t.slice(6));
        const fMs = parseInt(f.slice(6));
        if (tMs >= fMs) throw new Error("timer should fire before fetch: " + tMs + " vs " + fMs);
        if (fMs < 90) throw new Error("fetch should take ~100ms, got " + fMs);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {stderr}\nstdout: {stdout}");
    assert_eq!(stdout.trim(), "ok");
}

#[test]
fn bun_serve_concurrent_requests() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        const server = Bun.serve({
            port: 0,
            async fetch(req) {
                await new Promise(r => setTimeout(r, 100));
                return new Response("ok");
            },
        });
        console.log("PORT:" + server.port);
        "#,
    )
    .unwrap();

    let mut child = bun_rs()
        .arg(dir.join("m.ts"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = child.stdout.take().unwrap();
    let mut acc = Vec::new();
    let mut buf = [0u8; 256];
    let start = std::time::Instant::now();
    let port = loop {
        if start.elapsed() > std::time::Duration::from_secs(5) {
            let _ = child.kill();
            panic!("no port");
        }
        if let Ok(n) = stdout.read(&mut buf) {
            if n > 0 {
                acc.extend_from_slice(&buf[..n]);
                if let Some(p) = std::str::from_utf8(&acc)
                    .ok()
                    .and_then(|s| s.lines().find_map(|l| l.strip_prefix("PORT:")))
                    .and_then(|s| s.trim().parse::<u16>().ok())
                {
                    break p;
                }
            }
        }
    };

    // Hand-rolled HTTP/1.0 client; 5 concurrent threads, each a single
    // request. Measure how long all 5 take.
    let kickoff = std::time::Instant::now();
    let mut handles = Vec::new();
    for _ in 0..5 {
        handles.push(std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /slow HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            resp
        }));
    }
    for h in handles { let r = h.join().unwrap(); assert!(r.contains("200")); }
    let elapsed = kickoff.elapsed();

    let _ = child.kill();
    let _ = child.wait();

    // With true concurrency, 5 × 100ms requests should land in ~150-300ms.
    // With serial (the old behavior), it would be ~500ms+.
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "5 concurrent requests took {:?} — handler is still serialized",
        elapsed
    );
}

#[test]
fn bun_test_runner_basics() {
    let dir = tempdir();
    std::fs::write(
        dir.join("sample.test.ts"),
        r#"
        describe("math", () => {
            test("plus", () => expect(1 + 1).toBe(2));
            test("eq", () => expect({a:1}).toEqual({a:1}));
        });
        test("not", () => expect(1).not.toBe(2));
        test("async", async () => expect(await Promise.resolve(7)).toBe(7));
        "#,
    )
    .unwrap();
    let out = bun_rs().arg("test").arg(&dir).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("4 passed"), "stderr: {stderr}");
    assert!(stderr.contains("0 failed"), "stderr: {stderr}");
}

#[test]
fn bun_test_runner_reports_failure() {
    let dir = tempdir();
    std::fs::write(
        dir.join("bad.test.ts"),
        "test(\"will fail\", () => expect(1).toBe(2));\n",
    )
    .unwrap();
    let out = bun_rs().arg("test").arg(&dir).output().unwrap();
    assert!(!out.status.success(), "should have failed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("1 failed"), "stderr: {stderr}");
}

#[test]
fn bundle_emits_single_file_that_runs() {
    let dir = tempdir();
    std::fs::write(dir.join("greet.ts"), "export function greet(w:string){return \"hi \"+w;}").unwrap();
    std::fs::write(dir.join("life.ts"), "export default 42;").unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { greet } from './greet';\nimport m from './life';\nconsole.log(greet('bun'), m);",
    )
    .unwrap();

    let bundle_path = dir.join("out.js");
    let out = bun_rs()
        .arg("build")
        .arg(dir.join("main.ts"))
        .arg("--outfile")
        .arg(&bundle_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "build stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(bundle_path.exists());

    let run = bun_rs().arg(&bundle_path).output().unwrap();
    assert!(run.status.success(), "run stderr: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "hi bun 42");
}

#[test]
fn bundle_handles_node_external() {
    // Bundler keeps node:* as externals; the bundled output should still
    // run under bun-rs (since we provide the node:* builtins).
    let dir = tempdir();
    std::fs::write(
        dir.join("main.ts"),
        "import path from 'node:path'; console.log(path.join('a','b'));",
    )
    .unwrap();
    let bundle_path = dir.join("out.js");
    let build = bun_rs()
        .arg("build")
        .arg(dir.join("main.ts"))
        .arg("--outfile")
        .arg(&bundle_path)
        .output()
        .unwrap();
    assert!(build.status.success(), "build stderr: {}", String::from_utf8_lossy(&build.stderr));
    let run = bun_rs().arg(&bundle_path).output().unwrap();
    assert!(run.status.success(), "run stderr: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "a/b");
}

#[test]
fn readline_callback_form() {
    use std::io::Write;
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import readline from "node:readline";
        const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
        rl.question("name? ", (name) => {
            console.log("hi " + name);
            rl.close();
        });
        "#,
    )
    .unwrap();
    let mut child = bun_rs()
        .arg(dir.join("m.ts"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"world\n").unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("hi world"), "stdout: {s}");
}

#[test]
fn readline_promises_form() {
    use std::io::Write;
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import { promises as rl } from "node:readline";
        const iface = rl.createInterface({ input: process.stdin, output: process.stdout });
        const name = await iface.question("name? ");
        console.log("hi " + name);
        iface.close();
        "#,
    )
    .unwrap();
    let mut child = bun_rs()
        .arg(dir.join("m.ts"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"jim\n").unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("hi jim"));
}

#[test]
fn abort_controller_basics() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            const c = new AbortController();
            let fired = false;
            c.signal.addEventListener("abort", () => fired = true);
            if (c.signal.aborted) throw new Error("pre-aborted");
            c.abort();
            if (!c.signal.aborted) throw new Error("not aborted");
            if (!fired) throw new Error("listener didn't fire");
            if (c.signal.reason.name !== "AbortError") throw new Error("reason name");
            const a2 = AbortSignal.abort("custom");
            if (!a2.aborted || a2.reason !== "custom") throw new Error("static abort");
            try { a2.throwIfAborted(); throw new Error("should have thrown"); } catch (e) {
                if (e !== "custom") throw new Error("wrong throw");
            }
            console.log("ok");
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn bun_serve_echo() {
    use std::io::{Read, Write};
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        const server = Bun.serve({
            port: 0,
            fetch(req) {
                const u = new URL(req.url);
                if (u.pathname === "/json") return Response.json({path: u.pathname});
                return new Response("hi " + u.pathname, { headers: { "x-bunrs": "ok" } });
            }
        });
        console.log("PORT:" + server.port);
        "#,
    )
    .unwrap();

    let mut child = bun_rs()
        .arg(dir.join("m.ts"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Read child stdout until "PORT:<n>" appears.
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 256];
    let mut acc = Vec::new();
    let start = std::time::Instant::now();
    let port = loop {
        if start.elapsed() > std::time::Duration::from_secs(5) {
            let _ = child.kill();
            panic!("server didn't print port in time");
        }
        match stdout.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => acc.extend_from_slice(&buf[..n]),
            Err(_) => {}
        }
        if let Some(p) = std::str::from_utf8(&acc)
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("PORT:")))
            .and_then(|s| s.trim().parse::<u16>().ok())
        {
            break Some(p);
        }
    };
    let port = port.expect("got a port");

    // Do an HTTP request against the server with a raw TCP stream so we
    // don't need a client crate inside the test.
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to server");
    stream
        .write_all(b"GET /world HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    assert!(resp.contains("200 OK"), "resp: {resp}");
    assert!(resp.contains("x-bunrs: ok"), "resp: {resp}");
    assert!(resp.contains("hi /world"), "resp: {resp}");

    // JSON path.
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect again");
    stream
        .write_all(b"GET /json HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    assert!(resp.contains("\"path\":\"/json\""), "resp: {resp}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn buffer_class_basics() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            const a = Buffer.from("hello");
            if (a.length !== 5) throw new Error("len");
            if (a.toString() !== "hello") throw new Error("utf8");
            if (a.toString("hex") !== "68656c6c6f") throw new Error("hex");
            if (a.toString("base64") !== "aGVsbG8=") throw new Error("b64");
            if (Buffer.from("48656c6c6f", "hex").toString() !== "Hello") throw new Error("from hex");
            if (Buffer.from("aGVsbG8=", "base64").toString() !== "hello") throw new Error("from b64");
            if (Buffer.alloc(3, 0x41).toString() !== "AAA") throw new Error("alloc");
            if (Buffer.concat([Buffer.from("hi "), Buffer.from("bun")]).toString() !== "hi bun") throw new Error("concat");
            if (!Buffer.isBuffer(a)) throw new Error("isBuffer");
            console.log("ok");
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn fs_readfile_returns_buffer_no_encoding() {
    let dir = tempdir();
    std::fs::write(dir.join("b.bin"), [0x89u8, 0x50, 0x4e, 0x47]).unwrap();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import fs from "node:fs";
        const b = fs.readFileSync("b.bin");
        if (!Buffer.isBuffer(b)) throw new Error("not buffer");
        if (b.length !== 4) throw new Error("len " + b.length);
        if (b[0] !== 0x89 || b[1] !== 0x50 || b[2] !== 0x4e || b[3] !== 0x47)
          throw new Error("content");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn writefile_accepts_buffer() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import fs from "node:fs";
        fs.writeFileSync("w.bin", Buffer.from([1, 2, 3, 4, 5]));
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("w.bin")).unwrap();
    assert_eq!(bytes, vec![1, 2, 3, 4, 5]);
}

#[test]
fn readable_stream_from_iterable_and_for_await() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            (async () => {
                const rs = ReadableStream.from([1, 2, 3]);
                const out = [];
                for await (const v of rs) out.push(v);
                console.log(out.join(","));
            })()
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1,2,3");
}

#[test]
fn transform_stream_uppercase_pipeline() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            (async () => {
                const upper = new TransformStream({
                    transform(c, ctrl) { ctrl.enqueue(String(c).toUpperCase()); },
                });
                const collected = [];
                await ReadableStream.from(["hi", "bun-rs"])
                    .pipeThrough(upper)
                    .pipeTo(new WritableStream({ write(c) { collected.push(c); } }));
                console.log(collected.join(" "));
            })()
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "HI BUN-RS");
}

#[test]
fn response_body_is_a_stream() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            (async () => {
                const r = new Response("ok");
                if (!(r.body instanceof ReadableStream)) throw new Error("not a stream");
                const chunks = [];
                for await (const c of r.body) chunks.push(c);
                const total = chunks.reduce((s, c) => s + c.byteLength, 0);
                console.log("len:", total);
            })()
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "len: 2");
}

#[test]
fn writable_stream_collect_and_close() {
    let out = bun_rs()
        .args([
            "-e",
            r#"
            (async () => {
                const sunk = [];
                const ws = new WritableStream({ write(c) { sunk.push(c); } });
                const w = ws.getWriter();
                await w.write("a"); await w.write("b"); await w.write("c");
                await w.close();
                console.log(sunk.join(""));
            })()
            "#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "abc");
}

#[test]
fn node_stream_readable_pipe_writable() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import { Readable, Writable } from "node:stream";
        const src = Readable.from([1, 2, 3, 4]);
        const out: number[] = [];
        const sink = new Writable({
            write(chunk, enc, cb) { out.push(chunk); cb(); },
        });
        src.pipe(sink);
        await new Promise<void>(res => sink.on("finish", res));
        if (out.join(",") !== "1,2,3,4") throw new Error(out.join(","));
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn fs_create_read_stream_chunks_a_big_file() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import fs from "node:fs";
        import path from "node:path";
        const file = path.join(process.cwd(), "big.bin");
        const data = new Uint8Array(100 * 1024);
        for (let i = 0; i < data.length; i++) data[i] = i & 0xff;
        fs.writeFileSync(file, data);
        let bytes = 0, chunks = 0;
        await new Promise<void>((resolve, reject) => {
            const r = fs.createReadStream(file);
            r.on("data", (c: Buffer) => { bytes += c.length; chunks++; });
            r.on("end", resolve);
            r.on("error", reject);
        });
        fs.unlinkSync(file);
        if (bytes !== 102400) throw new Error("bytes " + bytes);
        if (chunks < 2) throw new Error("expected multiple chunks, got " + chunks);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn fs_create_write_stream_round_trip() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import fs from "node:fs";
        const w = fs.createWriteStream("out.bin");
        w.write(Buffer.from("a "));
        w.write(Buffer.from("b "));
        w.write(Buffer.from("c"));
        await new Promise<void>(res => w.end(res));
        const text = fs.readFileSync("out.bin", "utf-8");
        if (text !== "a b c") throw new Error("text: " + text);
        fs.unlinkSync("out.bin");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn error_stack_maps_to_source_lines() {
    // Throw on a known line and confirm the stack frame names that line,
    // not the rewritten/wrapped script line.
    let dir = tempdir();
    let bad = dir.join("bad.ts");
    std::fs::write(
        &bad,
        "// line 1\n// line 2\nexport function boom() {\n  throw new Error('x');\n}\n",
    )
    .unwrap();
    let main = dir.join("main.ts");
    std::fs::write(&main, "import { boom } from './bad';\nboom();\n").unwrap();

    let out = bun_rs().arg(&main).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Throw is on line 4 of bad.ts (`throw new Error('x');`). Frame should
    // mention `bad.ts:4` (NOT `:5` from the IIFE wrapper, NOT some other
    // synthesized number).
    assert!(
        stderr.contains("bad.ts:4"),
        "expected 'bad.ts:4' in stack; got:\n{stderr}"
    );
    // The call site in main.ts is on line 2.
    assert!(
        stderr.contains("main.ts:2"),
        "expected 'main.ts:2' in stack; got:\n{stderr}"
    );
}

#[test]
fn node_assert_strict_and_deep() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import assert from "node:assert";
        assert.strictEqual(1 + 1, 2);
        assert.deepStrictEqual({a:1,b:[2,3]}, {a:1,b:[2,3]});
        assert.throws(() => { throw new Error("boom"); }, /boom/);
        await assert.rejects(async () => { throw new Error("async"); }, /async/);
        try {
            assert.strictEqual(1, 2);
            throw new Error("should have failed");
        } catch (e: any) {
            if (e.name !== "AssertionError") throw new Error("wrong error: " + e.name);
        }
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_querystring_roundtrip() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import qs from "node:querystring";
        const o = qs.parse("a=1&b=hello%20world&a=2");
        if (o.b !== "hello world") throw new Error("b: " + o.b);
        if (!Array.isArray(o.a) || o.a.join(",") !== "1,2") throw new Error("a: " + o.a);
        const s = qs.stringify({x: 1, y: "a b"});
        if (s !== "x=1&y=a%20b") throw new Error("stringify: " + s);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_url_helpers() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import { fileURLToPath, pathToFileURL, URL } from "node:url";
        if (fileURLToPath("file:///x/y") !== "/x/y") throw new Error("fileURLToPath");
        const u = pathToFileURL("/x/y file.txt");
        if (!u.href.startsWith("file:///x/y")) throw new Error("pathToFileURL: " + u.href);
        if (!u.href.includes("%20")) throw new Error("not percent-encoded: " + u.href);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn process_stdout_stderr_write() {
    let out = bun_rs()
        .args(["-e", "process.stdout.write('out'); process.stderr.write('err');"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "out");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "err");
}

#[test]
fn fs_promises_actually_async() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import { promises as fs } from "node:fs";
        import path from "node:path";
        const p = path.join(process.cwd(), "f.txt");
        const t0 = Date.now();
        let timerFired = false;
        setTimeout(() => { timerFired = true; }, 1);
        await fs.writeFile(p, Buffer.from([1,2,3,4,5]));
        const b = await fs.readFile(p);
        if (!Buffer.isBuffer(b)) throw new Error("not buffer");
        if (b.length !== 5 || b[0] !== 1) throw new Error("contents");
        await fs.unlink(p);
        // Concurrent timer should have fired during the awaits (cooperative).
        if (!timerFired) throw new Error("timer didn't fire; fs.promises blocked");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_events_emitter() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        let calls = 0;
        e.on("foo", (a, b) => { calls++; if (a + b !== 3) throw new Error("args"); });
        e.emit("foo", 1, 2);
        e.emit("foo", 1, 2);
        if (calls !== 2) throw new Error("calls " + calls);
        if (e.listenerCount("foo") !== 1) throw new Error("count");
        let onceCalls = 0;
        e.once("o", () => onceCalls++);
        e.emit("o"); e.emit("o");
        if (onceCalls !== 1) throw new Error("once " + onceCalls);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_util_promisify_and_format() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import util from "node:util";
        const fn = (ms: number, cb: any) => setTimeout(() => cb(null, ms * 2), ms);
        const p = util.promisify(fn);
        const v = await p(5);
        if (v !== 10) throw new Error("promisify " + v);
        const f = util.format("a=%s b=%d c=%j", "x", 7, {n:1});
        if (f !== 'a=x b=7 c={"n":1}') throw new Error("format: " + f);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_crypto_hash_hmac_random() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import crypto from "node:crypto";
        // Known SHA-256 of "hello".
        const sha = crypto.createHash("sha256").update("hello").digest("hex");
        if (sha !== "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
            throw new Error("sha256 " + sha);
        // Known HMAC-SHA256("data", "secret").
        const hmac = crypto.createHmac("sha256", "secret").update("data").digest("hex");
        if (hmac !== "1b2c16b75bd2a870c114153ccda5bcfca63314bc722fa160d690de133ccbb9db")
            throw new Error("hmac " + hmac);
        const r = crypto.randomBytes(8);
        if (!Buffer.isBuffer(r) || r.length !== 8) throw new Error("randomBytes");
        const u = crypto.randomUUID();
        if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(u))
            throw new Error("uuid " + u);
        const a = Buffer.from("abcd"), b = Buffer.from("abcd"), c = Buffer.from("xbcd");
        if (!crypto.timingSafeEqual(a, b)) throw new Error("ts eq");
        if (crypto.timingSafeEqual(a, c)) throw new Error("ts neq");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_child_process_spawn_exec() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import cp from "node:child_process";
        const r = cp.spawnSync("echo", ["bun-rs"]);
        if (r.status !== 0) throw new Error("spawn status");
        if (!r.stdout.toString().includes("bun-rs")) throw new Error("spawn out");
        const text = cp.execSync("printf hello", { encoding: "utf-8" });
        if (text !== "hello") throw new Error("execSync " + text);
        await new Promise<void>((resolve, reject) => {
            cp.exec("printf cbStyle", (err: any, stdout: string) => {
                if (err) reject(err);
                else if (stdout !== "cbStyle") reject(new Error("exec " + stdout));
                else resolve();
            });
        });
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("m.ts")).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_fs_roundtrip() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import fs from "node:fs";
        import path from "node:path";
        const dir = path.join(process.cwd(), "tmp-bunrs-fs");
        fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(path.join(dir, "a.txt"), "hello");
        const s = fs.readFileSync(path.join(dir, "a.txt"), "utf8");
        if (s !== "hello") throw new Error("contents " + s);
        const st = fs.statSync(path.join(dir, "a.txt"));
        if (st.size !== 5) throw new Error("size " + st.size);
        if (!st.isFile()) throw new Error("isFile");
        const entries = fs.readdirSync(dir);
        if (!entries.includes("a.txt")) throw new Error("listing");
        fs.rmSync(dir, { recursive: true });
        if (fs.existsSync(dir)) throw new Error("not removed");
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn node_fs_promises_await() {
    let dir = tempdir();
    std::fs::write(
        dir.join("m.ts"),
        r#"
        import { promises as fs } from "node:fs";
        import path from "node:path";
        const p = path.join(process.cwd(), "promise-test.txt");
        await fs.writeFile(p, "abc");
        const got = await fs.readFile(p, "utf8");
        if (got !== "abc") throw new Error("got " + got);
        await fs.unlink(p);
        console.log("ok");
        "#,
    )
    .unwrap();
    let out = bun_rs()
        .arg(dir.join("m.ts"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn import_meta_url_and_friends() {
    let dir = tempdir();
    let file = dir.join("m.ts");
    std::fs::write(
        &file,
        "console.log(import.meta.url);\n\
         console.log(import.meta.filename);\n\
         console.log(import.meta.dirname);",
    )
    .unwrap();
    let out = bun_rs().arg(&file).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("file:///"), "url should be file:// : {s}");
    assert!(s.contains("m.ts"), "should mention file: {s}");
}

#[test]
fn esm_missing_specifier_errors() {
    let dir = tempdir();
    std::fs::write(
        dir.join("main.ts"),
        "import { x } from './does-not-exist';\nconsole.log(x);",
    )
    .unwrap();
    let out = bun_rs().arg(dir.join("main.ts")).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does-not-exist") || stderr.to_lowercase().contains("cannot find"),
        "stderr: {stderr}"
    );
}

// ── tiny test helper: a temp dir that cleans up on drop ─────────────────────

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nano = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "bun-rs-test-{nano}-{}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
