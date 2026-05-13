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

    #[test]
    fn mjs_passthrough() {
        // .mjs is plain JS — no transpile.
        let src = "import x from './y.js';\nexport const z = x + 1;";
        let out = transpile_file(Path::new("a.mjs"), src).unwrap();
        assert_eq!(out.code, src);
    }

    #[test]
    fn cjs_passthrough() {
        let src = "const fs = require('fs'); module.exports = { fs };";
        let out = transpile_file(Path::new("a.cjs"), src).unwrap();
        assert_eq!(out.code, src);
    }

    #[test]
    fn ts_interface_and_type_aliases_are_erased() {
        let src = "interface Point { x: number; y: number }\n\
                   type Vec = [number, number];\n\
                   const p: Point = { x: 1, y: 2 };";
        let out = transpile_file(Path::new("a.ts"), src).unwrap();
        // interface / type declarations vanish completely.
        assert!(!out.code.contains("interface"));
        assert!(!out.code.contains("type Vec"));
        // The runtime value survives.
        assert!(out.code.contains("p"));
        assert!(out.code.contains("x:"));
    }

    #[test]
    fn ts_enum_compiles_to_runtime_value() {
        let src = "enum Color { Red, Green, Blue }\nconst c = Color.Green;";
        let out = transpile_file(Path::new("a.ts"), src).unwrap();
        // Enums become a runtime object — `Color` should still appear.
        assert!(out.code.contains("Color"));
        // No leftover `enum` keyword.
        assert!(!out.code.contains("enum"));
    }

    #[test]
    fn ts_class_with_access_modifiers() {
        let src = "class C {\n\
                     constructor(public name: string, private age: number) {}\n\
                     greet(): string { return 'hi ' + this.name; }\n\
                   }";
        let out = transpile_file(Path::new("a.ts"), src).unwrap();
        // public/private modifiers stripped, fields assigned in ctor.
        assert!(!out.code.contains("public name"));
        assert!(!out.code.contains("private age"));
        assert!(out.code.contains("this.name"));
    }

    #[test]
    fn ts_optional_chaining_and_nullish_kept() {
        // These are JS features, not TS — codegen must preserve them.
        let src = "const v = obj?.foo ?? 'fallback';";
        let out = transpile_file(Path::new("a.ts"), src).unwrap();
        assert!(out.code.contains("?.") || out.code.contains("?."));
        assert!(out.code.contains("??") || out.code.contains("\"fallback\"") || out.code.contains("'fallback'"));
    }

    #[test]
    fn tsx_with_react_fragment() {
        let src = "const el = <><span>a</span><span>b</span></>;";
        let out = transpile_file(Path::new("a.tsx"), src).unwrap();
        assert!(!out.code.contains("<>"));
        // Classic runtime: should reference React (createElement / Fragment).
        assert!(
            out.code.contains("React") || out.code.contains("Fragment"),
            "expected React or Fragment in output, got: {}",
            out.code
        );
    }

    #[test]
    fn jsx_file_lowers_without_ts_strip() {
        // .jsx is JSX but not TS — JSX should still be lowered.
        let src = "const el = <div className=\"x\">hi</div>;";
        let out = transpile_file(Path::new("a.jsx"), src).unwrap();
        assert!(!out.code.contains("<div"));
    }

    #[test]
    fn empty_file_returns_empty_or_minimal() {
        let out = transpile_file(Path::new("a.ts"), "").unwrap();
        assert!(out.code.trim().is_empty() || out.code.len() < 5);
    }
}
