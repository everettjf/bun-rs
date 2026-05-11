//! TypeScript / JSX → JavaScript transpile via oxc.
//!
//! MVP scope: strip types, lower JSX (if any) using oxc_transformer's defaults.
//! No JSX runtime config yet — caller can ask for `react-jsx` later.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TranspileError {
    #[error("parse errors:\n{0}")]
    Parse(String),
    #[error("transform errors:\n{0}")]
    Transform(String),
}

#[derive(Debug, Clone)]
pub struct Transpiled {
    pub code: String,
}

/// Transpile a source file. The path's extension is used to pick the
/// dialect (`.ts`/`.tsx`/`.jsx`/`.mjs`/...).
pub fn transpile_file(path: &Path, source: &str) -> Result<Transpiled, TranspileError> {
    let source_type = SourceType::from_path(path).unwrap_or(SourceType::default());
    transpile(source_type, path.to_string_lossy().as_ref(), source)
}

/// Transpile with an explicit [`SourceType`].
pub fn transpile(
    source_type: SourceType,
    source_name: &str,
    source: &str,
) -> Result<Transpiled, TranspileError> {
    let allocator = Allocator::default();

    // Plain JS with no JSX / TS → no work to do.
    if !source_type.is_typescript() && !source_type.is_jsx() {
        return Ok(Transpiled { code: source.to_owned() });
    }

    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.errors.is_empty() {
        let msg = parsed
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(TranspileError::Parse(msg));
    }

    let mut program = parsed.program;

    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();

    // Use classic React.createElement runtime so transpiled JSX doesn't require
    // the `react/jsx-runtime` module (we don't have a module loader in MVP).
    // Callers can build a custom options struct for automatic runtime later.
    let mut options = TransformOptions::default();
    options.jsx = JsxOptions {
        runtime: JsxRuntime::Classic,
        ..JsxOptions::default()
    };
    let result = Transformer::new(&allocator, Path::new(source_name), &options)
        .build_with_scoping(scoping, &mut program);

    if !result.errors.is_empty() {
        let msg = result
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(TranspileError::Transform(msg));
    }

    let code = Codegen::new().build(&program).code;
    Ok(Transpiled { code })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_passthrough() {
        let out = transpile_file(Path::new("a.js"), "const x = 1;").unwrap();
        assert!(out.code.contains("const x"));
    }

    #[test]
    fn ts_strips_types() {
        let out = transpile_file(
            Path::new("a.ts"),
            "const x: number = 1; function greet(name: string): string { return 'hi ' + name; }",
        )
        .unwrap();
        // Type annotations should be gone.
        assert!(!out.code.contains(": number"));
        assert!(!out.code.contains(": string"));
        assert!(out.code.contains("greet"));
    }

    #[test]
    fn tsx_compiles() {
        let out = transpile_file(
            Path::new("a.tsx"),
            "const x: number = 1;\nconst el = <div>hi {x}</div>;",
        )
        .unwrap();
        // JSX should be lowered to a function call (React.createElement or jsx).
        assert!(!out.code.contains("<div>"), "JSX still present: {}", out.code);
    }

    #[test]
    fn parse_error_returns_err() {
        let err = transpile_file(Path::new("a.ts"), "const x: number = ;").unwrap_err();
        assert!(matches!(err, TranspileError::Parse(_)));
    }
}
