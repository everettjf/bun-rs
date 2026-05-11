//! bun-rs runtime: glue between the CLI, JSC, and our transpiler.
//!
//! Currently delivers P0+P1:
//!   - `cli_main` parses argv, drives `-e <code>` / `run <file>` / `--version`
//!   - `Runtime::new` builds a Context with console + process globals
//!   - `Runtime::eval_file` transpiles (if .ts/.tsx) and evaluates a script

use std::path::{Path, PathBuf};

use bun_jsc::{Callback, Context, JsException, Value};

mod console;
mod modules;
mod process_global;
mod timers;

pub use console::install_console;
pub use modules::{install_module_loader, run_entry, LoaderRuntimeError};
pub use process_global::install_process;
pub use timers::{install_timers, run_event_loop};

/// Top-level wrapper that holds the Context + has all builtins installed.
pub struct Runtime {
    pub ctx: Context,
}

impl Runtime {
    pub fn new(argv: Vec<String>) -> Self {
        let ctx = Context::new();
        install_console(&ctx);
        install_process(&ctx, argv);
        install_timers(&ctx);
        install_module_loader(&ctx);
        install_global_this(&ctx);
        Self { ctx }
    }

    /// Evaluate inline source, with `source_url` shown in stack traces.
    pub fn eval_string(&self, code: &str, source_url: &str) -> Result<Value<'_>, JsException> {
        self.ctx.eval(code, Some(source_url))
    }

    /// Run a script file as the entry module. Goes through the module loader
    /// so `import`/`export` work at top level.
    pub fn eval_file(&self, path: &Path) -> Result<(), RuntimeError> {
        run_entry(&self.ctx, path).map_err(|e| match e {
            LoaderRuntimeError::Io(p, ioe) => RuntimeError::ReadFile(p, ioe),
            other => RuntimeError::Throw(other.to_string()),
        })
    }

    /// Drive timers until the queue is empty. Call after the entry-point
    /// `eval_*` returns so `setTimeout` callbacks get a chance to fire.
    pub fn drain(&self) {
        run_event_loop(&self.ctx);
    }
}

/// Install `globalThis` (it's already there in JSC, but we make it explicit).
fn install_global_this(_ctx: &Context) {
    // JSC's default global object already binds `globalThis` per the spec.
    // Hook left here as a place to add Bun.* / window-like aliases later.
}

fn format_exception(exc: &JsException) -> String {
    let msg = exc.message();
    if let Some(stack) = exc.stack() {
        format!("{msg}\n{stack}")
    } else {
        msg
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("could not read {path}: {err}", path = .0.display(), err = .1)]
    ReadFile(PathBuf, std::io::Error),
    #[error(transparent)]
    Transpile(#[from] bun_transpile::TranspileError),
    #[error("Uncaught {0}")]
    Throw(String),
    #[error("usage: bun-rs [run] <file> | -e <code> | --version")]
    Usage,
}

// ── CLI entrypoint ───────────────────────────────────────────────────────────

pub fn cli_main() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(RuntimeError::Usage) => {
            eprintln!("{}", RuntimeError::Usage);
            64
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn run() -> Result<(), RuntimeError> {
    let mut args = std::env::args().collect::<Vec<_>>();
    let _argv0 = if args.is_empty() {
        "bun-rs".to_string()
    } else {
        args.remove(0)
    };

    if args.is_empty() {
        // Future: REPL. For now treat as usage error.
        return Err(RuntimeError::Usage);
    }

    match args[0].as_str() {
        "--version" | "-v" => {
            println!("bun-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--help" | "-h" => {
            println!(
                "bun-rs — a Rust port of bun.\n\
                 \n\
                 Usage:\n  \
                   bun-rs run <file>        run a JS/TS file\n  \
                   bun-rs <file>            shorthand for `run`\n  \
                   bun-rs -e <code>         evaluate inline code\n  \
                   bun-rs --version         print version\n"
            );
            Ok(())
        }
        "-e" | "--eval" | "-p" | "--print" => {
            let print_result = matches!(args[0].as_str(), "-p" | "--print");
            let code = args.get(1).ok_or(RuntimeError::Usage)?.clone();
            // After `-e`, the rest is script argv.
            let script_args = args.drain(2..).collect::<Vec<_>>();
            let mut all_argv = vec!["bun-rs".to_string(), "[inline]".to_string()];
            all_argv.extend(script_args);
            let rt = Runtime::new(all_argv);
            let result = rt.eval_string(&code, "[inline]");
            match result {
                Ok(v) => {
                    if print_result && !v.is_undefined() {
                        println!("{}", v.to_string());
                    }
                    rt.drain();
                    Ok(())
                }
                Err(exc) => Err(RuntimeError::Throw(format_exception(&exc))),
            }
        }
        "run" => {
            let file = args.get(1).ok_or(RuntimeError::Usage)?.clone();
            let script_args = args.drain(2..).collect::<Vec<_>>();
            let mut all_argv = vec!["bun-rs".to_string(), file.clone()];
            all_argv.extend(script_args);
            let rt = Runtime::new(all_argv);
            rt.eval_file(Path::new(&file))?;
            rt.drain();
            Ok(())
        }
        _ => {
            // Bare-file shorthand: `bun-rs script.ts` ≡ `bun-rs run script.ts`.
            let file = args[0].clone();
            let path = PathBuf::from(&file);
            if path.exists() {
                let script_args = args.drain(1..).collect::<Vec<_>>();
                let mut all_argv = vec!["bun-rs".to_string(), file];
                all_argv.extend(script_args);
                let rt = Runtime::new(all_argv);
                rt.eval_file(&path)?;
                rt.drain();
                Ok(())
            } else {
                Err(RuntimeError::Usage)
            }
        }
    }
}

// Re-export glue for outside crates that build Bun.* APIs.
pub use bun_jsc;

/// Helper: create a JS function from a `Fn(args) -> Result<Value, String>` and
/// bind it as `globalThis[name]`.
pub fn bind_global_fn<F>(ctx: &Context, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    let global = ctx.global_object();
    global
        .set_property(name, &cb.value_in(ctx))
        .unwrap_or_else(|e| eprintln!("warning: failed to install {name}: {e}"));
    // Leak the Callback wrapper — the underlying JS object is owned by JSC.
    std::mem::forget(cb);
}
