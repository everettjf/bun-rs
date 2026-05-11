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
}

/// One-shot: read a file, transpile if needed, rewrite ESM, return a
/// [`PreparedModule`]. Resolution of nested imports happens at runtime via
/// `__bun_require`.
pub fn prepare(path: &Path) -> Result<PreparedModule, LoaderError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| LoaderError::Io(path.to_path_buf(), e))?;
    let transpiled = bun_transpile::transpile_file(path, &source)?;
    let analysis = rewriter::rewrite_to_iife(&transpiled.code)?;
    Ok(PreparedModule {
        path: path.to_path_buf(),
        static_imports: analysis.imports,
        rewritten: analysis.code,
    })
}
