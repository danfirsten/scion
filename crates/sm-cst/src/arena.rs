//! The arena: [`SourceTree`], [`Node`], [`NodeId`].

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Index of a [`Node`] within a [`SourceTree`]'s arena.
///
/// IDs are assigned in **preorder** during [`crate::parse`], which gives two
/// properties the rest of the pipeline relies on:
///
/// 1. An ID is stable within its own tree (SPEC.md §4.2) — it does not depend on
///    pointer addresses or on tree-sitter's internal cursor state, so it can be
///    stored in matchings and edit scripts and serialized.
/// 2. `parent < child` for every edge, so a plain ascending scan of the arena is
///    a valid topological order (parents before children).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The root of any [`SourceTree`]. Preorder numbering puts it first.
    pub const ROOT: NodeId = NodeId(0);

    /// The arena index, for use as a `Vec` subscript.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a comment node has been attached by the trivia attachment pass.
///
/// **Populated by the trivia attachment pass** (not implemented in M0). Every
/// comment node parses with [`Attachment::Floating`], which is also the correct
/// terminal value for a comment that belongs to no particular sibling — a
/// section banner in the middle of a class body, say. Non-comment nodes always
/// carry [`Attachment::Floating`] and it is meaningless for them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "owner", rename_all = "snake_case")]
pub enum Attachment {
    /// This comment introduces the given node; it appears in that node's
    /// [`Node::leading_trivia`].
    Leading(NodeId),
    /// This comment trails the given node; it appears in that node's
    /// [`Node::trailing_trivia`].
    Trailing(NodeId),
    /// This comment belongs to no sibling and stays where it is in the parent's
    /// child list. Also the pre-attachment default for every node.
    #[default]
    Floating,
}

/// One node of the concrete syntax tree.
///
/// Both named nodes and anonymous token nodes (`{`, `;`, `public`) are present:
/// the emitter splices bytes, so it needs the punctuation. Use
/// [`SourceTree::named_children`] when only the named ones matter.
///
/// `Node` deliberately does not derive `Serialize`. The one authoritative
/// on-disk schema lives in [`crate::JsonTree`], so that changing a field here
/// does not silently change the format other tools consume.
#[derive(Clone, Debug)]
pub struct Node {
    /// The tree-sitter symbol ID for this node's kind. Cheap to compare; resolve
    /// it back to a name via `Language::ts_language().node_kind_for_id`.
    pub kind_id: u16,
    /// The grammar's name for this node's kind, e.g. `method_declaration`.
    ///
    /// This is `&'static str` because tree-sitter's kind names live in the
    /// statically linked grammar tables, so the "interning" is free and no side
    /// table is needed.
    pub kind: &'static str,
    /// Half-open byte range into [`SourceTree::source`].
    ///
    /// `u32` rather than `usize`: it halves the arena's memory and merge driver
    /// latency is user-visible (SPEC.md §3). Sources larger than `u32::MAX` are
    /// rejected by [`crate::parse`] rather than silently truncated.
    pub byte_range: Range<u32>,
    /// `None` only for the root.
    pub parent: Option<NodeId>,
    /// All children in source order — named children *and* anonymous tokens
    /// *and* `extra` nodes such as comments.
    pub children: Vec<NodeId>,
    /// Whether the grammar considers this a named node (as opposed to an
    /// anonymous token like `;`).
    pub is_named: bool,
    /// Whether this node is an `extra` — a node the grammar allows anywhere,
    /// which in practice means comments.
    pub is_extra: bool,
    /// Whether this node *is* an `ERROR` node.
    pub is_error: bool,
    /// Whether this node is a `MISSING` node inserted by error recovery. Such
    /// nodes have an empty byte range.
    pub is_missing: bool,
    /// Whether this node or any descendant is an `ERROR` or `MISSING` node.
    pub has_error: bool,
    /// Comments that introduce this node, in source order.
    ///
    /// **Populated by the trivia attachment pass** (not implemented in M0);
    /// always empty until then. Each ID refers to a comment node that also still
    /// occupies its structural position in some ancestor's [`Node::children`] —
    /// attachment annotates the tree, it never removes nodes from it, because
    /// removing them would break the byte-coverage invariant.
    pub leading_trivia: Vec<NodeId>,
    /// Comments that trail this node, in source order. **Populated by the trivia
    /// attachment pass**; see [`Node::leading_trivia`].
    pub trailing_trivia: Vec<NodeId>,
    /// For comment nodes, where this comment was attached.
    ///
    /// **Populated by the trivia attachment pass**; [`Attachment::Floating`]
    /// until then. This is the inverse of [`Node::leading_trivia`] /
    /// [`Node::trailing_trivia`] and the two must be kept consistent.
    pub attachment: Attachment,
}

impl Node {
    /// The byte range as `usize`, ready to subscript the source buffer.
    #[must_use]
    pub fn span(&self) -> Range<usize> {
        self.byte_range.start as usize..self.byte_range.end as usize
    }

    /// Number of bytes this node spans.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.byte_range.end - self.byte_range.start
    }

    /// Whether this node spans zero bytes. True for `MISSING` nodes and for the
    /// root of an empty file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this node has no children, i.e. it is a token.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// A parsed source file: the original bytes plus a preorder arena of nodes.
///
/// # Why the source is stored as bytes
///
/// `Vec<u8>`, not `String`. Three reasons, in order of weight:
///
/// 1. tree-sitter reports **byte** offsets, and the emitter's whole job is to
///    splice byte ranges out of the input (SPEC.md §4.6). Byte ranges into a
///    `String` would have to be re-validated as char boundaries on every splice;
///    byte ranges into a `Vec<u8>` are exact by construction.
/// 2. The JLS does not require Java source to be UTF-8. Files in real
///    repositories are occasionally Latin-1 or Shift-JIS. Requiring UTF-8 would
///    mean *refusing to merge* those files, and a merge driver that refuses is
///    strictly worse than one that treats the bytes as opaque.
/// 3. A merge driver must never lose data. Lossy decoding at the front door
///    would silently rewrite bytes the user never touched.
///
/// The cost is that any *display* of source text has to go through
/// [`String::from_utf8_lossy`], which can render `U+FFFD` for non-UTF-8 input.
/// That is confined to the renderer and the JSON dump — never to the merge path.
#[derive(Clone, Debug)]
pub struct SourceTree {
    source: Vec<u8>,
    nodes: Vec<Node>,
    language_name: &'static str,
    has_errors: bool,
}

impl SourceTree {
    pub(crate) fn new(source: Vec<u8>, nodes: Vec<Node>, language_name: &'static str) -> Self {
        let has_errors = nodes.first().is_some_and(|root| root.has_error);
        Self {
            source,
            nodes,
            language_name,
            has_errors,
        }
    }

    /// The complete original source, byte for byte.
    #[must_use]
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// The name of the [`crate::Language`] this tree was parsed with.
    #[must_use]
    pub fn language_name(&self) -> &'static str {
        self.language_name
    }

    /// Total number of nodes in the arena, including anonymous tokens.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Always `false`: even an empty file parses to a single root node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The root node's ID. Always [`NodeId::ROOT`].
    #[must_use]
    pub fn root_id(&self) -> NodeId {
        NodeId::ROOT
    }

    /// The root node.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.nodes[NodeId::ROOT.index()]
    }

    /// Whether the tree contains any `ERROR` or `MISSING` node.
    ///
    /// This is *not* a parse failure — tree-sitter is error-tolerant and the
    /// tree is still a faithful, byte-complete description of the file. It is
    /// the signal the merge driver (M4) uses to decide whether to fall back to
    /// `git merge-file` (SPEC.md §4.7), and the reason `sm parse` prints a
    /// warning.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.has_errors
    }

    /// Every node, in preorder.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &Node> {
        self.nodes.iter()
    }

    /// Every node ID, in preorder.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = NodeId> {
        (0..self.nodes.len()).map(|i| NodeId(i as u32))
    }

    /// Look up a node. Panics if `id` did not come from this tree.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    /// Record that `comment` introduces `owner`.
    ///
    /// This is the write half of the trivia seam: the attachment *pass* decides
    /// which comment goes where, and calls this to record the decision. Keeping
    /// the mutation behind a method is what guarantees the forward index
    /// ([`Node::leading_trivia`]) and the back-pointer ([`Node::attachment`])
    /// cannot drift apart.
    ///
    /// The comment keeps its structural position in its parent's
    /// [`Node::children`] — attachment annotates the tree, it never removes
    /// nodes from it, because removing them would break byte coverage.
    ///
    /// Attaching a comment that is already attached moves it: the previous
    /// owner's list is updated first.
    pub fn attach_leading(&mut self, owner: NodeId, comment: NodeId) {
        self.detach(comment);
        self.nodes[owner.index()].leading_trivia.push(comment);
        self.nodes[comment.index()].attachment = Attachment::Leading(owner);
    }

    /// Record that `comment` trails `owner`. See [`SourceTree::attach_leading`].
    pub fn attach_trailing(&mut self, owner: NodeId, comment: NodeId) {
        self.detach(comment);
        self.nodes[owner.index()].trailing_trivia.push(comment);
        self.nodes[comment.index()].attachment = Attachment::Trailing(owner);
    }

    /// Undo any attachment of `comment`, returning it to
    /// [`Attachment::Floating`]. A no-op if it was already floating.
    pub fn detach(&mut self, comment: NodeId) {
        let previous = std::mem::replace(
            &mut self.nodes[comment.index()].attachment,
            Attachment::Floating,
        );
        match previous {
            Attachment::Leading(owner) => {
                self.nodes[owner.index()]
                    .leading_trivia
                    .retain(|&c| c != comment);
            }
            Attachment::Trailing(owner) => {
                self.nodes[owner.index()]
                    .trailing_trivia
                    .retain(|&c| c != comment);
            }
            Attachment::Floating => {}
        }
    }

    /// The raw bytes this node spans.
    #[must_use]
    pub fn node_bytes(&self, id: NodeId) -> &[u8] {
        &self.source[self.node(id).span()]
    }

    /// The text this node spans, lossily decoded as UTF-8.
    ///
    /// For display only — see the encoding note on [`SourceTree`]. The merge
    /// path must use [`SourceTree::node_bytes`].
    #[must_use]
    pub fn node_text(&self, id: NodeId) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.node_bytes(id))
    }

    /// This node's children, named and anonymous alike, in source order.
    pub fn children(&self, id: NodeId) -> impl ExactSizeIterator<Item = NodeId> + '_ {
        self.node(id).children.iter().copied()
    }

    /// This node's *named* children only, in source order.
    ///
    /// This is the view the matcher (M2) works on: anonymous tokens carry no
    /// information a structural match can use, since their text is fully
    /// determined by the parent's kind.
    pub fn named_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id)
            .children
            .iter()
            .copied()
            .filter(move |&c| self.node(c).is_named)
    }

    /// Every descendant of `id` in preorder, `id` itself excluded.
    ///
    /// Preorder numbering makes this a contiguous ID range: a node's descendants
    /// are exactly the IDs between it and the next node that is not one of its
    /// descendants. This walks the child lists rather than exploiting that, to
    /// stay correct if the arena ever holds more than one tree.
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack: Vec<NodeId> = self.node(id).children.iter().rev().copied().collect();
        std::iter::from_fn(move || {
            let next = stack.pop()?;
            stack.extend(self.node(next).children.iter().rev().copied());
            Some(next)
        })
    }

    /// Every leaf (token) node in source order.
    ///
    /// The concatenation of these nodes' bytes, interleaved with the gap bytes
    /// between them, reconstructs the source exactly — see
    /// [`crate::invariants::check_byte_coverage`].
    pub fn leaves(&self) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::once(self.root_id())
            .chain(self.descendants(self.root_id()))
            .filter(move |&id| self.node(id).is_leaf())
    }

    /// Walk from `id` to the root, `id` excluded.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.node(id).parent;
        std::iter::from_fn(move || {
            let next = cur?;
            cur = self.node(next).parent;
            Some(next)
        })
    }
}
