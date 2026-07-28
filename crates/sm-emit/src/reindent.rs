//! The reindentation transform (SPEC.md §4.6).
//!
//! # What it is allowed to do
//!
//! Exactly one thing: replace the **leading whitespace run of a line** with a
//! different leading whitespace run. It never touches a byte that is not at the
//! start of a line, it never touches the first line of a span (that line's
//! indentation was supplied by the gap in front of it), and it never inserts or
//! removes a line break. Everything else about the emitted bytes is a verbatim
//! copy, which is what SPEC.md §0.5 requires.
//!
//! # When it fires
//!
//! A [`Shift`] is derived at the point of splicing: `from` is the leading
//! whitespace of the line the node starts on **in its own revision**, `to` is
//! the leading whitespace of the line the output is currently on. They differ
//! exactly when the node's nesting depth changed — which is the
//! wrapped-in-an-`if` case, and the only case SPEC.md §4.6 asks about.
//!
//! Deriving the shift from the two *observed* indentations rather than from a
//! guessed "indent unit" is what makes it safe with tabs, with spaces, with
//! four-space files, with two-space files, and with files that mix. There is no
//! detection heuristic to be wrong.
//!
//! Note that it is the **line's** indentation on both sides, not the node's
//! column. A method body's `{` sits at the end of `void a() {` and begins no
//! line of its own; its statements are nonetheless indented relative to that
//! line, and requiring the node to start a line would decline to reindent
//! precisely the case this exists for. See [`line_base_indent`].
//!
//! # When it declines
//!
//! - **A line's indentation does not begin with `from`.** The line is shallower
//!   than the node's own base indentation — a continuation line aligned to an
//!   opening paren, say — and rewriting it would be guessing. Left alone.
//! - **A line starts inside a multi-line leaf token that is not a comment.**
//!   Java text blocks, template literals, and any other construct whose interior
//!   whitespace is part of the value. See [`protected_ranges`].
//! - **A line is empty.** Indenting a blank line only produces trailing
//!   whitespace.
//!
//! Block comments are deliberately *not* protected: whitespace inside a comment
//! is not part of any value, and a Javadoc block that moved two levels deeper
//! and kept its old indentation looks broken.

use std::ops::Range;

use sm_cst::{Language, SourceTree};
use sm_merge::layout;

/// A leading-whitespace rewrite: replace `from` with `to` at the start of every
/// eligible line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Shift {
    pub from: Vec<u8>,
    pub to: Vec<u8>,
}

/// Byte ranges whose interior lines must never be reindented.
///
/// A leaf token that spans a newline and is not a comment: Java's
/// `multiline_string_fragment` inside a text block, TypeScript's
/// `template_string` chunks, and anything a future grammar does the same way.
/// The rule is structural rather than a list of kind names, so it holds for
/// languages this crate has never heard of.
///
/// Returned sorted by start, which preorder gives for free.
#[must_use]
pub(crate) fn protected_ranges(tree: &SourceTree, lang: &dyn Language) -> Vec<Range<u32>> {
    let source = tree.source();
    tree.ids()
        .filter(|&id| {
            let node = tree.node(id);
            node.is_leaf() && !layout::is_trivia(tree, lang, id)
        })
        .map(|id| tree.node(id).byte_range.clone())
        .filter(|r| source[r.start as usize..r.end as usize].contains(&b'\n'))
        .collect()
}

/// Whether `offset` lies strictly inside a protected range.
#[must_use]
pub(crate) fn is_protected(ranges: &[Range<u32>], offset: u32) -> bool {
    let at = ranges.partition_point(|r| r.start <= offset);
    at > 0 && offset < ranges[at - 1].end
}

/// The start of the line `offset` sits on.
fn line_start(source: &[u8], offset: usize) -> usize {
    source[..offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1)
}

/// The whitespace between the start of `offset`'s line and `offset`, or `None`
/// if anything else precedes `offset` on that line.
///
/// Used where "does this node begin a line" is the actual question — backing a
/// conflict block up to its own indentation.
#[must_use]
pub(crate) fn line_indent(source: &[u8], offset: usize) -> Option<&[u8]> {
    let start = line_start(source, offset);
    let indent = &source[start..offset];
    indent
        .iter()
        .all(|b| matches!(b, b' ' | b'\t'))
        .then_some(indent)
}

/// The leading whitespace run of the line `offset` sits on, whatever else is on
/// that line.
///
/// This, not [`line_indent`], is what the shift is derived from. A block whose
/// `{` sits at the end of `if (ok) {` does not begin a line, but its *lines*
/// are still indented relative to the line it opens on, and that is the
/// quantity a reindent has to preserve. Requiring the node to begin a line
/// would decline to reindent exactly the wrapped-block case SPEC.md §4.6 names.
#[must_use]
pub(crate) fn line_base_indent(source: &[u8], offset: usize) -> &[u8] {
    let start = line_start(source, offset);
    let end = source[start..offset.max(start)]
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t'))
        .map_or(offset, |i| start + i);
    &source[start..end]
}

/// The indentation of the line the output is currently on.
#[must_use]
pub(crate) fn output_indent(out: &[u8]) -> &[u8] {
    line_base_indent(out, out.len())
}

/// Apply `shift` to `bytes`, which start at `base_offset` in `source`.
///
/// The first line is never touched. Returns the number of lines rewritten.
pub(crate) fn apply(
    out: &mut Vec<u8>,
    bytes: &[u8],
    base_offset: u32,
    protected: &[Range<u32>],
    shift: &Shift,
) -> usize {
    let mut rewritten = 0usize;
    let mut i = 0usize;
    let mut at_line_start = false;
    while i < bytes.len() {
        if at_line_start {
            at_line_start = false;
            let absolute = base_offset + i as u32;
            let blank =
                bytes[i] == b'\n' || (bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n'));
            if !blank && !is_protected(protected, absolute) && bytes[i..].starts_with(&shift.from) {
                out.extend_from_slice(&shift.to);
                i += shift.from.len();
                rewritten += 1;
                continue;
            }
        }
        out.push(bytes[i]);
        at_line_start = bytes[i] == b'\n';
        i += 1;
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::{Shift, apply, is_protected, line_indent, output_indent};

    fn shifted(text: &str, from: &str, to: &str) -> String {
        let mut out = Vec::new();
        apply(
            &mut out,
            text.as_bytes(),
            0,
            &[],
            &Shift {
                from: from.as_bytes().to_vec(),
                to: to.as_bytes().to_vec(),
            },
        );
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn the_first_line_is_never_touched() {
        assert_eq!(shifted("a();\n  b();", "  ", "      "), "a();\n      b();");
    }

    #[test]
    fn deeper_lines_keep_their_relative_indent() {
        assert_eq!(
            shifted("if (x) {\n  a();\n}", "", "    "),
            "if (x) {\n      a();\n    }"
        );
    }

    #[test]
    fn a_line_shallower_than_the_shift_is_left_alone() {
        assert_eq!(shifted("a();\n b();\n", "  ", "\t"), "a();\n b();\n");
    }

    #[test]
    fn blank_lines_do_not_acquire_trailing_whitespace() {
        assert_eq!(shifted("a();\n\n  b();", "  ", "    "), "a();\n\n    b();");
        // The CRLF-only line is skipped; the following line is indented.
        assert_eq!(shifted("a();\n\r\n  b();", "", "  "), "a();\n\r\n    b();");
    }

    #[test]
    fn crlf_survives_because_only_leading_whitespace_is_rewritten() {
        assert_eq!(
            shifted("a();\r\n  b();\r\n", "  ", "    "),
            "a();\r\n    b();\r\n"
        );
    }

    #[test]
    fn protected_ranges_are_skipped() {
        let text = "x(\"\"\"\n  a\n  b\"\"\");\n  y();";
        // Protect the text-block interior, offsets 5..14.
        let mut out = Vec::new();
        apply(
            &mut out,
            text.as_bytes(),
            0,
            &[Range { start: 4, end: 14 }],
            &Shift {
                from: "  ".as_bytes().to_vec(),
                to: "      ".as_bytes().to_vec(),
            },
        );
        let got = String::from_utf8(out).expect("utf8");
        assert!(
            got.contains("\n  a\n  b"),
            "text block interior moved: {got:?}"
        );
        assert!(
            got.ends_with("\n      y();"),
            "code line not shifted: {got:?}"
        );
    }

    #[test]
    fn line_indent_reports_none_when_the_line_has_code_before_the_offset() {
        assert_eq!(line_indent(b"  a b", 5), None);
        assert_eq!(line_indent(b"  ab", 2), Some(&b"  "[..]));
        assert_eq!(line_indent(b"x\n\tab", 3), Some(&b"\t"[..]));
    }

    #[test]
    fn output_indent_tracks_the_last_line() {
        assert_eq!(output_indent(b""), &b""[..]);
        assert_eq!(output_indent(b"a\n   "), &b"   "[..]);
        // Mid-line: the *line's* indent, not the column, which is what a
        // block opened at the end of `if (ok) {` needs.
        assert_eq!(output_indent(b"a\n   b"), &b"   "[..]);
        assert_eq!(output_indent(b"  if (ok) "), &b"  "[..]);
    }

    #[test]
    fn line_base_indent_ignores_code_before_the_offset() {
        assert_eq!(super::line_base_indent(b"    if (x) {", 11), &b"    "[..]);
        assert_eq!(super::line_base_indent(b"a\n\tb();", 4), &b"\t"[..]);
        assert_eq!(super::line_base_indent(b"", 0), &b""[..]);
    }

    #[test]
    fn protection_is_strict_containment() {
        let ranges = [Range {
            start: 10,
            end: 20u32,
        }];
        assert!(!is_protected(&ranges, 9));
        assert!(is_protected(&ranges, 10));
        assert!(is_protected(&ranges, 19));
        assert!(!is_protected(&ranges, 20));
    }
}
