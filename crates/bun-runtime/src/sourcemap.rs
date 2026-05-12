//! Per-module source maps + a stack-trace remapper.
//!
//! Pipeline:
//!   user `.ts` source
//!     ─ bun-transpile (oxc) ─►  post-transpile JS (lines mostly preserved
//!                                for pure-TS strips; off for heavy JSX)
//!     ─ bun-loader::rewrite_to_iife ─►  rewritten body, plus a line_map
//!                                from rewritten line → post-transpile line
//!     ─ modules.rs IIFE wrap  ─►  final eval script
//!                                (wrapper adds 1 line of prefix at the top)
//!
//! On error we get a JSC stack like
//!   `funcName@/abs/path:LINE:COL`
//! where LINE refers to the final eval script. To map back:
//!   user_line = line_map[ LINE - 1 (wrapper) - 1 (1→0 index) ]
//! and we drop the column (we don't track column shifts).
//!
//! For unknown files (e.g. anonymous eval) we leave the frame alone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

thread_local! {
    static MAPS: RefCell<HashMap<PathBuf, ModuleMap>> = RefCell::new(HashMap::new());
}

struct ModuleMap {
    line_map: Vec<u32>,
    /// 1-indexed line-count of the original source (used to detect "synthetic line, just hide it" vs "real line").
    original_lines: u32,
}

/// Register a module's source map under its absolute path. Called from the
/// module loader after a module is prepared.
pub fn register(path: PathBuf, line_map: Vec<u32>, original_source: &str) {
    let original_lines = original_source.lines().count().max(1) as u32;
    MAPS.with(|m| {
        m.borrow_mut().insert(
            path,
            ModuleMap {
                line_map,
                original_lines,
            },
        );
    });
}

/// Rewrite a JSC stack trace so each frame points at the user's original
/// source line. Frames whose file isn't registered are left untouched.
pub fn remap_stack(stack: &str) -> String {
    let mut out = String::with_capacity(stack.len());
    for line in stack.lines() {
        if let Some(rewritten) = remap_frame(line) {
            out.push_str(&rewritten);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    // Drop trailing newline so the formatted result doesn't end in a blank line.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Try to remap a single stack frame line. Returns None if we don't
/// recognize the file or shape, in which case the caller keeps the
/// original line verbatim.
fn remap_frame(line: &str) -> Option<String> {
    // JSC stack frames usually have one of two shapes:
    //   foo@/abs/path:42:13
    //   global code@/abs/path:1:0
    //   /abs/path:42:13                   (no function name)
    // We find the LAST ":N:N" suffix and treat everything after the @ (or
    // the whole line if there's no @) as the file path.
    let (prefix, location) = match line.rfind('@') {
        Some(at) => (&line[..at + 1], &line[at + 1..]),
        None => ("", line),
    };

    // Split location on the last two colons.
    let colon_two = location.rfind(':')?;
    let colon_one = location[..colon_two].rfind(':')?;
    let file = &location[..colon_one];
    let line_str = &location[colon_one + 1..colon_two];
    let col_str = &location[colon_two + 1..];

    let line_no: u32 = line_str.parse().ok()?;
    let _: u32 = col_str.parse().ok()?;

    let path = PathBuf::from(file);
    let mapped = MAPS.with(|m| {
        let map = m.borrow();
        let entry = map.get(&path)?;
        // Wrapper added 1 line of prefix at the top.
        if line_no < 2 {
            return Some(("wrapper-prefix".to_string(), entry.original_lines));
        }
        let body_line = (line_no - 1) as usize;
        // line_map is 0-indexed; body_line is 1-indexed.
        let user_line = entry.line_map.get(body_line - 1).copied().unwrap_or(0);
        Some(("ok".to_string(), user_line))
    })?;
    let (status, user_line) = mapped;
    if status == "wrapper-prefix" {
        return None;
    }
    if user_line == 0 {
        // Synthetic line — hide it from the trace by tagging.
        return Some(format!("{prefix}{file}:<bunrs-internal>"));
    }
    Some(format!("{prefix}{file}:{user_line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_frame_no_match_is_passthrough() {
        // No registered map — should return None and the caller emits the
        // original line verbatim.
        assert!(remap_frame("foo@/nope/x.ts:5:1").is_none());
    }

    #[test]
    fn remap_frame_with_registered_map() {
        // Register a fake module whose body line 3 came from .ts line 7.
        let path = PathBuf::from("/tmp/test.ts");
        // line_map indexing: line_map[body_line - 1] = user_line
        // body lines 1..=3, with line 1 synthetic, 2 = user line 5, 3 = user line 7.
        register(path.clone(), vec![0, 5, 7], "1\n2\n3\n4\n5\n6\n7\n");
        // JSC line 4 = wrapper line 4, body line 3 (4 - 1 = 3), user line 7.
        let out = remap_frame("f@/tmp/test.ts:4:10").unwrap();
        assert_eq!(out, "f@/tmp/test.ts:7");
    }

    #[test]
    fn remap_frame_synthetic_tag() {
        let path = PathBuf::from("/tmp/test2.ts");
        register(path.clone(), vec![0, 0, 5], "1\n2\n3\n4\n5\n");
        // body line 1 → user line 0 (synthetic). Should tag rather than lie.
        let out = remap_frame("f@/tmp/test2.ts:2:0").unwrap();
        assert!(out.contains("<bunrs-internal>"));
    }
}
