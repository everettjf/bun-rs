//! bun-rs runtime: glue between the CLI, JSC, and our transpiler.
//!
//! Currently delivers P0+P1:
//!   - `cli_main` parses argv, drives `-e <code>` / `run <file>` / `--version`
//!   - `Runtime::new` builds a Context with console + process globals
//!   - `Runtime::eval_file` transpiles (if .ts/.tsx) and evaluates a script

use std::path::{Path, PathBuf};

use bun_jsc::{Callback, Context, JsException, Value};

mod async_rt;
mod babel_helpers;
mod buffer;
mod bun_api;
mod console;
mod modules;
mod node_builtins;
mod process_global;
mod repl;
mod sourcemap;
mod test_runner;
mod timers;
mod web;

pub use console::install_console;
pub use modules::{install_module_loader, run_entry, LoaderRuntimeError};
pub use process_global::install_process;
pub use timers::{install_timers, run_event_loop};

/// Absolute path of the current bun-rs binary, used as argv[0] /
/// process.execPath. Bun's test harnesses rely on `process.execPath`
/// pointing at a real file so they can re-spawn the binary.
pub fn bun_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "bun-rs".to_string())
}

/// Top-level wrapper that holds the Context + has all builtins installed.
pub struct Runtime {
    pub ctx: Context,
}

impl Runtime {
    pub fn new(argv: Vec<String>) -> Self {
        // Install the rustls crypto provider once. Idempotent; required
        // before any TLS use (Bun.serve tls:, fetch over https).
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        async_rt::init();
        let ctx = Context::new();
        install_console(&ctx);
        install_process(&ctx, argv);
        install_timers(&ctx);
        babel_helpers::install(&ctx);
        install_module_loader(&ctx);
        web::install_web(&ctx);
        buffer::install(&ctx);
        // Pre-warm node:stream so fs.createReadStream can use globalThis
        // refs without making the user import "node:stream" first.
        let _ = node_builtins::load(&ctx, "stream");
        bun_api::install_bun(&ctx);
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
        let mapped = sourcemap::remap_stack(&stack);
        format!("{msg}\n{mapped}")
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

    // Strip Bun-specific runtime flags we don't implement (so test spawns
    // that pass --smol / --no-warnings / --cwd / --silent don't trip the
    // usage-error path). We just consume the flag (and its value if it
    // takes one) and continue.
    let one_arg_flags = [
        "--cwd", "--watch", "--inspect", "--inspect-brk", "--inspect-wait",
        "--config", "--bun", "--filter", "--port", "--tsconfig",
        "--require", "--preload", "--define", "--external", "--target",
        "--loader", "--minify-syntax", "--minify-whitespace", "--minify",
    ];
    let zero_arg_flags = [
        "--smol", "--no-warnings", "--silent", "--no-deprecation",
        "--throw-deprecation", "--no-lazy", "--enable-source-maps",
        "--disable-proto", "--disable-warning", "--trace-warnings",
        "--no-print", "--prefer-offline", "--prefer-latest",
        "--use", "--print", "--compact",
    ];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if zero_arg_flags.contains(&a) {
            args.remove(i);
        } else if one_arg_flags.contains(&a) {
            args.remove(i);
            if i < args.len() { args.remove(i); }
        } else if a.starts_with("--") && a.contains('=') {
            // --flag=value: drop wholesale.
            args.remove(i);
        } else {
            i += 1;
        }
    }

    if args.is_empty() {
        // No args → REPL. If stdin isn't a TTY (piped), fall back to "read all
        // stdin and eval it like -e".
        let rt = Runtime::new(vec![bun_exe_path(), "[repl]".to_string()]);
        return repl::run(&rt.ctx);
    }

    match args[0].as_str() {
        "--version" | "-v" => {
            // Match Bun's output format (bare version number).
            println!("{}", env!("CARGO_PKG_VERSION"));
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
            let mut all_argv = vec![bun_exe_path(), "[inline]".to_string()];
            all_argv.extend(script_args);
            let rt = Runtime::new(all_argv);
            // Wrap in async IIFE so top-level `await` works in `-e` code.
            // Store any rejection on globalThis so we can pick it up after drain.
            let wrapped = format!(
                "(async function () {{ try {{ {} }} catch (e) {{ globalThis.__bun_e_err = e; throw e; }} }})().catch(e => {{ globalThis.__bun_e_err = e; }})",
                code
            );
            let result = rt.eval_string(&wrapped, "[inline]");
            match result {
                Ok(v) => {
                    if print_result && !v.is_undefined() {
                        println!("{}", v.to_string());
                    }
                    rt.drain();
                    // Check for any captured rejection.
                    let err_check = rt.ctx.eval(
                        "globalThis.__bun_e_err",
                        Some("[inline-err-check]"),
                    );
                    if let Ok(v) = err_check {
                        if !v.is_undefined() && !v.is_null() {
                            let msg = v.to_string();
                            let _ = rt.ctx.eval("delete globalThis.__bun_e_err", Some("[inline-err-cleanup]"));
                            return Err(RuntimeError::Throw(msg));
                        }
                    }
                    Ok(())
                }
                Err(exc) => Err(RuntimeError::Throw(format_exception(&exc))),
            }
        }
        "install" => {
            let mut prod = false;
            for a in &args[1..] {
                if a == "--production" { prod = true; }
            }
            let opts = bun_install::InstallOptions {
                cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                production: prod,
                registry: std::env::var("BUN_REGISTRY")
                    .unwrap_or_else(|_| "https://registry.npmjs.org".into()),
            };
            match bun_install::install(&opts) {
                Ok(r) => {
                    eprintln!("\n{} packages installed", r.installed.len());
                    return Ok(());
                }
                Err(e) => {
                    return Err(RuntimeError::Throw(format!("install failed: {e}")));
                }
            }
        }
        "build" => {
            // bun-rs build <entry> [--outfile <path>]
            let entry = args.get(1).ok_or(RuntimeError::Usage)?.clone();
            let mut outfile: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--outfile" | "-o" => {
                        i += 1;
                        outfile = args.get(i).cloned();
                    }
                    _ => {}
                }
                i += 1;
            }
            let entry_path = Path::new(&entry);
            let bundle = bun_bundler::bundle(entry_path)
                .map_err(|e| RuntimeError::Throw(format!("bundle failed: {e}")))?;
            match outfile {
                Some(p) => {
                    std::fs::write(&p, &bundle.code)
                        .map_err(|e| RuntimeError::ReadFile(PathBuf::from(&p), e))?;
                    eprintln!("wrote {} ({} modules, {} bytes)", p, bundle.modules.len(), bundle.code.len());
                }
                None => {
                    println!("{}", bundle.code);
                }
            }
            return Ok(());
        }
        "test" => {
            let paths: Vec<String> = args.drain(1..).collect();
            let code = test_runner::run_tests(paths);
            std::process::exit(code);
        }
        "run" => {
            let file = args.get(1).ok_or(RuntimeError::Usage)?.clone();
            let script_args = args.drain(2..).collect::<Vec<_>>();
            let mut all_argv = vec![bun_exe_path(), file.clone()];
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
                let mut all_argv = vec![bun_exe_path(), file];
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
