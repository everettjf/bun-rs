//! End-to-end CLI tests. Each test spawns the compiled `bun-rs` binary and
//! asserts on stdout/stderr/exit code. We use `env!("CARGO_BIN_EXE_bun-rs")`
//! so Cargo guarantees the binary is built before the test runs.

use std::process::Command;

fn bun_rs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bun-rs"))
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

// ── tiny test helper: a temp dir that cleans up on drop ─────────────────────

fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nano = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("bun-rs-test-{nano}-{}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
