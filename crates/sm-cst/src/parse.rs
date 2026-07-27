//! The parse entry point: tree-sitter tree in, [`SourceTree`] arena out.

use crate::arena::{Attachment, Node, NodeId, SourceTree};
use crate::language::Language;

/// A *hard* parse failure.
///
/// Note what is deliberately not in here: a source file with syntax errors.
/// tree-sitter is error-tolerant, and a tree containing `ERROR` or `MISSING`
/// nodes is still a complete, byte-faithful description of the file — so
/// [`parse`] returns `Ok` for it and the caller checks
/// [`SourceTree::has_errors`]. The merge driver (M4) uses that flag to fall back
/// to `git merge-file`; `sm parse` uses it to print a warning. Reserving `Err`
/// for genuine failures keeps the two situations from being confused.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// tree-sitter rejected the grammar — an ABI mismatch between the
    /// `tree-sitter` runtime and the compiled-in grammar crate.
    #[error("could not initialise the {language} parser (tree-sitter ABI mismatch?)")]
    LanguageInit {
        /// The language whose grammar was rejected.
        language: &'static str,
        /// The underlying tree-sitter error.
        #[source]
        source: tree_sitter::LanguageError,
    },

    /// The source is too large for the `u32` byte ranges the arena stores.
    #[error("source is {len} bytes; the maximum supported size is {max} bytes")]
    SourceTooLarge {
        /// Actual length of the input.
        len: usize,
        /// The largest input [`parse`] accepts.
        max: usize,
    },

    /// tree-sitter returned no tree at all. With no timeout and no cancellation
    /// flag set this should be unreachable, but the API is fallible and
    /// swallowing that would be a lie.
    #[error("the {language} parser produced no tree")]
    NoTree {
        /// The language that was being parsed.
        language: &'static str,
    },
}

/// The largest source [`parse`] will accept.
///
/// [`crate::Node::byte_range`] is a `Range<u32>`, so an offset must fit in a
/// `u32`. Anything this size is not hand-written Java in any case, and the merge
/// driver has a size budget of its own (SPEC.md §4.7).
pub const MAX_SOURCE_LEN: usize = u32::MAX as usize;

/// Parse `source` and build the arena.
///
/// IDs are assigned in preorder, so [`NodeId::ROOT`] is the root and every
/// node's ID is smaller than all of its descendants'. Anonymous tokens and
/// `extra` nodes (comments) are kept alongside named children: the emitter
/// splices bytes and needs the punctuation, and discarding comments would
/// violate the never-lose-bytes rule (SPEC.md §4.2).
///
/// # Errors
///
/// See [`ParseError`]. A file with syntax errors is *not* an error here — check
/// [`SourceTree::has_errors`] on the returned tree.
pub fn parse(source: &[u8], lang: &dyn Language) -> Result<SourceTree, ParseError> {
    if source.len() > MAX_SOURCE_LEN {
        return Err(ParseError::SourceTooLarge {
            len: source.len(),
            max: MAX_SOURCE_LEN,
        });
    }

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.ts_language())
        .map_err(|source| ParseError::LanguageInit {
            language: lang.name(),
            source,
        })?;

    let tree = parser.parse(source, None).ok_or(ParseError::NoTree {
        language: lang.name(),
    })?;

    let nodes = build_arena(&tree);
    Ok(SourceTree::new(source.to_vec(), nodes, lang.name()))
}

/// Flatten a tree-sitter tree into a preorder arena.
///
/// Iterative rather than recursive on purpose: a chain of binary expressions or
/// nested parentheses in generated Java can nest thousands deep, and a recursive
/// walk would blow the stack on input a merge driver is expected to survive.
fn build_arena<'t>(tree: &'t tree_sitter::Tree) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    // (tree-sitter node, arena id of its parent). Popped LIFO; children are
    // pushed in reverse so they pop in source order, which makes the arena
    // order exactly preorder.
    let mut stack: Vec<(tree_sitter::Node<'t>, Option<NodeId>)> = vec![(tree.root_node(), None)];
    let mut child_buf: Vec<tree_sitter::Node<'t>> = Vec::new();

    while let Some((ts_node, parent)) = stack.pop() {
        let id = NodeId(nodes.len() as u32);
        nodes.push(Node {
            kind_id: ts_node.kind_id(),
            kind: ts_node.kind(),
            byte_range: ts_node.start_byte() as u32..ts_node.end_byte() as u32,
            parent,
            children: Vec::new(),
            is_named: ts_node.is_named(),
            is_extra: ts_node.is_extra(),
            is_error: ts_node.is_error(),
            is_missing: ts_node.is_missing(),
            has_error: ts_node.has_error(),
            // Populated by the trivia attachment pass; see `Node::leading_trivia`.
            leading_trivia: Vec::new(),
            trailing_trivia: Vec::new(),
            attachment: Attachment::Floating,
        });
        if let Some(parent) = parent {
            nodes[parent.index()].children.push(id);
        }

        child_buf.clear();
        let mut cursor = ts_node.walk();
        child_buf.extend(ts_node.children(&mut cursor));
        for child in child_buf.drain(..).rev() {
            stack.push((child, Some(id)));
        }
    }

    nodes
}
