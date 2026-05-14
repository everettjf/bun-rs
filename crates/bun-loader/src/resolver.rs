//! Path resolver — thin wrapper over `oxc_resolver`.
//!
//! Handles relative/absolute/bare specifiers, ts/tsx/js/jsx extensions,
//! `index.<ext>` fallback, and node_modules + package.json `exports`/`main`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_resolver::{ResolveOptions, Resolver as OxcResolver};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("cannot find module '{spec}' from '{from}'")]
    NotFound { spec: String, from: PathBuf },
    #[error("resolve error for '{spec}' from '{from}': {err}")]
    Other {
        spec: String,
        from: PathBuf,
        err: String,
    },
}

#[derive(Clone)]
pub struct Resolver {
    inner: Arc<OxcResolver>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        let options = ResolveOptions {
            extensions: vec![
                ".ts".into(),
                ".tsx".into(),
                ".mts".into(),
                ".cts".into(),
                ".js".into(),
                ".jsx".into(),
                ".mjs".into(),
                ".cjs".into(),
                ".json".into(),
            ],
            condition_names: vec!["bun".into(), "import".into(), "default".into(), "node".into()],
            main_fields: vec!["module".into(), "main".into()],
            ..ResolveOptions::default()
        };
        Self {
            inner: Arc::new(OxcResolver::new(options)),
        }
    }

    /// Resolve `spec` as imported from a file at `importer_file`.
    ///
    /// `importer_file` should be the path of the module doing the import;
    /// we feed `oxc_resolver` its parent dir.
    pub fn resolve(&self, spec: &str, importer_file: &Path) -> Result<PathBuf, ResolveError> {
        let from_dir = importer_file
            .parent()
            .unwrap_or_else(|| Path::new("."));
        // Fast path for relative JSON/JSONC imports — oxc_resolver tries
        // to parse them as package manifests which fails on empty files.
        if (spec.starts_with("./") || spec.starts_with("../"))
            && (spec.ends_with(".json") || spec.ends_with(".jsonc") || spec.ends_with(".json5"))
        {
            let p = from_dir.join(spec);
            if p.exists() {
                return Ok(p);
            }
        }
        match self.inner.resolve(from_dir, spec) {
            Ok(r) => Ok(r.path().to_path_buf()),
            Err(e) => {
                let s = e.to_string();
                if s.contains("not found") || s.contains("Cannot find") {
                    Err(ResolveError::NotFound {
                        spec: spec.to_string(),
                        from: importer_file.to_path_buf(),
                    })
                } else {
                    Err(ResolveError::Other {
                        spec: spec.to_string(),
                        from: importer_file.to_path_buf(),
                        err: s,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn td(label: &str) -> PathBuf {
        let nano = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("bun-rs-resolver-{label}-{nano}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn relative_ts_resolves() {
        let dir = td("rel");
        fs::write(dir.join("a.ts"), "").unwrap();
        fs::write(dir.join("b.ts"), "").unwrap();
        let r = Resolver::new();
        let got = r.resolve("./b", &dir.join("a.ts")).unwrap();
        assert_eq!(got, dir.join("b.ts").canonicalize().unwrap());
    }

    #[test]
    fn relative_index_resolves() {
        let dir = td("idx");
        fs::create_dir(dir.join("a")).unwrap();
        fs::write(dir.join("a/index.ts"), "").unwrap();
        fs::write(dir.join("main.ts"), "").unwrap();
        let r = Resolver::new();
        let got = r.resolve("./a", &dir.join("main.ts")).unwrap();
        assert_eq!(got, dir.join("a/index.ts").canonicalize().unwrap());
    }

    #[test]
    fn bare_specifier_via_node_modules() {
        let dir = td("nm");
        fs::create_dir_all(dir.join("node_modules/foo")).unwrap();
        fs::write(
            dir.join("node_modules/foo/package.json"),
            r#"{"name":"foo","main":"./index.js"}"#,
        )
        .unwrap();
        fs::write(dir.join("node_modules/foo/index.js"), "module.exports = 1;").unwrap();
        fs::write(dir.join("main.ts"), "").unwrap();
        let r = Resolver::new();
        let got = r.resolve("foo", &dir.join("main.ts")).unwrap();
        assert_eq!(got, dir.join("node_modules/foo/index.js").canonicalize().unwrap());
    }

    #[test]
    fn missing_returns_not_found() {
        let dir = td("nf");
        fs::write(dir.join("a.ts"), "").unwrap();
        let r = Resolver::new();
        let err = r.resolve("./does-not-exist", &dir.join("a.ts")).unwrap_err();
        match err {
            ResolveError::NotFound { .. } => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
