//! Recover the source-declaration order of `table` and `enum` blocks from a
//! `.fbs` text.
//!
//! The reflection schema flatc emits sorts `Schema.objects()` and
//! `Schema.enums()` alphabetically by qualified name (binary-search
//! requirement). The committed `*_flatbed.rs` snapshots were generated in
//! source order, so byte-identical regeneration requires recovering that
//! order from the original `.fbs` text. This module does just that — no
//! type parsing, no field parsing — solely positions of top-level
//! declarations.

/// Walk a `.fbs` text and yield the bare names of `table` and `enum`
/// declarations in source order.
///
/// Comments (`//` line, `/* … */` block, including single-line block forms)
/// are stripped before scanning so a declaration commented out cannot
/// influence ordering.
pub(crate) fn source_decl_order(text: &str) -> Vec<String> {
    let cleaned = strip_fbs_comments(text);
    let bytes = cleaned.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        for (kw, kw_len) in [("table", 5usize), ("enum", 4usize)] {
            if at_word_boundary(bytes, i, kw.as_bytes()) {
                let mut j = i + kw_len;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > start {
                    out.push(std::str::from_utf8(&bytes[start..j]).unwrap().to_string());
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn at_word_boundary(bytes: &[u8], i: usize, kw: &[u8]) -> bool {
    if i + kw.len() > bytes.len() {
        return false;
    }
    if &bytes[i..i + kw.len()] != kw {
        return false;
    }
    if i > 0 {
        let prev = bytes[i - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return false;
        }
    }
    if i + kw.len() < bytes.len() {
        let next = bytes[i + kw.len()];
        if next.is_ascii_alphanumeric() || next == b'_' {
            return false;
        }
    }
    true
}

/// Strip `//` line comments and `/* … */` block comments from a `.fbs`
/// text, preserving newlines so byte offsets and line numbers stay roughly
/// stable for downstream diagnostics. String literals are left alone (a
/// `"//"` inside a string is not a comment).
fn strip_fbs_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    out.push(b'\n');
                }
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            } else {
                i = bytes.len();
            }
            continue;
        }
        if bytes[i] == b'"' {
            out.push(b'"');
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    out.push(bytes[i]);
                    out.push(bytes[i + 1]);
                    i += 2;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            if i < bytes.len() {
                out.push(b'"');
                i += 1;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("strip_fbs_comments preserves UTF-8 boundaries")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_decl_order_basic() {
        let schema = r#"
namespace test;
table First { x: int; }
enum E : byte { A, B }
table Second { y: int; }
"#;
        assert_eq!(
            source_decl_order(schema),
            vec!["First", "E", "Second"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_source_decl_order_skips_block_commented() {
        // A `table`/`enum` keyword inside a block comment must not influence
        // ordering. Without comment stripping, the codegen would think the
        // commented decl exists and try to look it up in the reflection
        // schema — producing a misleading position for the real decl.
        let schema = r#"
namespace test;
/*
table InMultilineComment { x: int; }
enum InMultilineComment : byte { A, B }
*/
/* table InSingleLine { y: int; } */
table RealTable { z: int; }
enum RealEnum : byte { Ok, Bad }
"#;
        assert_eq!(
            source_decl_order(schema),
            vec!["RealTable", "RealEnum"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_source_decl_order_skips_line_comments() {
        // `//` line comment containing the `table` keyword must not be
        // picked up. The reflection-driven flow already filters
        // non-existent decls, but the helper should still be robust.
        let schema = r#"
// table NotReal { x: int; }
table Real { y: int; }
"#;
        assert_eq!(source_decl_order(schema), vec!["Real".to_string()]);
    }

    #[test]
    fn test_strip_fbs_comments_preserves_string_literals() {
        // `//` and `/* */` inside a string literal are content, not
        // comments. Stripping them would corrupt schema attributes.
        let schema = r#"@meta("// not a comment");"#;
        let stripped = strip_fbs_comments(schema);
        assert!(stripped.contains("// not a comment"));
    }
}
