//! Minimal npm package installer.
//!
//! Walks `package.json` deps, hits `registry.npmjs.org` for each package's
//! manifest, picks a version matching the requested semver range (very
//! loosely — see `pick_version`), downloads the tarball, extracts it under
//! `node_modules/<name>/`, then recurses on its own `dependencies` field.
//! A flat `bun-rs.lock.json` records the resolved tree.
//!
//! Deliberately out of scope for the MVP:
//!   - true semver range satisfaction (`^`, `~`, prerelease, etc.) —
//!     we honor an exact match if pinned, otherwise pick the registry's
//!     `dist-tags.latest` and warn.
//!   - peer / optional / dev dep semantics distinct from regular deps —
//!     `--production` skips devDependencies.
//!   - workspaces / monorepos
//!   - lifecycle scripts (pre/post install)
//!   - native module builds
//!   - scoped registries / auth
//!
//! Scoped packages (`@scope/name`) are supported in resolution + extraction.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("registry: {0}")]
    Registry(String),
    #[error("package.json not found at {0}")]
    NoManifest(PathBuf),
}

pub struct InstallOptions {
    pub cwd: PathBuf,
    pub production: bool,
    pub registry: String,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            production: false,
            registry: "https://registry.npmjs.org".into(),
        }
    }
}

pub fn install(opts: &InstallOptions) -> Result<InstallReport, InstallError> {
    let pkg_path = opts.cwd.join("package.json");
    if !pkg_path.exists() {
        return Err(InstallError::NoManifest(pkg_path));
    }
    let pkg: Json = serde_json::from_str(&std::fs::read_to_string(&pkg_path)?)?;
    let modules_dir = opts.cwd.join("node_modules");
    std::fs::create_dir_all(&modules_dir)?;

    let mut state = State {
        opts,
        modules_dir,
        installed: HashMap::new(),
        manifest_cache: HashMap::new(),
        report: InstallReport::default(),
    };

    let mut deps = collect_top_deps(&pkg, opts.production);
    while let Some((name, range)) = deps.pop() {
        if state.installed.contains_key(&name) {
            continue;
        }
        let installed_version = install_one(&mut state, &name, &range)?;
        // Recurse on this package's own deps.
        let pkg_dir = state.modules_dir.join(&name);
        let nested = pkg_dir.join("package.json");
        if let Ok(text) = std::fs::read_to_string(&nested) {
            if let Ok(j) = serde_json::from_str::<Json>(&text) {
                for (n, r) in collect_top_deps(&j, true) {
                    if !state.installed.contains_key(&n) {
                        deps.push((n, r));
                    }
                }
            }
        }
        state.installed.insert(name, installed_version);
    }

    // Write a simple lockfile.
    let lock_path = opts.cwd.join("bun-rs.lock.json");
    let lock = serde_json::to_string_pretty(&serde_json::json!({
        "lockfileVersion": 0,
        "packages": state
            .installed
            .iter()
            .collect::<BTreeMap<_, _>>(),
    }))?;
    std::fs::write(&lock_path, lock)?;

    Ok(state.report)
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<(String, String)>,
}

struct State<'a> {
    opts: &'a InstallOptions,
    modules_dir: PathBuf,
    installed: HashMap<String, String>,
    manifest_cache: HashMap<String, Json>,
    report: InstallReport,
}

fn collect_top_deps(pkg: &Json, production: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(obj) = pkg.get("dependencies").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            out.push((k.clone(), v.as_str().unwrap_or("latest").to_string()));
        }
    }
    if !production {
        if let Some(obj) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                out.push((k.clone(), v.as_str().unwrap_or("latest").to_string()));
            }
        }
    }
    out
}

fn install_one(
    state: &mut State<'_>,
    name: &str,
    range: &str,
) -> Result<String, InstallError> {
    let manifest = fetch_manifest(state, name)?;
    let version = pick_version(&manifest, range)
        .ok_or_else(|| InstallError::Registry(format!("no version for {name} {range}")))?;
    let tarball_url = manifest
        .get("versions")
        .and_then(|vs| vs.get(&version))
        .and_then(|v| v.get("dist"))
        .and_then(|d| d.get("tarball"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| InstallError::Registry(format!("no tarball for {name} {version}")))?
        .to_string();

    let pkg_dir = state.modules_dir.join(name);
    // Re-create on each run (idempotent), but skip if already populated.
    if pkg_dir.join("package.json").exists() {
        return Ok(version);
    }
    std::fs::create_dir_all(&pkg_dir)?;

    eprintln!("  + {name}@{version}");
    let response = ureq::get(&tarball_url)
        .call()
        .map_err(|e| InstallError::Http(e.to_string()))?;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_body()
        .as_reader()
        .read_to_end(&mut bytes)?;
    extract_tarball(&bytes, &pkg_dir)?;

    state.report.installed.push((name.to_string(), version.clone()));
    Ok(version)
}

fn fetch_manifest<'a>(
    state: &'a mut State<'_>,
    name: &str,
) -> Result<&'a Json, InstallError> {
    if !state.manifest_cache.contains_key(name) {
        let url = format!("{}/{}", state.opts.registry, name);
        let mut resp = ureq::get(&url)
            .header("accept", "application/json")
            .call()
            .map_err(|e| InstallError::Http(e.to_string()))?;
        let mut body = String::new();
        resp.body_mut().as_reader().read_to_string(&mut body)?;
        let j: Json = serde_json::from_str(&body)?;
        state.manifest_cache.insert(name.to_string(), j);
    }
    Ok(state.manifest_cache.get(name).unwrap())
}

/// Pick the version to install. Very loose semver: exact match (pinned)
/// trumps everything; otherwise we look up `dist-tags.<range>` (e.g.
/// "latest"); otherwise we strip the leading `^`/`~`/`>=` and try exact;
/// otherwise we fall back to `dist-tags.latest`.
fn pick_version(manifest: &Json, range: &str) -> Option<String> {
    let versions = manifest.get("versions").and_then(|v| v.as_object())?;
    if versions.contains_key(range) {
        return Some(range.to_string());
    }
    if let Some(t) = manifest
        .get("dist-tags")
        .and_then(|t| t.get(range))
        .and_then(|t| t.as_str())
    {
        return Some(t.to_string());
    }
    let trimmed = range.trim_start_matches(|c: char| {
        matches!(c, '^' | '~' | '>' | '<' | '=' | ' ' | 'v')
    });
    if versions.contains_key(trimmed) {
        return Some(trimmed.to_string());
    }
    // Fallback: the highest-looking version under `latest`.
    manifest
        .get("dist-tags")
        .and_then(|t| t.get("latest"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<(), InstallError> {
    // npm tarballs are gzipped tar. Entries are prefixed `package/`; strip
    // that so files land at dest/<rest>.
    let dec = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(dec);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let stripped = match path.strip_prefix("package") {
            Ok(p) => p.to_path_buf(),
            Err(_) => path,
        };
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(stripped);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        entry.unpack(&out_path)?;
    }
    Ok(())
}
