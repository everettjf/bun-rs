//! Rewrites top-level `import` / `export` declarations into calls to
//! `__bun_require` and assignments on `__exports`.
//!
//! The runtime wraps the resulting source in an IIFE:
//!
//! ```js
//! (function (__exports, __bun_require, __filename, __dirname) {
//!     /* rewritten body */
//! })({}, requireFn, "/abs/path", "/abs/dir");
//! ```
//!
//! Strategy: parse with oxc, walk top-level statements, emit replacement
//! text for import/export nodes and slice the original source for everything
//! else. Output is plain JS that does not contain any ESM syntax.

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingIdentifier, Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier,
    ModuleExportName, Statement,
};
use oxc_parser::Parser;
use oxc_span::SourceType;

#[derive(Debug, thiserror::Error)]
pub enum RewriteError {
    #[error("parse error:\n{0}")]
    Parse(String),
    #[error("unsupported syntax at {span:?}: {what}")]
    Unsupported { span: (u32, u32), what: String },
}

#[derive(Debug, Clone, Default)]
pub struct ModuleAnalysis {
    /// Specifier strings this module statically depends on (from `import`,
    /// `export {} from`, `export *`). De-duplicated.
    pub imports: Vec<String>,
    /// The rewritten module body (no ESM syntax left).
    pub code: String,
}

/// Rewrite a JS source string. Caller passes JS (post-TS-transpile).
pub fn rewrite_to_iife(source: &str) -> Result<ModuleAnalysis, RewriteError> {
    let allocator = Allocator::default();
    // Use SourceType::default()-equivalent: JS module. We tell oxc this is a
    // module so import/export at top-level is accepted.
    let st = SourceType::default().with_module(true);
    let parsed = Parser::new(&allocator, source, st).parse();
    if !parsed.errors.is_empty() {
        let msg = parsed
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(RewriteError::Parse(msg));
    }

    let program = parsed.program;
    let mut out = String::with_capacity(source.len() + 256);
    let mut imports = Vec::<String>::new();
    let mut next_local: u32 = 0;
    let mut alloc_local = |imports: &mut Vec<String>, src: &str| -> String {
        // Only push to imports once per distinct specifier.
        if !imports.iter().any(|s| s == src) {
            imports.push(src.to_string());
        }
        let id = next_local;
        next_local += 1;
        format!("__m_{id}")
    };

    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(d) => {
                // `import type ... from` should have been stripped by bun-transpile
                // (it runs first). Skip defensively.
                if d.import_kind.is_type() {
                    continue;
                }
                let spec_text = &d.source.value;
                let local = alloc_local(&mut imports, spec_text);
                writeln_(&mut out, &format!(
                    "const {local} = __bun_require({}, __filename);",
                    js_string(spec_text)
                ));

                if let Some(specs) = &d.specifiers {
                    let mut parts: Vec<String> = Vec::new();
                    for s in specs {
                        match s {
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(ds) => {
                                parts.push(format!(
                                    "const {} = {local}.default;",
                                    binding_name(&ds.local)
                                ));
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => {
                                parts.push(format!(
                                    "const {} = {local};",
                                    binding_name(&ns.local)
                                ));
                            }
                            ImportDeclarationSpecifier::ImportSpecifier(is) => {
                                let imported = export_name_str(&is.imported);
                                let local_name = binding_name(&is.local);
                                if is_ident(&imported) {
                                    parts.push(format!(
                                        "const {local_name} = {local}.{imported};"
                                    ));
                                } else {
                                    parts.push(format!(
                                        "const {local_name} = {local}[{}];",
                                        js_string(&imported)
                                    ));
                                }
                            }
                        }
                    }
                    for p in parts {
                        writeln_(&mut out, &p);
                    }
                }
                // `import "./y"` → just the require call, already emitted.
            }

            Statement::ExportAllDeclaration(d) => {
                let spec_text = &d.source.value;
                let local = alloc_local(&mut imports, spec_text);
                writeln_(
                    &mut out,
                    &format!(
                        "const {local} = __bun_require({}, __filename);",
                        js_string(spec_text)
                    ),
                );
                if let Some(exported) = &d.exported {
                    // `export * as ns from "./y"` → __exports.ns = local;
                    let name = export_name_str(exported);
                    if is_ident(&name) {
                        writeln_(&mut out, &format!("__exports.{name} = {local};"));
                    } else {
                        writeln_(
                            &mut out,
                            &format!("__exports[{}] = {local};", js_string(&name)),
                        );
                    }
                } else {
                    // `export * from "./y"` → re-export every key
                    writeln_(
                        &mut out,
                        &format!(
                            "for (const __k in {local}) {{ if (__k !== 'default') __exports[__k] = {local}[__k]; }}"
                        ),
                    );
                }
            }

            Statement::ExportNamedDeclaration(d) => {
                if d.export_kind.is_type() {
                    continue;
                }
                // Three shapes:
                //  (a) `export const x = 1;` / `export function f(){}` / `export class C{}`
                //  (b) `export { a, b as c };`
                //  (c) `export { a } from "./y";`
                if let Some(decl) = &d.declaration {
                    // Shape (a): re-emit the inner declaration, then add hookups.
                    let (inner_text, names) = emit_inner_decl(source, decl);
                    out.push_str(&inner_text);
                    if !inner_text.ends_with('\n') {
                        out.push('\n');
                    }
                    for n in names {
                        if is_ident(&n) {
                            writeln_(&mut out, &format!("__exports.{n} = {n};"));
                        }
                    }
                } else if let Some(src) = &d.source {
                    // Shape (c): re-export from another module.
                    let spec_text = &src.value;
                    let local = alloc_local(&mut imports, spec_text);
                    writeln_(
                        &mut out,
                        &format!(
                            "const {local} = __bun_require({}, __filename);",
                            js_string(spec_text)
                        ),
                    );
                    for s in &d.specifiers {
                        let local_name = export_name_str(&s.local);
                        let exported_name = export_name_str(&s.exported);
                        let access = if is_ident(&local_name) {
                            format!("{local}.{local_name}")
                        } else {
                            format!("{local}[{}]", js_string(&local_name))
                        };
                        if is_ident(&exported_name) {
                            writeln_(
                                &mut out,
                                &format!("__exports.{exported_name} = {access};"),
                            );
                        } else {
                            writeln_(
                                &mut out,
                                &format!(
                                    "__exports[{}] = {access};",
                                    js_string(&exported_name)
                                ),
                            );
                        }
                    }
                } else {
                    // Shape (b): re-export local bindings.
                    for s in &d.specifiers {
                        let local_name = export_name_str(&s.local);
                        let exported_name = export_name_str(&s.exported);
                        if !is_ident(&local_name) {
                            return Err(RewriteError::Unsupported {
                                span: (s.span.start, s.span.end),
                                what: "string-literal local export name".into(),
                            });
                        }
                        if is_ident(&exported_name) {
                            writeln_(
                                &mut out,
                                &format!("__exports.{exported_name} = {local_name};"),
                            );
                        } else {
                            writeln_(
                                &mut out,
                                &format!(
                                    "__exports[{}] = {local_name};",
                                    js_string(&exported_name)
                                ),
                            );
                        }
                    }
                }
            }

            Statement::ExportDefaultDeclaration(d) => {
                match &d.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        let span = f.span;
                        let body = &source[span.start as usize..span.end as usize];
                        if let Some(id) = &f.id {
                            // Named: emit function then assign.
                            out.push_str(body);
                            out.push('\n');
                            writeln_(
                                &mut out,
                                &format!("__exports.default = {};", id.name),
                            );
                        } else {
                            // Anonymous: wrap as expression.
                            writeln_(
                                &mut out,
                                &format!("__exports.default = ({});", body),
                            );
                        }
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        let span = c.span;
                        let body = &source[span.start as usize..span.end as usize];
                        if let Some(id) = &c.id {
                            out.push_str(body);
                            out.push('\n');
                            writeln_(
                                &mut out,
                                &format!("__exports.default = {};", id.name),
                            );
                        } else {
                            writeln_(
                                &mut out,
                                &format!("__exports.default = ({});", body),
                            );
                        }
                    }
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {
                        // Types are erased by bun-transpile; ignore defensively.
                    }
                    expr => {
                        use oxc_span::GetSpan;
                        let span = expr.span();
                        let body = &source[span.start as usize..span.end as usize];
                        writeln_(
                            &mut out,
                            &format!("__exports.default = ({});", body),
                        );
                    }
                }
            }

            // Anything else: keep source as-is.
            other => {
                let span = stmt_span(other);
                out.push_str(&source[span.0 as usize..span.1 as usize]);
                out.push('\n');
            }
        }
    }

    Ok(ModuleAnalysis { imports, code: out })
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn stmt_span(s: &Statement<'_>) -> (u32, u32) {
    use oxc_span::GetSpan;
    let sp = s.span();
    (sp.start, sp.end)
}

fn binding_name<'a>(b: &BindingIdentifier<'a>) -> String {
    b.name.to_string()
}

fn export_name_str<'a>(n: &ModuleExportName<'a>) -> String {
    match n {
        ModuleExportName::IdentifierName(i) => i.name.to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first == '_' || first == '$' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn writeln_(out: &mut String, s: &str) {
    out.push_str(s);
    out.push('\n');
}

// ── Inner decl emission (for `export const|fn|class`) ───────────────────────

fn emit_inner_decl<'a>(source: &str, decl: &Declaration<'a>) -> (String, Vec<String>) {
    use oxc_ast::ast::*;
    use oxc_span::GetSpan;
    let span = decl.span();
    let text = source[span.start as usize..span.end as usize].to_string();
    let mut names = Vec::new();
    match decl {
        Declaration::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                gather_binding_names(&d.id, &mut names);
            }
        }
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                names.push(id.name.to_string());
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(id) = &c.id {
                names.push(id.name.to_string());
            }
        }
        _ => {
            // TS declarations: bun-transpile strips these; defensive no-op.
        }
    }
    (text, names)
}

fn gather_binding_names(p: &oxc_ast::ast::BindingPattern<'_>, out: &mut Vec<String>) {
    use oxc_ast::ast::BindingPattern;
    match p {
        BindingPattern::BindingIdentifier(b) => out.push(b.name.to_string()),
        BindingPattern::ObjectPattern(o) => {
            for prop in &o.properties {
                gather_binding_names(&prop.value, out);
            }
            if let Some(rest) = &o.rest {
                gather_binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(a) => {
            for elem in &a.elements {
                if let Some(e) = elem {
                    gather_binding_names(e, out);
                }
            }
            if let Some(rest) = &a.rest {
                gather_binding_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(a) => {
            gather_binding_names(&a.left, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_no_imports() {
        let r = rewrite_to_iife("const x = 1; console.log(x);").unwrap();
        assert!(r.imports.is_empty());
        assert!(r.code.contains("const x"));
    }

    #[test]
    fn rewrites_default_import() {
        let r = rewrite_to_iife(r#"import foo from "./bar"; foo();"#).unwrap();
        assert_eq!(r.imports, vec!["./bar".to_string()]);
        assert!(r.code.contains("__bun_require(\"./bar\""));
        assert!(r.code.contains("const foo ="));
        assert!(r.code.contains(".default"));
    }

    #[test]
    fn rewrites_named_imports_with_alias() {
        let r = rewrite_to_iife(r#"import { a, b as c } from "./x";"#).unwrap();
        assert!(r.code.contains("const a ="));
        assert!(r.code.contains("const c ="));
        assert!(r.code.contains(".a"));
        assert!(r.code.contains(".b"));
    }

    #[test]
    fn rewrites_namespace_import() {
        let r = rewrite_to_iife(r#"import * as ns from "./y";"#).unwrap();
        assert!(r.code.contains("const ns ="));
    }

    #[test]
    fn rewrites_side_effect_import() {
        let r = rewrite_to_iife(r#"import "./side";"#).unwrap();
        assert!(r.code.contains("__bun_require(\"./side\""));
    }

    #[test]
    fn rewrites_export_const() {
        let r = rewrite_to_iife("export const x = 42;").unwrap();
        assert!(r.code.contains("const x = 42"));
        assert!(r.code.contains("__exports.x = x"));
    }

    #[test]
    fn rewrites_export_named_specifiers() {
        let r = rewrite_to_iife("const a = 1; const b = 2; export { a, b as c };").unwrap();
        assert!(r.code.contains("__exports.a = a"));
        assert!(r.code.contains("__exports.c = b"));
    }

    #[test]
    fn rewrites_export_default_expr() {
        let r = rewrite_to_iife("export default 1 + 2;").unwrap();
        assert!(r.code.contains("__exports.default = (1 + 2)"));
    }

    #[test]
    fn rewrites_export_default_function() {
        let r = rewrite_to_iife("export default function foo() { return 1; }").unwrap();
        assert!(r.code.contains("function foo()"));
        assert!(r.code.contains("__exports.default = foo"));
    }

    #[test]
    fn rewrites_export_star_renamed() {
        let r = rewrite_to_iife(r#"export * as ns from "./y";"#).unwrap();
        assert!(r.code.contains("__exports.ns ="));
    }

    #[test]
    fn rewrites_export_star() {
        let r = rewrite_to_iife(r#"export * from "./y";"#).unwrap();
        assert!(r.code.contains("for (const __k in"));
    }

    #[test]
    fn rewrites_export_from() {
        let r = rewrite_to_iife(r#"export { a, b as c } from "./y";"#).unwrap();
        assert!(r.code.contains("__exports.a ="));
        assert!(r.code.contains("__exports.c ="));
    }
}
