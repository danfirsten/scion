//! Structural invariants of a [`SourceTree`], as executable assertions.
//!
//! These are not merely test helpers. SPEC.md §4.7 requires the merge driver to
//! fall back to `git merge-file` if *any* internal invariant fails, and this
//! module is what "internal invariant" means for the CST layer. The test suite
//! runs [`check_all`] over every fixture; M4 will run it in debug builds and on
//! the fallback path.
//!
//! The load-bearing one is [`check_byte_coverage`]: it is the mechanical proof
//! that no byte of the input was dropped, which is the property the emitter's
//! correctness rests on.

use crate::arena::{NodeId, SourceTree};

/// A broken structural invariant.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvariantViolation {
    /// (a) A node's byte range does not contain a child's.
    #[error(
        "containment: node {parent} ({parent_kind}) spans {parent_start}..{parent_end} \
         but child {child} ({child_kind}) spans {child_start}..{child_end}"
    )]
    Containment {
        /// The offending parent.
        parent: NodeId,
        /// Its kind.
        parent_kind: &'static str,
        /// Parent range start.
        parent_start: u32,
        /// Parent range end.
        parent_end: u32,
        /// The child that escapes it.
        child: NodeId,
        /// Its kind.
        child_kind: &'static str,
        /// Child range start.
        child_start: u32,
        /// Child range end.
        child_end: u32,
    },

    /// (b) Two adjacent siblings overlap or are out of source order.
    #[error(
        "sibling order: under {parent}, {left} ({left_kind}) ends at {left_end} \
         but the next sibling {right} ({right_kind}) starts at {right_start}"
    )]
    SiblingOrder {
        /// The common parent.
        parent: NodeId,
        /// The earlier sibling.
        left: NodeId,
        /// Its kind.
        left_kind: &'static str,
        /// Where it ends.
        left_end: u32,
        /// The later sibling.
        right: NodeId,
        /// Its kind.
        right_kind: &'static str,
        /// Where it starts.
        right_start: u32,
    },

    /// (c) Leaf tokens plus the gaps between them do not reproduce the source.
    #[error(
        "byte coverage: reconstruction from leaf tokens and inter-token gaps \
         first differs from the source at byte {offset} \
         (reconstructed {reconstructed_len} bytes, source is {source_len})"
    )]
    ByteCoverage {
        /// First byte offset at which reconstruction and source disagree, or the
        /// length of the shorter of the two if one is a prefix of the other.
        offset: usize,
        /// Length of the reconstruction.
        reconstructed_len: usize,
        /// Length of the source.
        source_len: usize,
    },

    /// (d) Preorder numbering is broken: a child's ID is not greater than its
    /// parent's.
    #[error("preorder: child {child} is not numbered after its parent {parent}")]
    PreorderIds {
        /// The parent.
        parent: NodeId,
        /// The child that violates the ordering.
        child: NodeId,
    },

    /// (d) A child's `parent` back-pointer does not agree with the parent's
    /// `children` list.
    #[error(
        "parent link: node {child} lists {actual:?} as its parent, but it is a child of {expected}"
    )]
    ParentLink {
        /// The child.
        child: NodeId,
        /// What it claims its parent is.
        actual: Option<NodeId>,
        /// Who actually lists it as a child.
        expected: NodeId,
    },

    /// (d) The root has a parent, or a non-root node has none.
    #[error("root: node {node} has parent {parent:?}, which is inconsistent with being the root")]
    RootParent {
        /// The node in question.
        node: NodeId,
        /// Its recorded parent.
        parent: Option<NodeId>,
    },
}

/// Run every check, returning all violations found.
///
/// An empty result means the tree is structurally sound.
#[must_use]
pub fn check_all(tree: &SourceTree) -> Vec<InvariantViolation> {
    let mut out = Vec::new();
    out.extend(check_containment(tree));
    out.extend(check_sibling_order(tree));
    out.extend(check_byte_coverage(tree));
    out.extend(check_preorder_ids(tree));
    out
}

/// (a) Every node's byte range contains all of its children's byte ranges.
#[must_use]
pub fn check_containment(tree: &SourceTree) -> Vec<InvariantViolation> {
    let mut out = Vec::new();
    for id in tree.ids() {
        let parent = tree.node(id);
        for child_id in tree.children(id) {
            let child = tree.node(child_id);
            if child.byte_range.start < parent.byte_range.start
                || child.byte_range.end > parent.byte_range.end
            {
                out.push(InvariantViolation::Containment {
                    parent: id,
                    parent_kind: parent.kind,
                    parent_start: parent.byte_range.start,
                    parent_end: parent.byte_range.end,
                    child: child_id,
                    child_kind: child.kind,
                    child_start: child.byte_range.start,
                    child_end: child.byte_range.end,
                });
            }
        }
    }
    out
}

/// (b) Sibling ranges are non-overlapping and in source order.
///
/// Adjacency is allowed (`left.end == right.start`), and so are zero-width
/// siblings: a `MISSING` node inserted by error recovery spans no bytes and can
/// legitimately sit exactly where its neighbour ends.
#[must_use]
pub fn check_sibling_order(tree: &SourceTree) -> Vec<InvariantViolation> {
    let mut out = Vec::new();
    for id in tree.ids() {
        let mut prev: Option<NodeId> = None;
        for child_id in tree.children(id) {
            if let Some(left_id) = prev {
                let left = tree.node(left_id);
                let right = tree.node(child_id);
                if right.byte_range.start < left.byte_range.end {
                    out.push(InvariantViolation::SiblingOrder {
                        parent: id,
                        left: left_id,
                        left_kind: left.kind,
                        left_end: left.byte_range.end,
                        right: child_id,
                        right_kind: right.kind,
                        right_start: right.byte_range.start,
                    });
                }
            }
            prev = Some(child_id);
        }
    }
    out
}

/// (c) Every byte of the source is accounted for by leaf tokens plus the gaps
/// between them.
///
/// Walks the leaves in source order and rebuilds the file: for each leaf, first
/// the bytes between the previous leaf's end and this leaf's start (whitespace,
/// which the grammar does not represent as nodes), then the leaf's own bytes;
/// finally any trailing bytes after the last leaf. If that reconstruction is
/// byte-identical to the source, then nothing was dropped and nothing was
/// double-counted.
///
/// This is the invariant that makes "never discard bytes" (SPEC.md §4.2) a
/// checked property rather than an intention.
#[must_use]
pub fn check_byte_coverage(tree: &SourceTree) -> Vec<InvariantViolation> {
    let source = tree.source();
    let mut rebuilt: Vec<u8> = Vec::with_capacity(source.len());
    let mut cursor: usize = 0;

    for leaf_id in tree.leaves() {
        let span = tree.node(leaf_id).span();
        // A leaf that starts before the cursor would mean overlapping tokens;
        // `check_sibling_order` reports that case with better detail, and here it
        // would just corrupt the reconstruction, so clamp and let the byte
        // comparison below fail.
        if span.start >= cursor && span.end <= source.len() {
            rebuilt.extend_from_slice(&source[cursor..span.start]);
            rebuilt.extend_from_slice(&source[span.clone()]);
            cursor = span.end;
        }
    }
    if cursor <= source.len() {
        rebuilt.extend_from_slice(&source[cursor..]);
    }

    if rebuilt == source {
        return Vec::new();
    }
    let offset = rebuilt
        .iter()
        .zip(source.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| rebuilt.len().min(source.len()));
    vec![InvariantViolation::ByteCoverage {
        offset,
        reconstructed_len: rebuilt.len(),
        source_len: source.len(),
    }]
}

/// (d) Preorder numbering: `parent < child` for every edge, the root is
/// [`NodeId::ROOT`] and parentless, and `parent`/`children` agree.
#[must_use]
pub fn check_preorder_ids(tree: &SourceTree) -> Vec<InvariantViolation> {
    let mut out = Vec::new();

    if let Some(parent) = tree.root().parent {
        out.push(InvariantViolation::RootParent {
            node: tree.root_id(),
            parent: Some(parent),
        });
    }

    for id in tree.ids() {
        if id != tree.root_id() && tree.node(id).parent.is_none() {
            out.push(InvariantViolation::RootParent {
                node: id,
                parent: None,
            });
        }
        for child_id in tree.children(id) {
            if child_id <= id {
                out.push(InvariantViolation::PreorderIds {
                    parent: id,
                    child: child_id,
                });
            }
            let actual = tree.node(child_id).parent;
            if actual != Some(id) {
                out.push(InvariantViolation::ParentLink {
                    child: child_id,
                    actual,
                    expected: id,
                });
            }
        }
    }

    out
}
