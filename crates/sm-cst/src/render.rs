//! The textual tree renderer behind `sm parse`.
//!
//! This lives in `sm-cst` rather than in `sm-cli` so that the snapshot test can
//! assert on exactly the bytes the CLI prints without shelling out to a binary.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use crate::arena::{Attachment, NodeId, SourceTree};
use crate::language::Language;

/// The optional first line(s) of `sm parse` output.
#[derive(Clone, Debug)]
pub struct HeaderInfo<'a> {
    /// Path to display. The renderer does no filesystem access of its own.
    pub path: &'a str,
    /// How long parsing took.
    ///
    /// `None` omits the timing field, which is what the snapshot test uses —
    /// wall-clock timings are not reproducible and have no business in a
    /// committed snapshot.
    pub parse_time: Option<Duration>,
}

/// Knobs for [`render_parse`].
#[derive(Clone, Debug)]
pub struct RenderOptions<'a> {
    /// Render comments.
    ///
    /// When false (`sm parse --no-trivia`), comment nodes are omitted from the
    /// output entirely and no trivia blocks are printed. This hides bytes, so it
    /// is a display option only — it has no equivalent anywhere in the merge
    /// path.
    pub show_trivia: bool,
    /// Maximum number of characters of escaped source text to show per node
    /// before truncating with `...`.
    pub max_text_len: usize,
    /// Header to print above the tree, if any.
    pub header: Option<HeaderInfo<'a>>,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            show_trivia: true,
            max_text_len: 40,
            header: None,
        }
    }
}

/// The warning `sm parse` prints when the tree contains `ERROR`/`MISSING` nodes.
pub const ERROR_WARNING: &str = "!! WARNING: this file did not parse cleanly (ERROR/MISSING nodes present). \
     The merge driver will fall back to git's line merge for it.";

/// Render a parsed tree as indented text.
///
/// Each line is `kind [start..end]`, followed by flags for nodes that are
/// `extra`/`ERROR`/`MISSING`, followed by the source text for nodes whose text
/// carries meaning ([`Language::significant_text`]) and for comments.
///
/// # Trivia
///
/// The renderer already handles attached trivia, even though nothing attaches
/// any yet: a comment listed in some node's `leading_trivia` or
/// `trailing_trivia` is printed *under that node* as a `leading:` / `trailing:`
/// line and suppressed at its structural position, so it appears exactly once.
/// Until the trivia attachment pass lands, every comment is
/// [`Attachment::Floating`] and therefore renders at its structural position,
/// which is the honest description of what the tree currently knows.
#[must_use]
pub fn render_parse(tree: &SourceTree, lang: &dyn Language, opts: &RenderOptions<'_>) -> String {
    let mut out = String::with_capacity(tree.len() * 48);

    if let Some(header) = &opts.header {
        let _ = write!(
            out,
            "{}  language={}  nodes={}",
            header.path,
            tree.language_name(),
            tree.len()
        );
        if let Some(elapsed) = header.parse_time {
            let _ = write!(out, "  parse={:.3}ms", elapsed.as_secs_f64() * 1000.0);
        }
        out.push('\n');
        if tree.has_errors() {
            out.push_str(ERROR_WARNING);
            out.push('\n');
        }
    }

    // Comments that some node has claimed. Empty until the trivia attachment
    // pass runs; the suppression logic below is inert until then.
    let attached: HashSet<NodeId> = if opts.show_trivia {
        tree.ids()
            .flat_map(|id| {
                let n = tree.node(id);
                n.leading_trivia.iter().chain(n.trailing_trivia.iter())
            })
            .copied()
            .collect()
    } else {
        HashSet::new()
    };

    // Iterative walk: `Trailing` frames are pushed before a node's children so
    // they pop after the whole subtree has been rendered.
    enum Frame {
        Node(NodeId, usize),
        Trailing(NodeId, usize),
    }
    let mut stack = vec![Frame::Node(tree.root_id(), 0)];

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Trailing(id, depth) => {
                for &t in &tree.node(id).trailing_trivia {
                    write_trivia_line(&mut out, tree, lang, opts, t, depth, "trailing");
                }
            }
            Frame::Node(id, depth) => {
                let node = tree.node(id);
                let is_comment = lang.is_comment(node.kind);

                if is_comment && !opts.show_trivia {
                    continue;
                }
                if is_comment && attached.contains(&id) {
                    // Rendered under its owner instead.
                    continue;
                }

                write_indent(&mut out, depth);
                write_node_line(&mut out, tree, lang, opts, id);

                if opts.show_trivia {
                    for &t in &node.leading_trivia {
                        write_trivia_line(&mut out, tree, lang, opts, t, depth + 1, "leading");
                    }
                    if !node.trailing_trivia.is_empty() {
                        stack.push(Frame::Trailing(id, depth + 1));
                    }
                }

                for &child in node.children.iter().rev() {
                    stack.push(Frame::Node(child, depth + 1));
                }
            }
        }
    }

    out
}

fn write_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_trivia_line(
    out: &mut String,
    tree: &SourceTree,
    lang: &dyn Language,
    opts: &RenderOptions<'_>,
    id: NodeId,
    depth: usize,
    label: &str,
) {
    write_indent(out, depth);
    out.push_str(label);
    out.push_str(": ");
    write_node_line(out, tree, lang, opts, id);
}

fn write_node_line(
    out: &mut String,
    tree: &SourceTree,
    lang: &dyn Language,
    opts: &RenderOptions<'_>,
    id: NodeId,
) {
    let node = tree.node(id);
    let _ = write!(
        out,
        "{} [{}..{}]",
        node.kind, node.byte_range.start, node.byte_range.end
    );

    if node.is_error {
        out.push_str(" ERROR");
    }
    if node.is_missing {
        out.push_str(" MISSING");
    }
    if node.is_extra {
        out.push_str(" extra");
    }
    match node.attachment {
        Attachment::Leading(owner) => {
            let _ = write!(out, " attached=leading({owner})");
        }
        Attachment::Trailing(owner) => {
            let _ = write!(out, " attached=trailing({owner})");
        }
        Attachment::Floating => {}
    }

    if lang.significant_text(node.kind) || lang.is_comment(node.kind) {
        let text = escape(&tree.node_text(id), opts.max_text_len);
        let _ = write!(out, " {text}");
    }

    out.push('\n');
}

/// Render source text as a double-quoted, escaped, length-capped literal.
///
/// Escaping matters more than it looks: comments and string literals contain
/// newlines, and a tree dump whose lines can themselves contain newlines is
/// unreadable and unsnapshottable.
fn escape(text: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut truncated = false;
    for (count, ch) in text.chars().enumerate() {
        if count >= max_len {
            truncated = true;
            break;
        }
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    if truncated {
        out.push_str("...");
    }
    out
}
