//! Module resolution + ESM → IIFE rewriting for bun-rs.
//!
//! This crate is JS-engine-agnostic: it produces a [`PreparedModule`] from a
//! path, and the runtime decides how to actually evaluate the resulting JS.
//!
//! Two pieces:
//!   - [`Resolver`]: spec → absolute path (relative, absolute, node_modules)
//!   - [`rewrite_to_iife`]: takes JS source (post-transpile) and rewrites
//!     top-level `import`/`export` into calls to a sync `__bun_require` global
//!     and assignments to `__exports`.

mod resolver;
mod rewriter;

pub use resolver::{ResolveError, Resolver};
pub use rewriter::{rewrite_to_iife, ModuleAnalysis, RewriteError};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error("could not read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error(transparent)]
    Transpile(#[from] bun_transpile::TranspileError),
    #[error(transparent)]
    Rewrite(#[from] RewriteError),
    #[error("parse error in {0}: {1}")]
    ParseModule(PathBuf, String),
}

/// A module ready to be wrapped + evaluated by the runtime.
#[derive(Debug, Clone)]
pub struct PreparedModule {
    /// Absolute path of this module (cache key).
    pub path: PathBuf,
    /// Static specifiers this module imports (already resolved? no — runtime
    /// will pass them to `__bun_require` at exec time, which will recursively
    /// load them through this same pipeline).
    pub static_imports: Vec<String>,
    /// JS source ready to be wrapped in the IIFE and evaluated.
    pub rewritten: String,
    /// For each line of `rewritten` (1-indexed), the source line in the
    /// original *post-transpile* JS (1-indexed; 0 = synthetic line we made
    /// up, e.g. a `__bun_require` shim for an import).
    ///
    /// Note: this map is post-transpile → post-rewriter. If transpile
    /// (TS → JS) shifted lines too, the user-visible map will be off in
    /// those frames. For TS files without JSX, oxc's transpile typically
    /// preserves lines.
    pub line_map: Vec<u32>,
    /// The original source text (the user's .ts file). Kept so error
    /// formatters can show a code excerpt at the offending line.
    pub original_source: String,
}

/// One-shot: read a file, transpile if needed, rewrite ESM, return a
/// [`PreparedModule`]. Resolution of nested imports happens at runtime via
/// `__bun_require`.
pub fn prepare(path: &Path) -> Result<PreparedModule, LoaderError> {
    // Non-JS file types: wrap as a CJS module that exports a parsed
    // value. ESM `import x from "./file.json"` and `import x from
    // "./file.txt"` go through this path.
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // Binary assets short-circuit before reading the file (some are not
    // UTF-8). Bun returns the absolute file path string from
    // `import asset from "./file.png"`.
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp"
        | "mp3" | "mp4" | "wav" | "ogg" | "webm" | "mov"
        | "ttf" | "otf" | "woff" | "woff2" | "eot"
        | "pdf" | "zip" | "wasm" | "node" | "bin" => {
            let p = path.to_string_lossy().into_owned();
            let escaped = p
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = `{}`;\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: String::new(),
            });
        }
        _ => {}
    }

    let source = std::fs::read_to_string(path)
        .map_err(|e| LoaderError::Io(path.to_path_buf(), e))?;
    match ext.as_str() {
        "json" => {
            // Strict JSON — fast path: embed source as JS string literal
            // and run JSON.parse at module-init time.
            let escaped = source
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = JSON.parse(`{}`);\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: source,
            });
        }
        "jsonc" | "json5" | "lock" => {
            // JSONC has // comments; JSON5 has comments + trailing commas
            // + unquoted keys + single quotes. Parse with the json5 crate
            // (which is JSON5-strict superset of JSONC) then emit as JSON
            // literal so the runtime side is identical to .json.
            let value: serde_json::Value = json5::from_str(&source)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let canonical = serde_json::to_string(&value)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let escaped = canonical
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = JSON.parse(`{}`);\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: source,
            });
        }
        "toml" => {
            let value: toml::Value = toml::from_str(&source)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let as_json = serde_json::to_value(&value)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let canonical = serde_json::to_string(&as_json)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let escaped = canonical
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = JSON.parse(`{}`);\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: source,
            });
        }
        "yaml" | "yml" => {
            let value: serde_yaml::Value = serde_yaml::from_str(&source)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let as_json = serde_json::to_value(&value)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let canonical = serde_json::to_string(&as_json)
                .map_err(|e| LoaderError::ParseModule(path.to_path_buf(), e.to_string()))?;
            let escaped = canonical
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = JSON.parse(`{}`);\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: source,
            });
        }
        "txt" | "html" | "css" => {
            // Default import is the raw text body.
            let escaped = source
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = `{}`;\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: source,
            });
        }
        // SVG is text — could go either way; Bun returns the path so we
        // do too.
        "svg" => {
            let p = path.to_string_lossy().into_owned();
            let escaped = p
                .replace('\\', "\\\\")
                .replace('`', "\\`")
                .replace("${", "\\${");
            let wrapped = format!(
                "module.exports = `{}`;\n",
                escaped
            );
            return Ok(PreparedModule {
                path: path.to_path_buf(),
                static_imports: vec![],
                rewritten: wrapped,
                line_map: vec![0],
                original_source: String::new(),
            });
        }
        _ => {}
    }

    let transpiled = bun_transpile::transpile_file(path, &source)?;
    let analysis = rewriter::rewrite_to_iife(&transpiled.code)?;
    Ok(PreparedModule {
        path: path.to_path_buf(),
        static_imports: analysis.imports,
        rewritten: analysis.code,
        line_map: analysis.line_map,
        original_source: source,
    })
}
