//! The stable JSON representation of a [`SourceTree`], behind `sm parse --json`.
//!
//! This is a hand-written data-transfer type rather than a `derive(Serialize)`
//! on [`crate::Node`], so that the wire format is a deliberate decision instead
//! of a side effect of internal refactors. Bump [`SCHEMA_VERSION`] on any
//! incompatible change.

use std::borrow::Cow;

use serde::Serialize;

use crate::arena::{Attachment, NodeId, SourceTree};

/// Version of the JSON schema below. Consumers should reject what they do not
/// recognise.
pub const SCHEMA_VERSION: u32 = 1;

/// A whole tree, ready to serialize.
#[derive(Debug, Serialize)]
pub struct JsonTree<'a> {
    /// See [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The language name, e.g. `"java"`.
    pub language: &'a str,
    /// Length of the source in bytes. The source itself is not included: it is
    /// on disk, and every node carries exact offsets into it.
    pub source_len: usize,
    /// Number of nodes, including anonymous tokens.
    pub node_count: usize,
    /// Whether the tree contains `ERROR` or `MISSING` nodes.
    pub has_errors: bool,
    /// ID of the root. Always `0`, but explicit beats implied.
    pub root: NodeId,
    /// Every node, in preorder — so `nodes[i].id == i`.
    pub nodes: Vec<JsonNode<'a>>,
}

/// One node.
#[derive(Debug, Serialize)]
pub struct JsonNode<'a> {
    /// Arena index, equal to this entry's position in `nodes`.
    pub id: NodeId,
    /// Grammar kind name.
    pub kind: &'a str,
    /// Grammar symbol ID for `kind`.
    pub kind_id: u16,
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
    /// Parent ID; `null` for the root.
    pub parent: Option<NodeId>,
    /// All children, named and anonymous, in source order.
    pub children: &'a [NodeId],
    /// Whether the grammar calls this a named node.
    pub named: bool,
    /// Whether this is an `extra` node (a comment).
    pub extra: bool,
    /// Whether this is an `ERROR` node.
    pub error: bool,
    /// Whether this is a `MISSING` node.
    pub missing: bool,
    /// Whether this node or a descendant is `ERROR`/`MISSING`.
    pub has_error: bool,
    /// Attached leading comments, in source order.
    pub leading_trivia: &'a [NodeId],
    /// Attached trailing comments, in source order.
    pub trailing_trivia: &'a [NodeId],
    /// Where this comment was attached: `"leading"` or `"trailing"` with an
    /// owner, or `"floating"`. Always `"floating"` for a non-comment node.
    pub attachment: Attachment,
    /// Source text, present for leaf (token) nodes only.
    ///
    /// Interior nodes are omitted because their text is the concatenation of
    /// their leaves' and the gaps between them, so including it would make the
    /// dump quadratic in tree depth. Decoded lossily — see the encoding note on
    /// [`SourceTree`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<Cow<'a, str>>,
}

impl<'a> JsonTree<'a> {
    /// Build the DTO for a tree. Borrows throughout; nothing is copied except
    /// leaf text that needed lossy decoding.
    #[must_use]
    pub fn new(tree: &'a SourceTree) -> Self {
        let nodes = tree
            .ids()
            .map(|id| {
                let node = tree.node(id);
                JsonNode {
                    id,
                    kind: node.kind,
                    kind_id: node.kind_id,
                    start: node.byte_range.start,
                    end: node.byte_range.end,
                    parent: node.parent,
                    children: &node.children,
                    named: node.is_named,
                    extra: node.is_extra,
                    error: node.is_error,
                    missing: node.is_missing,
                    has_error: node.has_error,
                    leading_trivia: &node.leading_trivia,
                    trailing_trivia: &node.trailing_trivia,
                    attachment: node.attachment,
                    text: node.is_leaf().then(|| tree.node_text(id)),
                }
            })
            .collect();

        Self {
            schema_version: SCHEMA_VERSION,
            language: tree.language_name(),
            source_len: tree.source().len(),
            node_count: tree.len(),
            has_errors: tree.has_errors(),
            root: tree.root_id(),
            nodes,
        }
    }
}
