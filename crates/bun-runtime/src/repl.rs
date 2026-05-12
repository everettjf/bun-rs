//! Minimal REPL. Reads lines from stdin, evaluates each in the same context,
//! prints the resulting value (Node-style: skip `undefined`).
//!
//! Multi-line support: if `JSCheckScriptSyntax` reports a syntax error that
//! looks like "unexpected end of input", we keep reading. Otherwise the error
//! is shown immediately. Ctrl-D (EOF) exits with code 0.
//!
//! No history/edit features — those would need a curses/readline crate. For
//! now this is good enough to debug snippets interactively.

use std::io::{self, BufRead, Write};

use bun_jsc::Context;
use bun_jsc_sys as sys;

use crate::RuntimeError;

pub fn run(ctx: &Context) -> Result<(), RuntimeError> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    eprintln!(
        "bun-rs REPL (v{} on JavaScriptCore.framework). Ctrl-D to exit.",
        env!("CARGO_PKG_VERSION")
    );

    let mut buffer = String::new();
    let mut continuation = false;

    loop {
        let prompt = if continuation { "... " } else { "> " };
        write!(stderr, "{prompt}").ok();
        stderr.flush().ok();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                // EOF
                writeln!(stderr).ok();
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("stdin error: {e}");
                return Err(RuntimeError::Usage);
            }
        }

        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(line.trim_end_matches('\n'));

        // Check syntax. If the error suggests incomplete input, prompt again.
        if !is_complete(ctx, &buffer) {
            continuation = true;
            continue;
        }

        match ctx.eval(&buffer, Some("[repl]")) {
            Ok(v) => {
                if !v.is_undefined() {
                    writeln!(stdout, "{}", v.to_string()).ok();
                }
            }
            Err(exc) => {
                let msg = exc.message();
                writeln!(stderr, "Uncaught {msg}").ok();
                if let Some(stack) = exc.stack() {
                    if !stack.is_empty() {
                        writeln!(stderr, "{stack}").ok();
                    }
                }
            }
        }

        buffer.clear();
        continuation = false;
    }
}

/// Returns false when the source parses as "unexpected end of input" — i.e.
/// the user is still typing. Anything else (valid, or a different error)
/// counts as complete enough to attempt evaluation.
fn is_complete(ctx: &Context, source: &str) -> bool {
    use std::ptr;
    let script = bun_jsc::JsString::adopt(unsafe {
        sys::JSStringCreateWithCharacters(
            source.encode_utf16().collect::<Vec<u16>>().as_ptr(),
            source.encode_utf16().count(),
        )
    });
    let mut exc: sys::JSValueRef = ptr::null();
    let ok = unsafe {
        sys::JSCheckScriptSyntax(ctx.as_raw(), script.as_raw(), ptr::null_mut(), 1, &mut exc)
    };
    if ok {
        return true;
    }
    if exc.is_null() {
        return true;
    }
    let s = unsafe { sys::JSValueToStringCopy(ctx.as_raw(), exc, ptr::null_mut()) };
    if s.is_null() {
        return true;
    }
    let msg = bun_jsc::JsString::adopt(s).to_string().to_lowercase();
    // Common JSC phrasings when the user has unbalanced braces/parens/quotes.
    !(msg.contains("unexpected end of script")
        || msg.contains("unexpected eof")
        || msg.contains("unterminated"))
}
