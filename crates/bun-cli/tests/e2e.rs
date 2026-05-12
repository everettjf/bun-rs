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
