//! Single-file bundler.
//!
//! Walks the import graph from `entry`, prepares each module via
//! `bun-loader::prepare` (which transpiles + rewrites ESM to IIFE form),
//! then emits one self-contained JS file. The bundle includes a small
//! runtime shim that simulates the `__bun_require` + `__exports`
//! environment each module was rewritten against.
//!
//! Limitations (this is an MVP):
//! - No tree-shaking; every module reachable from the entry is included
//!   in full.
//! - No code splitting / dynamic-import-aware chunks. A `import()` in
//!   the source still gets routed through the bundle's `__bun_require`
//!   helper, but its target must be statically resolvable from a
//!   string literal.
//! - No minification.
//! - No source map (yet).
//! - `node:*` imports are kept as `__bun_require("node:fs")` calls; the
//!   output expects the host runtime to provide them (bun-rs does;
//!   plain Node doesn't, so it'd need `const fs = require("node:fs")`
//!   shimmed in by the caller).
//!
//! Output shape:
//!
//! ```js
//! (() => {
//!   const __modules = {};
//!   const __pending = {};
//!   async function __bun_require(spec, importer) { ... routes by id ... }
//!   // Each module emitted as an async fn:
//!   const __M_0 = async (__exports, __filename, __dirname) => { /* body */ };
//!   const __M_1 = async (__exports, __filename, __dirname) => { /* body */ };
//!   // ...
//!   await __bun_load_index(0);  // entry
//! })();
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error(transparent)]
    Loader(#[from] bun_loader::LoaderError),
    #[error(transparent)]
    Resolve(#[from] bun_loader::ResolveError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("entry not found: {0}")]
    EntryNotFound(PathBuf),
}

pub struct BundleOutput {
    pub code: String,
    pub modules: Vec<PathBuf>,
}

/// Bundle from `entry` into a single string of JS.
pub fn bundle(entry: &Path) -> Result<BundleOutput, BundleError> {
    let entry_abs = entry
        .canonicalize()
        .map_err(|_| BundleError::EntryNotFound(entry.to_path_buf()))?;
    let resolver = bun_loader::Resolver::new();

    // path → (id, prepared, deps_resolved: Vec<(spec, resolved_abs_path)>)
    let mut id_by_path: HashMap<PathBuf, u32> = HashMap::new();
    let mut modules: Vec<(PathBuf, bun_loader::PreparedModule, Vec<(String, PathBuf)>)> =
        Vec::new();
    let mut queue = vec![entry_abs.clone()];
    while let Some(path) = queue.pop() {
        if id_by_path.contains_key(&path) {
            continue;
        }
        let id = modules.len() as u32;
        id_by_path.insert(path.clone(), id);

        let prepared = bun_loader::prepare(&path)?;
        let mut resolved_deps = Vec::new();
        for spec in &prepared.static_imports {
            // `node:*` and unresolvable bare specifiers stay external.
            if spec.starts_with("node:") {
                resolved_deps.push((spec.clone(), PathBuf::from(spec)));
                continue;
            }
            match resolver.resolve(spec, &path) {
                Ok(abs) => {
                    queue.push(abs.clone());
                    resolved_deps.push((spec.clone(), abs));
                }
                Err(_) => {
                    // External: leave it to be routed via __bun_require at
                    // runtime.
                    resolved_deps.push((spec.clone(), PathBuf::from(spec)));
                }
            }
        }
        modules.push((path, prepared, resolved_deps));
    }

    // Rewrite each module body so __bun_require("./y", __filename) calls
    // point at our local __bun_load(<id>) instead.
    let path_to_id: HashMap<&PathBuf, u32> = id_by_path
        .iter()
        .map(|(p, &id)| (p, id))
        .collect();

    let total = modules.len();
    let mut emit_paths: Vec<PathBuf> = Vec::with_capacity(total);

    let mut out = String::new();
    out.push_str("// bun-rs bundle\n");
    out.push_str("(async () => {\n");
    out.push_str("  const __cache = {};\n");
    out.push_str("  const __pending = {};\n");
    out.push_str("  const __ALL = [\n");

    for (id, (path, prepared, deps)) in modules.iter().enumerate() {
        emit_paths.push(path.clone());
        let rewritten = rewrite_require_calls(&prepared.rewritten, deps, &path_to_id);
        let path_str = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let dir_str = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        out.push_str(&format!("    // module {id}: {path_str}\n"));
        out.push_str(
            "    async (__exports, __bun_require, __filename, __dirname, __bun_meta) => {\n",
        );
        out.push_str(&rewritten);
        out.push_str("\n    },\n");
        let _ = dir_str;
    }

    out.push_str("  ];\n\n");
    out.push_str("  const __PATHS = [\n");
    for (path, _, _) in &modules {
        let path_str = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("    \"{path_str}\",\n"));
    }
    out.push_str("  ];\n\n");
    out.push_str(BUNDLE_RUNTIME);
    out.push_str(&format!("  await __bun_load(0);\n}})();\n"));
    let _ = total;
    Ok(BundleOutput {
        code: out,
        modules: emit_paths,
    })
}

/// Rewrite `__bun_require(<spec>, __filename)` calls to local
/// `__bun_load(<id>)` calls, mapping each spec via `deps`. Unresolved specs
/// (e.g. node:* externals or bare names with no local match) are left as
/// runtime-routed via the host's own `__bun_require`.
fn rewrite_require_calls(
    body: &str,
    deps: &[(String, PathBuf)],
    path_to_id: &HashMap<&PathBuf, u32>,
) -> String {
    // Find each occurrence of `__bun_require(` and parse out the spec
    // string literal. Build the replacement based on the resolved path.
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let needle = b"__bun_require(";
    let mut i = 0usize;
    while i < bytes.len() {
        if i + needle.len() < bytes.len() && &bytes[i..i + needle.len()] == needle {
            // Try to parse the first arg as a string literal.
            let after = i + needle.len();
            if let Some((spec, _arg_end)) = parse_string_arg(&body[after..]) {
                // Find the matching outer `)` to determine the full call.
                if let Some(end) = find_matching_paren(&body[i + needle.len() - 1..]) {
                    let full_end = i + needle.len() - 1 + end + 1;
                    // Look up resolved path for spec.
                    let resolved = deps
                        .iter()
                        .find(|(s, _)| *s == spec)
                        .map(|(_, p)| p.clone());
                    if let Some(rp) = resolved {
                        if let Some(&id) = path_to_id.get(&rp) {
                            // Internal: route via local table.
                            out.push_str(&format!("__bun_load({id})"));
                            i = full_end;
                            continue;
                        }
                    }
                    // External: keep the original call. The host runtime
                    // (bun-rs) handles node:* and unresolved bare names.
                    out.push_str(&body[i..full_end]);
                    i = full_end;
                    continue;
                }
            }
        }
        out.push(body[i..].chars().next().unwrap());
        i += body[i..].chars().next().unwrap().len_utf8();
    }
    out
}

fn parse_string_arg(s: &str) -> Option<(String, usize)> {
    // Skip leading whitespace.
    let mut chars = s.char_indices();
    let (_, mut c) = chars.next()?;
    while c.is_whitespace() {
        let (_, n) = chars.next()?;
        c = n;
    }
    let quote = if c == '"' || c == '\'' { c } else { return None };
    let mut s_out = String::new();
    let mut last_idx = 0;
    for (idx, ch) in chars {
        last_idx = idx;
        if ch == '\\' {
            // crude: just take next char as-is
            continue;
        }
        if ch == quote {
            return Some((s_out, idx + 1));
        }
        s_out.push(ch);
    }
    let _ = last_idx;
    None
}

fn find_matching_paren(s: &str) -> Option<usize> {
    // s should start with '('. Find the matching ')'.
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes[0] != b'(' {
        return None;
    }
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' { i += 2; continue; }
            if b == q { in_str = None; }
        } else if b == b'"' || b == b'\'' {
            in_str = Some(b);
        } else if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

const BUNDLE_RUNTIME: &str = r#"  async function __bun_load(id) {
    if (id in __cache) return __cache[id];
    if (id in __pending) return __pending[id];
    const exports = {};
    __pending[id] = exports;
    const path = __PATHS[id] || "";
    const dir = path.lastIndexOf("/") >= 0 ? path.slice(0, path.lastIndexOf("/")) : "";
    const meta = { url: path ? "file://" + path : "", filename: path, dirname: dir, main: id === 0 };
    await __ALL[id](exports, __bun_require_external, path, dir, meta);
    __cache[id] = exports;
    delete __pending[id];
    return exports;
  }
  async function __bun_require_external(spec, _importer) {
    if (typeof globalThis.__bun_require === "function") return globalThis.__bun_require(spec, _importer);
    throw new Error("bundle external '" + spec + "' has no host loader");
  }

"#;
