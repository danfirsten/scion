//! The output model: [`MergedTree`], [`MergedNode`], [`Conflict`] and the
//! reporting types that hang off them.
//!
//! # The provenance contract (SPEC.md §4.6)
//!
//! Every leaf of a [`MergedTree`] carries a **side plus a `NodeId`**, which is a
//! byte range in one of the three inputs. Nothing in this crate ever produces
//! text; the only bytes the pipeline can invent are [`Gap::Synthesized`], which
//! exists precisely so that "did we synthesise anything here?" is a question the
//! test suite can answer mechanically rather than by inspection.
//!
//! # Why a `leads` side table instead of a field on the node
//!
//! Where the whitespace *between* two emitted siblings comes from is a merge
//! decision, not an emitter one: only the merge knows whether an element sits in
//! a slot the container's own revision already had (take that revision's gap) or
//! was carried in from the other side (take the gap it had over there). Putting
//! it in a side table keeps [`MergedNode`] exactly the shape SPEC.md §4.6
//! describes while still recording the decision. See the crate docs, "Whitespace
//! ownership".

use std::ops::Range;

use serde::{Deserialize, Serialize};
use sm_cst::NodeId;

/// Which of the three input revisions a byte range belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The common ancestor.
    Base,
    /// The revision being merged *into* — `%A` in git's merge-driver contract.
    Ours,
    /// The revision being merged *in* — `%B`.
    Theirs,
}

impl Side {
    /// A short stable name, used in conflict-marker labels and in debug output.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Ours => "ours",
            Self::Theirs => "theirs",
        }
    }

    /// All three sides, in the canonical order used by every report.
    pub const ALL: [Self; 3] = [Self::Base, Self::Ours, Self::Theirs];
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Index of a [`MergedNode`] in a [`MergedTree`]'s arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MergedId(pub u32);

impl MergedId {
    /// The arena index, for use as a `Vec` subscript.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for MergedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The bytes that precede an emitted node — the inter-sibling gap.
///
/// A gap holds whitespace, and any *floating* comment that the trivia pass did
/// not attach to either neighbour. It is byte-copied like everything else.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    /// Copy `range` verbatim out of `side`'s source. An empty range means "no
    /// gap", which is the common case for the first child of a container.
    Copied {
        /// Which revision the bytes come from.
        side: Side,
        /// Half-open byte range into that revision's source.
        range: Range<u32>,
    },
    /// Bytes that appear in none of the three inputs.
    ///
    /// The merge emits this only when an element has to be placed into a
    /// container that had no element to copy a gap from on any side — see the
    /// crate docs, "Whitespace ownership". SPEC.md §5's byte-preservation
    /// property is stated in terms of this variant: everything *not* in a
    /// `Synthesized` gap (or a conflict marker) is traceable to an input range.
    Synthesized(Vec<u8>),
}

impl Gap {
    /// The empty gap, copied from `side` (so that even "nothing" has a
    /// provenance).
    #[must_use]
    pub const fn none(side: Side) -> Self {
        Self::Copied { side, range: 0..0 }
    }

    /// Whether this gap contributes no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Copied { range, .. } => range.start >= range.end,
            Self::Synthesized(bytes) => bytes.is_empty(),
        }
    }
}

/// One node of the merged tree.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergedNode {
    /// Take this subtree verbatim from one revision.
    ///
    /// The emitted bytes are the node's *extent*: its own byte range widened to
    /// cover the comments the trivia pass attached to it. That is what makes a
    /// comment ride along with the declaration it documents when the
    /// declaration moves.
    Splice {
        /// Which revision to copy from.
        side: Side,
        /// The node in that revision.
        node: NodeId,
    },
    /// Keep this container from one revision, but recompose its child list.
    ///
    /// The container's own tokens (`{`, `}`, `(`, `,`, operators) are part of
    /// `children` — they are ordinary [`MergedNode::Splice`] nodes over the
    /// anonymous token nodes. Only the container's *leading and trailing
    /// trivia*, which lie outside its byte range, come from `side` implicitly.
    Rebuilt {
        /// Which revision owns this container's frame.
        side: Side,
        /// The container node in that revision.
        node: NodeId,
        /// The merged child list, in emission order.
        children: Vec<MergedId>,
    },
    /// An unresolved three-way disagreement. The emitter renders markers.
    Conflict(Conflict),
}

/// A three-way disagreement, in terms of input nodes rather than text.
///
/// Text markers are the emitter's business (SPEC.md §4.5). Each side is a *run*
/// of sibling nodes, because a conflict frequently covers several statements at
/// once; an empty run means that side contributes nothing (a deletion).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Conflict {
    /// The ancestor's nodes, when there is a common ancestor for this region.
    /// `None` means the region is an insertion on both sides.
    pub base: Option<Vec<NodeId>>,
    /// Our side's nodes for this region, in source order.
    pub ours: Vec<NodeId>,
    /// Their side's nodes for this region, in source order.
    pub theirs: Vec<NodeId>,
    /// Why the two sides could not be reconciled.
    pub reason: ConflictReason,
}

/// Why a region conflicted.
///
/// Deliberately fine-grained: M5's report histograms these, and a coarse enum
/// would make the "which class of conflict should we attack next" question
/// unanswerable. `ours`/`theirs` in the names are in that order, so
/// [`ConflictReason::DeleteUpdate`] is *we* deleted and *they* updated.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// Both sides changed the same node's content, differently.
    UpdateUpdate,
    /// Both sides moved the same node, to different parents.
    MoveMove,
    /// We deleted a node the other side changed.
    DeleteUpdate,
    /// We changed a node the other side deleted.
    UpdateDelete,
    /// We deleted a node the other side moved somewhere else.
    DeleteMove,
    /// We moved a node the other side deleted.
    MoveDelete,
    /// Both sides inserted different content into the same set-like container.
    InsertInsert,
    /// Both sides inserted different content at the same anchor of an *ordered*
    /// list. We do not guess an interleaving (SPEC.md §4.5).
    OrderedInsertCollision,
    /// Both sides permuted the same ordered region, differently.
    ReorderConflict,
    /// Both sides changed the same node's anonymous tokens differently —
    /// `x += 1` versus `x -= 1`. Splicing either side's frame would silently
    /// pick an operator.
    KindClash,
    /// Both sides edited the comments attached to the same node, differently.
    CommentEdit,
    /// The three roots could not be paired, so there is no anchor to merge
    /// against. The whole file becomes one conflict.
    RootMismatch,
    /// Anything the table above does not name. `detail` is a short, stable,
    /// machine-groupable string.
    Unmergeable {
        /// What went wrong.
        detail: String,
    },
}

impl ConflictReason {
    /// A short stable identifier, for histograms and snapshot tests.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::UpdateUpdate => "update_update",
            Self::MoveMove => "move_move",
            Self::DeleteUpdate => "delete_update",
            Self::UpdateDelete => "update_delete",
            Self::DeleteMove => "delete_move",
            Self::MoveDelete => "move_delete",
            Self::InsertInsert => "insert_insert",
            Self::OrderedInsertCollision => "ordered_insert_collision",
            Self::ReorderConflict => "reorder_conflict",
            Self::KindClash => "kind_clash",
            Self::CommentEdit => "comment_edit",
            Self::RootMismatch => "root_mismatch",
            Self::Unmergeable { .. } => "unmergeable",
        }
    }

    /// The mirror-image reason, i.e. the one the same scenario produces when
    /// `ours` and `theirs` are swapped.
    ///
    /// SPEC.md §5's symmetry property is "the *decision* must not differ"; the
    /// labels legitimately do. This is what the symmetry tests compare against.
    #[must_use]
    pub fn mirrored(&self) -> Self {
        match self {
            Self::DeleteUpdate => Self::UpdateDelete,
            Self::UpdateDelete => Self::DeleteUpdate,
            Self::DeleteMove => Self::MoveDelete,
            Self::MoveDelete => Self::DeleteMove,
            other => other.clone(),
        }
    }
}

impl std::fmt::Display for ConflictReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unmergeable { detail } => write!(f, "unmergeable({detail})"),
            other => f.write_str(other.tag()),
        }
    }
}

/// The merged tree: an arena of [`MergedNode`] plus the inter-sibling gaps.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MergedTree {
    nodes: Vec<MergedNode>,
    leads: Vec<Gap>,
    root: MergedId,
}

impl MergedTree {
    pub(crate) fn from_parts(nodes: Vec<MergedNode>, leads: Vec<Gap>, root: MergedId) -> Self {
        debug_assert_eq!(nodes.len(), leads.len());
        Self { nodes, leads, root }
    }

    /// The root of the merged tree.
    #[must_use]
    pub const fn root(&self) -> MergedId {
        self.root
    }

    /// Look up a node. Panics if `id` did not come from this tree.
    #[must_use]
    pub fn node(&self, id: MergedId) -> &MergedNode {
        &self.nodes[id.index()]
    }

    /// The gap emitted immediately *before* `id`.
    #[must_use]
    pub fn lead(&self, id: MergedId) -> &Gap {
        &self.leads[id.index()]
    }

    /// Total arena size, including nodes that were superseded by conflict
    /// promotion and are no longer reachable from the root.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Always false — a merged tree always has a root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// This node's children, or an empty slice for a leaf.
    #[must_use]
    pub fn children(&self, id: MergedId) -> &[MergedId] {
        match &self.nodes[id.index()] {
            MergedNode::Rebuilt { children, .. } => children,
            _ => &[],
        }
    }

    /// The `(side, node)` this merged node's bytes come from, if it is a single
    /// input node. `None` for conflicts.
    ///
    /// M6 re-resolves names over the merged tree; this is how it gets from a
    /// merged node back to a scope in one of the three input trees.
    #[must_use]
    pub fn provenance(&self, id: MergedId) -> Option<(Side, NodeId)> {
        match self.nodes[id.index()] {
            MergedNode::Splice { side, node } | MergedNode::Rebuilt { side, node, .. } => {
                Some((side, node))
            }
            MergedNode::Conflict(_) => None,
        }
    }

    /// Every node reachable from the root, in preorder.
    pub fn walk(&self) -> impl Iterator<Item = MergedId> + '_ {
        let mut stack = vec![self.root];
        std::iter::from_fn(move || {
            let next = stack.pop()?;
            stack.extend(self.children(next).iter().rev().copied());
            Some(next)
        })
    }
}

/// One entry of [`MergeOutcome::conflicts`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ConflictSummary {
    /// Where the conflict sits in the merged tree.
    pub id: MergedId,
    /// Why it conflicted.
    pub reason: ConflictReason,
    /// The kind of the smallest enclosing input node, for grouping in reports.
    pub kind: String,
    /// The region in the ancestor, if any.
    pub base: Option<Range<u32>>,
    /// The region in our revision, if any.
    pub ours: Option<Range<u32>>,
    /// The region in their revision, if any.
    pub theirs: Option<Range<u32>>,
}

/// Per-side counts of [`crate::Fate`], for reporting.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FateCounts {
    /// Nodes neither side touched.
    pub unchanged: usize,
    /// Nodes whose content changed in place.
    pub updated: usize,
    /// Nodes that changed position but not content.
    pub moved: usize,
    /// Nodes that changed both.
    pub moved_and_updated: usize,
    /// Nodes with no counterpart on this side.
    pub deleted: usize,
}

/// Counters for the report (SPEC.md §6.3) and for regression tests.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MergeStats {
    /// Arena sizes of the three inputs.
    pub base_nodes: usize,
    /// See [`MergeStats::base_nodes`].
    pub ours_nodes: usize,
    /// See [`MergeStats::base_nodes`].
    pub theirs_nodes: usize,
    /// Reachable nodes in the merged tree.
    pub merged_nodes: usize,
    /// Reachable [`MergedNode::Splice`] nodes.
    pub splices: usize,
    /// Reachable [`MergedNode::Rebuilt`] nodes.
    pub rebuilds: usize,
    /// Reachable [`MergedNode::Conflict`] nodes.
    pub conflicts: usize,
    /// Fates of base nodes on our side.
    pub ours_fates: FateCounts,
    /// Fates of base nodes on their side.
    pub theirs_fates: FateCounts,
    /// Base nodes emitted under a different parent than they had in base.
    pub reparented: usize,
    /// Conflicting regions that a commutative group resolved by set union.
    pub set_merged_regions: usize,
    /// Insertions present on both sides that were emitted once.
    pub deduplicated_insertions: usize,
    /// Ancestor elements dropped from a container's child list because they
    /// were emitted at a move destination instead.
    ///
    /// Every one of these is a delete/modify conflict that did *not* happen:
    /// the element left the container rather than being deleted, so the other
    /// side's changes to it are applied where it landed (docs/prior-art.md
    /// §2.5).
    pub covered_deletions: usize,
}

/// Everything [`crate::merge`] produces.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MergeOutcome {
    /// The merged tree, ready for `sm-emit`.
    pub tree: MergedTree,
    /// Every reachable conflict, in preorder.
    pub conflicts: Vec<ConflictSummary>,
    /// Counters.
    pub stats: MergeStats,
    /// Fate of every base node on our side, indexed by [`NodeId`].
    pub ours_fates: Vec<crate::Fate>,
    /// Fate of every base node on their side, indexed by [`NodeId`].
    pub theirs_fates: Vec<crate::Fate>,
}

impl MergeOutcome {
    /// Whether the merge produced no conflicts at all.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}
