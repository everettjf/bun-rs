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
    ImportExpression, MetaProperty, ModuleExportName, Statement,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

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
    /// For each 1-indexed line of `code`, the originating line in the
    /// input source (1-indexed; 0 = synthetic).
    pub line_map: Vec<u32>,
}

/// Rewrite a JS source string. Caller passes JS (post-TS-transpile).
pub fn rewrite_to_iife(source: &str) -> Result<ModuleAnalysis, RewriteError> {
    // Two passes:
    // 1. Replace dynamic `import(spec)` with `__bun_require(spec, __filename)`
    //    at every nested location (handled by `lower_dynamic_imports`).
    // 2. Rewrite top-level static `import`/`export` into `await __bun_require`
    //    + `__exports.X = …` (the original logic, below).
    let source = lower_dynamic_imports(source)?;
    rewrite_static(&source)
}

/// Pass 1 — rewrite every `import(...)` expression in the source so it calls
/// `__bun_require(spec, __filename)` instead. Static `import`/`export`
/// statements are left for the next pass.
fn lower_dynamic_imports(source: &str) -> Result<String, RewriteError> {
    let allocator = Allocator::default();
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

    enum Spot {
        DynamicImport {
            start: u32,
            end: u32,
            src_start: u32,
            src_end: u32,
        },
        ImportMeta {
            start: u32,
            end: u32,
        },
    }
    struct Collect {
        spots: Vec<Spot>,
    }
    impl<'a> Visit<'a> for Collect {
        fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
            let sp = it.span;
            let src_sp = it.source.span();
            self.spots.push(Spot::DynamicImport {
                start: sp.start,
                end: sp.end,
                src_start: src_sp.start,
                src_end: src_sp.end,
            });
        }

        fn visit_meta_property(&mut self, it: &MetaProperty<'a>) {
            // `import.meta` — meta="import", property="meta"
            if it.meta.name == "import" && it.property.name == "meta" {
                self.spots.push(Spot::ImportMeta {
                    start: it.span.start,
                    end: it.span.end,
                });
            }
        }
    }
    let mut c = Collect { spots: Vec::new() };
    c.visit_program(&parsed.program);

    if c.spots.is_empty() {
        return Ok(source.to_string());
    }

    // Apply end → start so earlier spans remain valid.
    let mut out = source.to_string();
    let mut spots = c.spots;
    spots.sort_by_key(|s| std::cmp::Reverse(match s {
        Spot::DynamicImport { start, .. } => *start,
        Spot::ImportMeta { start, .. } => *start,
    }));
    for s in spots {
        match s {
            Spot::DynamicImport {
                start,
                end,
                src_start,
                src_end,
            } => {
                let arg_text = &source[src_start as usize..src_end as usize];
                let replacement = format!("__bun_require({arg_text}, __filename)");
                out.replace_range(start as usize..end as usize, &replacement);
            }
            Spot::ImportMeta { start, end } => {
                out.replace_range(start as usize..end as usize, "__bun_meta");
            }
        }
    }
    Ok(out)
}

fn rewrite_static(source: &str) -> Result<ModuleAnalysis, RewriteError> {
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
    let byte_to_line = build_byte_to_line(source);
    let mut emit = Emit::new(source.len() + 256);
    let mut imports = Vec::<String>::new();
    let mut next_local: u32 = 0;
    let mut alloc_local = |imports: &mut Vec<String>, src: &str| -> String {
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
                if d.import_kind.is_type() {
                    continue;
                }
                let spec_text = &d.source.value;
                let local = alloc_local(&mut imports, spec_text);
                emit.synth(&format!(
                    "const {local} = await __bun_require({}, __filename);",
                    js_string(spec_text)
                ));

                if let Some(specs) = &d.specifiers {
                    for s in specs {
                        match s {
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(ds) => {
                                emit.synth(&format!(
                                    "const {} = {local}.default;",
                                    binding_name(&ds.local)
                                ));
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => {
                                emit.synth(&format!(
                                    "const {} = {local};",
                                    binding_name(&ns.local)
                                ));
                            }
                            ImportDeclarationSpecifier::ImportSpecifier(is) => {
                                let imported = export_name_str(&is.imported);
                                let local_name = binding_name(&is.local);
                                if is_ident(&imported) {
                                    emit.synth(&format!(
                                        "const {local_name} = {local}.{imported};"
                                    ));
                                } else {
                                    emit.synth(&format!(
                                        "const {local_name} = {local}[{}];",
                                        js_string(&imported)
                                    ));
                                }
                            }
                        }
                    }
                }
            }

            Statement::ExportAllDeclaration(d) => {
                let spec_text = &d.source.value;
                let local = alloc_local(&mut imports, spec_text);
                emit.synth(&format!(
                    "const {local} = await __bun_require({}, __filename);",
                    js_string(spec_text)
                ));
                if let Some(exported) = &d.exported {
                    let name = export_name_str(exported);
                    if is_ident(&name) {
                        emit.synth(&format!("__exports.{name} = {local};"));
                    } else {
                        emit.synth(&format!(
                            "__exports[{}] = {local};",
                            js_string(&name)
                        ));
                    }
                } else {
                    emit.synth(&format!(
                        "for (const __k in {local}) {{ if (__k !== 'default') __exports[__k] = {local}[__k]; }}"
                    ));
                }
            }

            Statement::ExportNamedDeclaration(d) => {
                if d.export_kind.is_type() {
                    continue;
                }
                if let Some(decl) = &d.declaration {
                    // Shape (a): re-emit the inner declaration FROM SOURCE
                    // (preserves the user's line numbers), then synthetic
                    // hookups.
                    use oxc_span::GetSpan;
                    let sp = decl.span();
                    let start_line = byte_to_line[sp.start as usize];
                    let text = &source[sp.start as usize..sp.end as usize];
                    let names = decl_names(decl);
                    emit.slice(text, start_line);
                    for n in names {
                        if is_ident(&n) {
                            emit.synth(&format!("__exports.{n} = {n};"));
                        }
                    }
                } else if let Some(src) = &d.source {
                    let spec_text = &src.value;
                    let local = alloc_local(&mut imports, spec_text);
                    emit.synth(&format!(
                        "const {local} = await __bun_require({}, __filename);",
                        js_string(spec_text)
                    ));
                    for s in &d.specifiers {
                        let local_name = export_name_str(&s.local);
                        let exported_name = export_name_str(&s.exported);
                        let access = if is_ident(&local_name) {
                            format!("{local}.{local_name}")
                        } else {
                            format!("{local}[{}]", js_string(&local_name))
                        };
                        if is_ident(&exported_name) {
                            emit.synth(&format!(
                                "__exports.{exported_name} = {access};"
                            ));
                        } else {
                            emit.synth(&format!(
                                "__exports[{}] = {access};",
                                js_string(&exported_name)
                            ));
                        }
                    }
                } else {
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
                            emit.synth(&format!(
                                "__exports.{exported_name} = {local_name};"
                            ));
                        } else {
                            emit.synth(&format!(
                                "__exports[{}] = {local_name};",
                                js_string(&exported_name)
                            ));
                        }
                    }
                }
            }

            Statement::ExportDefaultDeclaration(d) => {
                use oxc_span::GetSpan;
                match &d.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        let span = f.span;
                        let body = &source[span.start as usize..span.end as usize];
                        let start_line = byte_to_line[span.start as usize];
                        if let Some(id) = &f.id {
                            emit.slice(body, start_line);
                            emit.synth(&format!("__exports.default = {};", id.name));
                        } else {
                            // Wrap anon fn as expression — single synthetic line.
                            emit.synth(&format!("__exports.default = ({});", body));
                        }
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        let span = c.span;
                        let body = &source[span.start as usize..span.end as usize];
                        let start_line = byte_to_line[span.start as usize];
                        if let Some(id) = &c.id {
                            emit.slice(body, start_line);
                            emit.synth(&format!("__exports.default = {};", id.name));
                        } else {
                            emit.synth(&format!("__exports.default = ({});", body));
                        }
                    }
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {}
                    expr => {
                        let span = expr.span();
                        let body = &source[span.start as usize..span.end as usize];
                        emit.synth(&format!("__exports.default = ({});", body));
                    }
                }
            }

            // Anything else: keep source as-is, preserving its lines.
            other => {
                let span = stmt_span(other);
                let start_line = byte_to_line[span.0 as usize];
                emit.slice(
                    &source[span.0 as usize..span.1 as usize],
                    start_line,
                );
            }
        }
    }

    let (code, line_map) = emit.finish();
    Ok(ModuleAnalysis { imports, code, line_map })
}

/// Extract the names a declaration binds. Used for shape (a) of
/// `export <decl>` so we can wire up `__exports.X = X` afterwards.
fn decl_names<'a>(decl: &oxc_ast::ast::Declaration<'a>) -> Vec<String> {
    use oxc_ast::ast::*;
    let mut out = Vec::new();
    match decl {
        Declaration::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                gather_binding_names(&d.id, &mut out);
            }
        }
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                out.push(id.name.to_string());
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(id) = &c.id {
                out.push(id.name.to_string());
            }
        }
        _ => {}
    }
    out
}

/// Emitter that tracks per-output-line correspondence to input lines.
struct Emit {
    out: String,
    /// 1-indexed input line per emitted output line.
    line_map: Vec<u32>,
}

impl Emit {
    fn new(cap: usize) -> Self {
        Self {
            out: String::with_capacity(cap),
            line_map: Vec::new(),
        }
    }

    /// Emit a synthetic chunk. Each output line it produces maps to "no
    /// input line" (0).
    fn synth(&mut self, text: &str) {
        self.write_chunk(text, 0, false);
    }

    /// Emit text taken verbatim from the input. The first output line
    /// corresponds to `start_input_line`; subsequent lines (split by `\n`
    /// in `text`) map to `start_input_line + 1`, +2, ...
    fn slice(&mut self, text: &str, start_input_line: u32) {
        self.write_chunk(text, start_input_line, true);
    }

    fn write_chunk(&mut self, text: &str, base_line: u32, advance: bool) {
        let mut cur = base_line;
        for c in text.chars() {
            self.out.push(c);
            if c == '\n' {
                self.line_map.push(cur);
                if advance && cur != 0 {
                    cur += 1;
                }
            }
        }
        // Ensure the chunk ends on a newline so subsequent emit calls start
        // their own output line. This also keeps the line_map aligned.
        if !self.out.ends_with('\n') {
            self.out.push('\n');
            self.line_map.push(cur);
        }
    }

    fn finish(self) -> (String, Vec<u32>) {
        (self.out, self.line_map)
    }
}

/// `byte_to_line[i]` is the 1-indexed line number of byte `i` in `source`.
fn build_byte_to_line(source: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(source.len() + 1);
    let mut line: u32 = 1;
    for b in source.bytes() {
        out.push(line);
        if b == b'\n' {
            line += 1;
        }
    }
    out.push(line);
    out
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
        assert!(r.code.contains("await __bun_require(\"./bar\""));
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
        assert!(r.code.contains("await __bun_require(\"./side\""));
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
    fn line_map_preserves_user_lines_for_passthrough() {
        // 4 lines of input. The middle one is a non-import statement,
        // the others are an import (synthetic) and an export const (mixed).
        let src = r#"import { x } from "./y";
const a = 1;
const b = 2;
export const c = 3;
"#;
        let r = rewrite_to_iife(src).unwrap();
        // For every output line, line_map says which input line it came from.
        // For `const a = 1;` on input line 2 we expect at least one output
        // line mapped to 2.
        assert!(r.line_map.contains(&2), "line_map: {:?}", r.line_map);
        assert!(r.line_map.contains(&3), "line_map: {:?}", r.line_map);
        // Synthetic lines (the require call) should be 0.
        assert!(r.line_map.contains(&0), "line_map: {:?}", r.line_map);
        // No mapping should claim a line beyond the input file.
        assert!(r.line_map.iter().all(|&l| l <= 4), "line_map: {:?}", r.line_map);
    }

    #[test]
    fn rewrites_export_from() {
        let r = rewrite_to_iife(r#"export { a, b as c } from "./y";"#).unwrap();
        assert!(r.code.contains("__exports.a ="));
        assert!(r.code.contains("__exports.c ="));
    }
}
