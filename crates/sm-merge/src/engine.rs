//! The recursive three-way merge itself.
//!
//! The shape of the algorithm, in one paragraph: start at the three roots; at
//! every matched triple decide whether a whole side can be taken verbatim (the
//! common case, and the only way SPEC.md §4.6's byte-identity invariant can
//! hold); if not, align the three child lists — *anonymous tokens included* —
//! with a diff3 chunker keyed on base-node identity, and recurse into the
//! elements that survive. Nodes that changed container are emitted once, at
//! their destination, with their content merged there.
//!
//! The design record is the crate-level documentation. This file implements it.

use std::collections::HashMap;
use std::ops::Range;

use sm_cst::{ChildListKind, Language, NodeId, SourceTree};
use sm_match::{Matching, structurally_equal};

use crate::config::MergeConfig;
use crate::fate;
use crate::model::{
    Conflict, ConflictReason, ConflictSummary, Gap, MergeOutcome, MergeStats, MergedId, MergedNode,
    MergedTree, Side,
};
use crate::seq::{Hunk, HunkKind, diff3};
use crate::tables::{Item, SideTables};

/// "There is a conflict below here that an enclosing boundary should absorb."
type Promo = Option<ConflictReason>;

/// The identity of a child-list entry, for sequence alignment.
///
/// Identity is *base-node identity*, not text and not position (SPEC.md §4.5):
/// an edited statement still aligns with the statement it came from, and two
/// textually identical statements are not confused for each other.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    /// An element with a counterpart in the ancestor.
    Base(u32),
    /// An anonymous token, identified by grammar kind. Repeats are expected —
    /// a comma is a comma — and the aligner handles that the way any text diff
    /// handles repeated lines.
    Token(u16),
    /// An element with no counterpart in the ancestor. The side is part of the
    /// key so that two independent insertions never collide by accident; the
    /// dedup pass rewrites one to the other when they really are the same.
    New(u8, u32),
    /// An atomic set element, keyed by its own source text.
    Text(u64),
}

/// What precedes an item in a child list.
///
/// The point of naming this is the whitespace rule's validity condition: a
/// leading gap was *measured* against some predecessor, and it is only reusable
/// where that same predecessor is still there. See [`ListCtx::lead_for`].
#[derive(Clone, Copy, Debug)]
enum Neighbour {
    /// Nothing precedes it: the item sits directly after the container's head.
    Start,
    /// The identity of the item in front of it.
    Item(Key),
    /// A conflict region, or an item that is not in this list's aligned
    /// sequence. Never equal to anything, including itself.
    Opaque,
}

impl Neighbour {
    /// Deliberately not `PartialEq`: `Opaque` must not equal `Opaque`.
    fn same_as(self, other: Self) -> bool {
        match (self, other) {
            (Self::Start, Self::Start) => true,
            (Self::Item(a), Self::Item(b)) => a == b,
            _ => false,
        }
    }
}

/// What a leading gap's bytes look like, precomputed so the repair rule can ask
/// without holding a borrow of the source.
#[derive(Clone, Copy, Debug)]
struct GapShape {
    /// No bytes at all — the case that can fuse two tokens together.
    empty: bool,
    /// Non-empty and **whitespace only**: it separates two tokens and carries
    /// nothing else. Only such a gap may be substituted for another, because a
    /// gap that holds a floating comment would duplicate that comment.
    separating: bool,
    /// Contains a newline. Used only to prefer the least disruptive repair.
    newline: bool,
    len: u32,
    /// Hash of the bytes. A "repair" that would copy the same bytes out of a
    /// different revision changes nothing about the output and is not made, so
    /// that the framing side keeps its provenance wherever the layout agrees.
    hash: u64,
}

/// Where a base node will be emitted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Home {
    /// Under the container matched to this base node.
    Base(NodeId),
    /// Under a container that exists on one side only.
    Local(Side, NodeId),
}

/// How much one side changed a node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    /// Byte for byte identical, comments and whitespace included.
    None,
    /// Same code and same comments, different bytes. Reindentation.
    Formatting,
    /// Something a reader would notice.
    Content,
}

const TAG_BASE: u8 = 0;
const TAG_OURS: u8 = 1;
const TAG_THEIRS: u8 = 2;

const fn tag_of(side: Side) -> u8 {
    match side {
        Side::Base => TAG_BASE,
        Side::Ours => TAG_OURS,
        Side::Theirs => TAG_THEIRS,
    }
}

const fn side_of(tag: u8) -> Side {
    match tag {
        TAG_BASE => Side::Base,
        TAG_OURS => Side::Ours,
        _ => Side::Theirs,
    }
}

pub(crate) struct Engine<'a> {
    base: SideTables<'a>,
    ours: SideTables<'a>,
    theirs: SideTables<'a>,
    m_ours: &'a Matching,
    m_theirs: &'a Matching,
    lang: &'a dyn Language,
    cfg: &'a MergeConfig,

    /// Where each base node is emitted. Indexed by base `NodeId`.
    home: Vec<Option<Home>>,
    /// Base nodes both sides moved, to different places.
    move_conflict: Vec<bool>,
    /// Whether a verbatim splice of this node would be wrong because something
    /// inside it needs merging. Indexed by that side's `NodeId`.
    descend_ours: Vec<bool>,
    descend_theirs: Vec<bool>,
    /// Cycle guard for `merge_at`, indexed by base `NodeId`.
    in_progress: Vec<bool>,

    nodes: Vec<MergedNode>,
    leads: Vec<Gap>,
    stats: MergeStats,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(
        base: SideTables<'a>,
        ours: &'a SourceTree,
        theirs: &'a SourceTree,
        m_ours: &'a Matching,
        m_theirs: &'a Matching,
        lang: &'a dyn Language,
        cfg: &'a MergeConfig,
    ) -> Self {
        let stats = MergeStats {
            base_nodes: base.tree.len(),
            ours_nodes: ours.len(),
            theirs_nodes: theirs.len(),
            ..MergeStats::default()
        };
        let mut engine = Self {
            home: vec![None; base.tree.len()],
            move_conflict: vec![false; base.tree.len()],
            descend_ours: vec![false; ours.len()],
            descend_theirs: vec![false; theirs.len()],
            in_progress: vec![false; base.tree.len()],
            base,
            ours: SideTables::build(ours, lang),
            theirs: SideTables::build(theirs, lang),
            m_ours,
            m_theirs,
            lang,
            cfg,
            nodes: Vec::new(),
            leads: Vec::new(),
            stats,
        };
        engine.plan_homes();
        engine.plan_descents();
        engine
    }

    // ---------------------------------------------------------------- helpers

    fn tab(&self, side: Side) -> &SideTables<'a> {
        match side {
            Side::Base => &self.base,
            Side::Ours => &self.ours,
            Side::Theirs => &self.theirs,
        }
    }

    /// The base node a side node came from.
    fn origin(&self, side: Side, node: NodeId) -> Option<NodeId> {
        match side {
            Side::Base => Some(node),
            Side::Ours => self.m_ours.src_of(node),
            Side::Theirs => self.m_theirs.src_of(node),
        }
    }

    /// The side node a base node maps to.
    fn image(&self, side: Side, base: NodeId) -> Option<NodeId> {
        match side {
            Side::Base => Some(base),
            Side::Ours => self.m_ours.dst_of(base),
            Side::Theirs => self.m_theirs.dst_of(base),
        }
    }

    /// The container a side node sits in, normalised to the ancestor when that
    /// container has a counterpart there.
    fn container_home(&self, side: Side, container: NodeId) -> Home {
        match self.origin(side, container) {
            Some(b) => Home::Base(b),
            None => Home::Local(side, container),
        }
    }

    fn home_of(&self, b: NodeId) -> Option<Home> {
        self.home[b.index()]
    }

    fn change(&self, b: NodeId, side: Side, s: NodeId) -> Change {
        let t = self.tab(side);
        if self.base.extent_bytes(b) == t.extent_bytes(s) {
            Change::None
        } else if self.base.full_hash(b) == t.full_hash(s) {
            Change::Formatting
        } else {
            Change::Content
        }
    }

    fn changed_at_all(&self, b: NodeId, side: Side) -> bool {
        match self.image(side, b) {
            None => true,
            Some(s) => self.change(b, side, s) != Change::None,
        }
    }

    /// Whether the two sides ended up at the same code *and* the same comments.
    ///
    /// The hash is confirmed structurally before it is believed: a 64-bit
    /// collision here would not be a wrong diff, it would be a wrong merge, and
    /// the confirmation is `O(size)` on a path that was `O(size)` anyway.
    fn converged(&self, o: NodeId, t: NodeId) -> bool {
        self.ours.metrics.hash(o) == self.theirs.metrics.hash(t)
            && self.ours.full_hash(o) == self.theirs.full_hash(t)
            && structurally_equal(
                self.ours.tree,
                &self.ours.metrics,
                self.theirs.tree,
                &self.theirs.metrics,
                self.lang,
                o,
                t,
            )
    }

    fn commutable(&self, container_kind: &str, element_kind: &str) -> bool {
        match self.lang.child_list_kind(container_kind) {
            ChildListKind::Ordered => false,
            ChildListKind::Unordered => true,
            ChildListKind::PartiallyUnordered { unordered_kinds } => {
                unordered_kinds.contains(&element_kind)
            }
        }
    }

    // ------------------------------------------------------------- the planner

    /// Decide, once and globally, which container each base node is emitted
    /// under.
    ///
    /// Doing this up front rather than while descending is what guarantees a
    /// moved node is emitted exactly once: the container it left drops it, and
    /// the container it arrived in picks it up.
    fn plan_homes(&mut self) {
        for i in 0..self.base.tree.len() {
            let b = NodeId(i as u32);
            if !self.base.metrics.participates(b) {
                continue;
            }
            let Some(base_parent) = self.base.metrics.parent(b) else {
                continue; // the root has no home to compute
            };
            let base_home = Home::Base(base_parent);

            let side_home = |side: Side| -> Option<Home> {
                let s = self.image(side, b)?;
                if !self.cfg.detect_moves {
                    return Some(base_home);
                }
                let parent = self.tab(side).metrics.parent(s)?;
                Some(self.container_home(side, parent))
            };
            let ho = side_home(Side::Ours);
            let ht = side_home(Side::Theirs);

            let mut clash = false;
            let home = match (ho, ht) {
                (None, None) => base_home,
                (Some(h), None) | (None, Some(h)) => h,
                (Some(ho), Some(ht)) => match (ho != base_home, ht != base_home) {
                    (false, false) => base_home,
                    (true, false) => ho,
                    (false, true) => ht,
                    (true, true) if ho == ht => ho,
                    (true, true) => {
                        clash = true;
                        ho
                    }
                },
            };
            self.move_conflict[b.index()] = clash;
            if home != base_home {
                self.stats.reparented += 1;
            }
            self.home[b.index()] = Some(home);
        }
    }

    /// Mark the side nodes whose subtrees cannot simply be spliced.
    ///
    /// A subtree inserted on one side is normally emitted verbatim. It must not
    /// be when it *contains* an ancestor-matched node that the other side also
    /// changed — that is the "we wrapped the block in an `if`, they edited a
    /// statement inside it" case, and getting it right is the point of merging
    /// on trees at all.
    fn plan_descents(&mut self) {
        for (side, other) in [(Side::Ours, Side::Theirs), (Side::Theirs, Side::Ours)] {
            let tree: &SourceTree = self.tab(side).tree;
            let mut flags = vec![false; tree.len()];
            for (i, flag) in flags.iter_mut().enumerate() {
                let d = NodeId(i as u32);
                let Some(b) = self.origin(side, d) else {
                    continue;
                };
                let Some(parent) = self.tab(side).metrics.parent(d) else {
                    continue;
                };
                let expected = self.container_home(side, parent);
                let other_changed = match self.image(other, b) {
                    None => true,
                    Some(s) => self.base.full_hash(b) != self.tab(other).full_hash(s),
                };
                *flag = self.home_of(b) != Some(expected)
                    || self.move_conflict[b.index()]
                    || other_changed;
            }
            // `parent < child`, so one reverse scan propagates upwards.
            for i in (0..tree.len()).rev() {
                if flags[i]
                    && let Some(p) = tree.node(NodeId(i as u32)).parent
                {
                    flags[p.index()] = true;
                }
            }
            if side == Side::Ours {
                self.descend_ours = flags;
            } else {
                self.descend_theirs = flags;
            }
        }
    }

    fn descend(&self, side: Side, node: NodeId) -> bool {
        match side {
            Side::Ours => self.descend_ours[node.index()],
            Side::Theirs => self.descend_theirs[node.index()],
            Side::Base => false,
        }
    }

    // ------------------------------------------------------------ arena writes

    fn push(&mut self, node: MergedNode) -> MergedId {
        let id = MergedId(self.nodes.len() as u32);
        self.nodes.push(node);
        self.leads.push(Gap::none(Side::Base));
        id
    }

    fn splice(&mut self, side: Side, node: NodeId) -> MergedId {
        self.push(MergedNode::Splice { side, node })
    }

    fn conflict(
        &mut self,
        base: Option<Vec<NodeId>>,
        ours: Vec<NodeId>,
        theirs: Vec<NodeId>,
        reason: ConflictReason,
    ) -> MergedId {
        self.push(MergedNode::Conflict(Conflict {
            base,
            ours,
            theirs,
            reason,
        }))
    }

    fn set_lead(&mut self, id: MergedId, gap: Gap) {
        self.leads[id.index()] = gap;
    }

    // ------------------------------------------------------------- the descent

    pub(crate) fn run(mut self) -> MergeOutcome {
        let (br, or, tr) = (
            self.base.tree.root_id(),
            self.ours.tree.root_id(),
            self.theirs.tree.root_id(),
        );

        let root = if self.m_ours.dst_of(br) == Some(or) && self.m_theirs.dst_of(br) == Some(tr) {
            self.merge_at(br, Some(or), Some(tr)).0
        } else {
            self.conflict(
                Some(vec![br]),
                vec![or],
                vec![tr],
                ConflictReason::RootMismatch,
            )
        };

        let ours_fates = fate::classify(&self.base, &self.ours, self.m_ours);
        let theirs_fates = fate::classify(&self.base, &self.theirs, self.m_theirs);
        let mut stats = std::mem::take(&mut self.stats);
        stats.ours_fates = fate::count(&ours_fates);
        stats.theirs_fates = fate::count(&theirs_fates);

        let tree = MergedTree::from_parts(
            std::mem::take(&mut self.nodes),
            std::mem::take(&mut self.leads),
            root,
        );
        let conflicts = self.summarise(&tree);
        stats.conflicts = conflicts.len();
        for id in tree.walk() {
            stats.merged_nodes += 1;
            match tree.node(id) {
                MergedNode::Splice { .. } => stats.splices += 1,
                MergedNode::Rebuilt { .. } => stats.rebuilds += 1,
                MergedNode::Conflict(_) => {}
            }
        }

        MergeOutcome {
            tree,
            conflicts,
            stats,
            ours_fates,
            theirs_fates,
        }
    }

    /// [`Engine::merge_inner`], with conflict promotion applied on the way out.
    ///
    /// See the crate docs, "Conflict promotion": a conflict raised below a
    /// statement or declaration is replaced by a conflict *over* that statement
    /// or declaration, so the emitted region is something a reader can act on.
    fn merge_at(&mut self, b: NodeId, o: Option<NodeId>, t: Option<NodeId>) -> (MergedId, Promo) {
        if self.in_progress[b.index()] {
            // Only reachable if a matching relates a node to one of its own
            // ancestors, which `sm-match` does not do. Degrade rather than
            // recurse forever.
            let id = match (o, t) {
                (Some(o), _) => self.splice(Side::Ours, o),
                (None, Some(t)) => self.splice(Side::Theirs, t),
                (None, None) => self.splice(Side::Base, b),
            };
            return (id, None);
        }
        self.in_progress[b.index()] = true;
        let (id, promo) = self.merge_inner(b, o, t);
        self.in_progress[b.index()] = false;

        if let Some(reason) = promo.clone()
            && self.cfg.is_boundary(self.base.tree.node(b).kind)
        {
            let promoted = self.conflict(
                Some(vec![b]),
                o.into_iter().collect(),
                t.into_iter().collect(),
                reason,
            );
            let lead = self.leads[id.index()].clone();
            self.set_lead(promoted, lead);
            return (promoted, None);
        }
        (id, promo)
    }

    fn merge_inner(
        &mut self,
        b: NodeId,
        o: Option<NodeId>,
        t: Option<NodeId>,
    ) -> (MergedId, Promo) {
        let at_base_home = self
            .base
            .metrics
            .parent(b)
            .is_some_and(|p| self.home_of(b) == Some(Home::Base(p)));
        match (o, t) {
            (None, None) => {
                let reason = ConflictReason::Unmergeable {
                    detail: "requested a node that is absent from both sides".into(),
                };
                let id = self.conflict(Some(vec![b]), Vec::new(), Vec::new(), reason.clone());
                (id, Some(reason))
            }
            (Some(o), None) => {
                let reason = if at_base_home {
                    ConflictReason::UpdateDelete
                } else {
                    ConflictReason::MoveDelete
                };
                let id = self.conflict(Some(vec![b]), vec![o], Vec::new(), reason.clone());
                (id, Some(reason))
            }
            (None, Some(t)) => {
                let reason = if at_base_home {
                    ConflictReason::DeleteUpdate
                } else {
                    ConflictReason::DeleteMove
                };
                let id = self.conflict(Some(vec![b]), Vec::new(), vec![t], reason.clone());
                (id, Some(reason))
            }
            (Some(o), Some(t)) => self.merge_matched(b, o, t),
        }
    }

    /// The node-level decision table (SPEC.md §4.5). See the crate docs for the
    /// expanded form and the reasoning behind each row.
    fn merge_matched(&mut self, b: NodeId, o: NodeId, t: NodeId) -> (MergedId, Promo) {
        if self.move_conflict[b.index()] {
            let id = self.conflict(Some(vec![b]), vec![o], vec![t], ConflictReason::MoveMove);
            return (id, Some(ConflictReason::MoveMove));
        }

        let co = self.change(b, Side::Ours, o);
        let ct = self.change(b, Side::Theirs, t);

        // Rows 1 and 2: one side is byte-for-byte unchanged, so the other
        // side's bytes are the answer. At the root this *is* SPEC.md §4.6's
        // invariant, which is why the invariant is structural rather than
        // something the emitter has to be careful about.
        if co == Change::None {
            return (self.splice(Side::Theirs, t), None);
        }
        if ct == Change::None {
            return (self.splice(Side::Ours, o), None);
        }
        // Row 3: convergent edits. Both sides' bytes are equally correct, so
        // the tie goes to ours — see the crate docs, "Side preference".
        if self.converged(o, t) {
            return (self.splice(Side::Ours, o), None);
        }
        // Rows 4 and 5: a pure reformat loses to a content change. Reprinting
        // one side's code in the other side's layout is not something a byte
        // splicer can do, and conflicting here would be a regression against
        // git on a change git does not even notice.
        if co == Change::Formatting {
            return (self.splice(Side::Theirs, t), None);
        }
        if ct == Change::Formatting {
            return (self.splice(Side::Ours, o), None);
        }

        // Both sides changed content, differently. A node's own attached
        // comments are part of its frame, so settle who owns the frame first.
        let bt = self.base.own_trivia_hash(b);
        let ot = self.ours.own_trivia_hash(o);
        let tt = self.theirs.own_trivia_hash(t);
        if ot != bt && tt != bt && ot != tt {
            let id = self.conflict(Some(vec![b]), vec![o], vec![t], ConflictReason::CommentEdit);
            return (id, Some(ConflictReason::CommentEdit));
        }
        let frame = if tt != bt && ot == bt {
            Side::Theirs
        } else {
            Side::Ours
        };

        self.merge_children(b, o, t, frame)
    }

    // ------------------------------------------------------ child-list merging

    fn merge_children(
        &mut self,
        b: NodeId,
        o: NodeId,
        t: NodeId,
        frame: Side,
    ) -> (MergedId, Promo) {
        let container_kind = self.base.tree.node(b).kind;
        let home = Home::Base(b);

        let items_b = self.base.items(b);
        let items_o = self.ours.items(o);
        let items_t = self.theirs.items(t);
        if items_b.is_empty() || items_o.is_empty() || items_t.is_empty() {
            // A leaf, or a container one side emptied entirely. There is
            // nothing to align, so the disagreement is the node itself.
            let id = self.conflict(
                Some(vec![b]),
                vec![o],
                vec![t],
                ConflictReason::UpdateUpdate,
            );
            return (id, Some(ConflictReason::UpdateUpdate));
        }

        let (keys_b, keep_b) = self.keys(Side::Base, &items_b, home);
        let (keys_o, keep_o) = self.keys(Side::Ours, &items_o, home);
        let (keys_t, keep_t) = self.keys(Side::Theirs, &items_t, home);
        let mut keys_o = keys_o;
        let mut keys_t = keys_t;
        if self.cfg.deduplicate_insertions {
            self.deduplicate(&items_o, &mut keys_o, &items_t, &mut keys_t);
        }

        // Elements that moved away are simply absent from this list.
        let seq_b = filter(&keys_b, &keep_b);
        let seq_o = filter(&keys_o, &keep_o);
        let seq_t = filter(&keys_t, &keep_t);
        // Elements that left this container are dropped from all three
        // sequences; counting them here is what makes the covering rule
        // visible in the report rather than only in its absence.
        self.stats.covered_deletions += keep_b.iter().filter(|k| !**k).count();
        let idx_b = indices(&keep_b);
        let idx_o = indices(&keep_o);
        let idx_t = indices(&keep_t);

        let hunks = diff3(&seq_b, &seq_o, &seq_t, self.cfg.alignment_budget);

        // Leads, precomputed so the emission loop can borrow `self` mutably.
        let lead_b: Vec<_> = (0..items_b.len())
            .map(|i| self.base.lead(b, &items_b, i))
            .collect();
        let lead_o: Vec<_> = (0..items_o.len())
            .map(|i| self.ours.lead(o, &items_o, i))
            .collect();
        let lead_t: Vec<_> = (0..items_t.len())
            .map(|i| self.theirs.lead(t, &items_t, i))
            .collect();
        let shape_b: Vec<_> = lead_b.iter().map(|r| shape_of(self.base.tree, r)).collect();
        let shape_o: Vec<_> = lead_o.iter().map(|r| shape_of(self.ours.tree, r)).collect();
        let shape_t: Vec<_> = lead_t
            .iter()
            .map(|r| shape_of(self.theirs.tree, r))
            .collect();
        let rpos_b = reverse(&idx_b, items_b.len());
        let rpos_o = reverse(&idx_o, items_o.len());
        let rpos_t = reverse(&idx_t, items_t.len());

        // Where the framing revision has each key, for the whitespace rule.
        let mut frame_index: HashMap<Key, usize> = HashMap::new();
        for (i, k) in if frame == Side::Ours { &seq_o } else { &seq_t }
            .iter()
            .enumerate()
        {
            frame_index.entry(*k).or_insert(i);
        }

        let ctx = ListCtx {
            b,
            frame,
            container_kind,
            items_b,
            items_o,
            items_t,
            seq_b,
            seq_o,
            seq_t,
            idx_b,
            idx_o,
            idx_t,
            lead_b,
            lead_o,
            lead_t,
            shape_b,
            shape_o,
            shape_t,
            rpos_b,
            rpos_o,
            rpos_t,
            frame_index,
        };

        let mut children: Vec<MergedId> = Vec::new();
        let mut promo: Promo = None;
        // What the merged list has put in front of the next item, which is what
        // decides whether a copied gap is still valid. See `ListCtx::lead_for`.
        let mut prev = Neighbour::Start;

        for hunk in hunks {
            let mut kind = hunk.kind;
            let mut forced: Option<ConflictReason> = None;
            match kind {
                HunkKind::OursOnly => {
                    if let Some(reason) = self.deletion_escalation(&ctx, &hunk.base, Side::Ours) {
                        kind = HunkKind::Conflicting;
                        forced = Some(reason);
                    }
                }
                HunkKind::TheirsOnly => {
                    if let Some(reason) = self.deletion_escalation(&ctx, &hunk.base, Side::Theirs) {
                        kind = HunkKind::Conflicting;
                        forced = Some(reason);
                    }
                }
                _ => {}
            }

            match kind {
                HunkKind::Stable | HunkKind::BothSame => {
                    let range = if frame == Side::Ours {
                        hunk.ours.clone()
                    } else {
                        hunk.theirs.clone()
                    };
                    self.emit_run(&ctx, frame, range, &mut children, &mut promo, &mut prev);
                }
                HunkKind::OursOnly => {
                    self.emit_run(
                        &ctx,
                        Side::Ours,
                        hunk.ours.clone(),
                        &mut children,
                        &mut promo,
                        &mut prev,
                    );
                }
                HunkKind::TheirsOnly => self.emit_run(
                    &ctx,
                    Side::Theirs,
                    hunk.theirs.clone(),
                    &mut children,
                    &mut promo,
                    &mut prev,
                ),
                HunkKind::Conflicting => {
                    if forced.is_none()
                        && self.try_set_merge(&ctx, &hunk, &mut children, &mut promo, &mut prev)
                    {
                        self.stats.set_merged_regions += 1;
                    } else {
                        let reason = forced.unwrap_or_else(|| self.conflict_reason(&ctx, &hunk));
                        self.emit_conflict(&ctx, &hunk, reason, &mut children, &mut promo);
                        prev = Neighbour::Opaque;
                    }
                }
            }
        }

        let node = if frame == Side::Ours { o } else { t };
        let id = self.push(MergedNode::Rebuilt {
            side: frame,
            node,
            children,
        });
        (id, promo)
    }

    /// Build the identity keys for one revision's item list, and say which
    /// items belong in *this* container at all.
    fn keys(&self, side: Side, items: &[Item], home: Home) -> (Vec<Key>, Vec<bool>) {
        let tab = self.tab(side);
        let mut keys = Vec::with_capacity(items.len());
        let mut keep = Vec::with_capacity(items.len());
        for item in items {
            let node = tab.tree.node(item.node);
            let key = if item.is_element {
                if self.cfg.is_atomic_set_kind(node.kind) {
                    Key::Text(text_key(tab.tree.node_bytes(item.node)))
                } else {
                    match self.origin(side, item.node) {
                        Some(b) => Key::Base(b.0),
                        None => Key::New(tag_of(side), item.node.0),
                    }
                }
            } else {
                Key::Token(node.kind_id)
            };
            // An element whose home is elsewhere is absent from *every* one of
            // the three sequences for this container, the ancestor's included.
            // Dropping it from the two sides but not from the ancestor would
            // make the move read as a deletion by both sides, and then a
            // replacement on the side that moved it reads as delete/insert.
            let here = match key {
                Key::Base(b) => self.home_of(NodeId(b)) == Some(home),
                _ => true,
            };
            keys.push(key);
            keep.push(here);
        }
        (keys, keep)
    }

    /// Give the same key to an insertion both sides made identically.
    ///
    /// Without this, "both branches added the same helper method" is two
    /// unrelated insertions at one anchor, i.e. a conflict, when the answer is
    /// one copy of the method. Identity is checked structurally and on
    /// comments, not on raw bytes, so the two copies may differ in layout.
    fn deduplicate(
        &mut self,
        items_o: &[Item],
        keys_o: &mut [Key],
        items_t: &[Item],
        keys_t: &mut [Key],
    ) {
        let mut claimed = vec![false; items_o.len()];
        let mut deduped = 0usize;
        for (ti, key_t) in keys_t.iter_mut().enumerate() {
            if !matches!(key_t, Key::New(..)) || !items_t[ti].is_element {
                continue;
            }
            let t_node = items_t[ti].node;
            for (oi, key_o) in keys_o.iter().enumerate() {
                if claimed[oi] || !matches!(key_o, Key::New(..)) || !items_o[oi].is_element {
                    continue;
                }
                let o_node = items_o[oi].node;
                if self.ours.tree.node(o_node).kind != self.theirs.tree.node(t_node).kind {
                    continue;
                }
                if self.converged(o_node, t_node) {
                    claimed[oi] = true;
                    *key_t = *key_o;
                    deduped += 1;
                    break;
                }
            }
        }
        self.stats.deduplicated_insertions += deduped;
    }

    /// Does taking one side's version of a region silently discard a change the
    /// *other* side made to something that side deleted?
    ///
    /// This is SPEC.md §4.5's delete/modify row, and the sequence merge cannot
    /// see it on its own: at the level of identities, "they kept the element"
    /// and "they kept the element and rewrote its body" look the same.
    ///
    /// The escape hatch is docs/prior-art.md §2.5's covering check in its
    /// simplest honest form: if the element *moved* rather than being deleted,
    /// the other side's changes are applied at the destination, so there is
    /// nothing to lose and no conflict to raise.
    fn deletion_escalation(
        &self,
        ctx: &ListCtx,
        base_range: &Range<usize>,
        taken: Side,
    ) -> Option<ConflictReason> {
        let other = if taken == Side::Ours {
            Side::Theirs
        } else {
            Side::Ours
        };
        for pos in base_range.clone() {
            let key = ctx.seq_b[pos];
            let Key::Base(raw) = key else {
                continue;
            };
            // Absent from this *region* is not absent from the list: a reorder
            // reads as a deletion here and an insertion further along, and
            // conflicting on it would both be wrong and emit the element twice.
            if ctx.seq_of(taken).contains(&key) {
                continue;
            }
            let b = NodeId(raw);
            debug_assert_eq!(
                self.home_of(b),
                Some(Home::Base(ctx.b)),
                "an element with another home is filtered out of every sequence"
            );
            // The other side left this region alone at the identity level, so
            // whatever it did to this element it did in place.
            if self.image(other, b).is_some() && self.changed_at_all(b, other) {
                return Some(if taken == Side::Ours {
                    ConflictReason::DeleteUpdate
                } else {
                    ConflictReason::UpdateDelete
                });
            }
        }
        None
    }

    /// Emit one run of entries, taken from one revision.
    fn emit_run(
        &mut self,
        ctx: &ListCtx,
        from: Side,
        range: Range<usize>,
        out: &mut Vec<MergedId>,
        promo: &mut Promo,
        prev: &mut Neighbour,
    ) {
        for pos in range {
            let item_idx = ctx.idx_of(from)[pos];
            let key = ctx.seq_of(from)[pos];
            let item = ctx.items(from)[item_idx].clone();

            let child = if !item.is_element {
                self.splice(from, item.node)
            } else {
                match key {
                    Key::Base(raw) => {
                        let b = NodeId(raw);
                        let (id, p) =
                            self.merge_at(b, self.m_ours.dst_of(b), self.m_theirs.dst_of(b));
                        *promo = promo.take().or(p);
                        id
                    }
                    Key::New(tag, raw) => {
                        let (id, p) = self.emit_from(side_of(tag), NodeId(raw));
                        *promo = promo.take().or(p);
                        id
                    }
                    Key::Text(_) | Key::Token(_) => self.splice(from, item.node),
                }
            };
            let gap = ctx.lead_for(from, item_idx, key, *prev);
            self.set_lead(child, gap);
            out.push(child);
            *prev = Neighbour::Item(key);
        }
    }

    /// Resolve a conflicting region by set union, when every element in it
    /// belongs to the same commutative group (docs/prior-art.md §2.4).
    ///
    /// Returns `false` if the region does not qualify, in which case the caller
    /// emits a conflict.
    ///
    /// # Separators
    ///
    /// Some commutative groups have separator tokens between their elements —
    /// TypeScript's `{ a, b }` import clause, Java's `throws A, B`. A union that
    /// removes an element from such a run also has to remove the right comma,
    /// and getting that wrong produces code that does not parse. So separators
    /// are tolerated in exactly one shape: a region that is a **pure insertion
    /// on both sides**, where nothing is removed and each side's run —
    /// separators and all — can be taken whole. Anything else declines, and a
    /// conflict is the answer.
    fn try_set_merge(
        &mut self,
        ctx: &ListCtx,
        hunk: &Hunk,
        out: &mut Vec<MergedId>,
        promo: &mut Promo,
        prev: &mut Neighbour,
    ) -> bool {
        let mut tokens_present = false;
        for (side, range) in [
            (Side::Base, &hunk.base),
            (Side::Ours, &hunk.ours),
            (Side::Theirs, &hunk.theirs),
        ] {
            for pos in range.clone() {
                let item = &ctx.items(side)[ctx.idx_of(side)[pos]];
                if item.is_element {
                    let kind = self.tab(side).tree.node(item.node).kind;
                    if !self.commutable(ctx.container_kind, kind) {
                        return false;
                    }
                } else {
                    tokens_present = true;
                }
            }
        }

        let base_keys: Vec<Key> = ctx.seq_b[hunk.base.clone()].to_vec();
        let ours_keys: Vec<Key> = ctx.seq_o[hunk.ours.clone()].to_vec();
        let theirs_keys: Vec<Key> = ctx.seq_t[hunk.theirs.clone()].to_vec();

        if tokens_present {
            if !hunk.base.is_empty() {
                return false;
            }
            // Their run is taken whole or not at all: a partial take would need
            // to decide which separator goes with which element.
            let their_elements: Vec<Key> = hunk
                .theirs
                .clone()
                .filter(|&pos| ctx.items(Side::Theirs)[ctx.idx_of(Side::Theirs)[pos]].is_element)
                .map(|pos| ctx.seq_t[pos])
                .collect();
            let duplicates = their_elements
                .iter()
                .filter(|k| ours_keys.contains(k))
                .count();
            if duplicates != 0 && duplicates != their_elements.len() {
                return false;
            }
            self.emit_run(ctx, Side::Ours, hunk.ours.clone(), out, promo, prev);
            if duplicates == 0 {
                self.emit_run(ctx, Side::Theirs, hunk.theirs.clone(), out, promo, prev);
            }
            return true;
        }

        // A set union must not quietly drop a change either. Same rule as the
        // ordered path: an element one side deleted and the other side edited
        // is a conflict, unless it moved.
        for key in &base_keys {
            let Key::Base(raw) = *key else { continue };
            let b = NodeId(raw);
            if self.home_of(b) != Some(Home::Base(ctx.b)) {
                continue;
            }
            let dropped_by_ours = !ours_keys.contains(key);
            let dropped_by_theirs = !theirs_keys.contains(key);
            if (dropped_by_ours && !dropped_by_theirs && self.changed_at_all(b, Side::Theirs))
                || (dropped_by_theirs && !dropped_by_ours && self.changed_at_all(b, Side::Ours))
            {
                return false;
            }
        }

        // Our order is preserved; their additions follow. Deterministic, and it
        // keeps a group one side reordered in that side's order.
        for pos in hunk.ours.clone() {
            let key = ctx.seq_o[pos];
            if base_keys.contains(&key) && !theirs_keys.contains(&key) {
                continue; // they deleted it
            }
            self.emit_run(ctx, Side::Ours, pos..pos + 1, out, promo, prev);
        }
        for pos in hunk.theirs.clone() {
            let key = ctx.seq_t[pos];
            if base_keys.contains(&key) || ours_keys.contains(&key) {
                continue;
            }
            self.emit_run(ctx, Side::Theirs, pos..pos + 1, out, promo, prev);
        }
        true
    }

    fn conflict_reason(&self, ctx: &ListCtx, hunk: &Hunk) -> ConflictReason {
        let all_tokens = |side: Side, range: &Range<usize>| {
            range
                .clone()
                .all(|pos| !ctx.items(side)[ctx.idx_of(side)[pos]].is_element)
        };
        if hunk.base.is_empty() {
            let commutative = hunk.ours.clone().all(|pos| {
                let item = &ctx.items_o[ctx.idx_o[pos]];
                item.is_element
                    && self.commutable(ctx.container_kind, self.ours.tree.node(item.node).kind)
            });
            return if commutative {
                ConflictReason::InsertInsert
            } else {
                ConflictReason::OrderedInsertCollision
            };
        }
        if hunk.ours.is_empty() {
            return ConflictReason::DeleteUpdate;
        }
        if hunk.theirs.is_empty() {
            return ConflictReason::UpdateDelete;
        }
        if all_tokens(Side::Base, &hunk.base)
            && all_tokens(Side::Ours, &hunk.ours)
            && all_tokens(Side::Theirs, &hunk.theirs)
        {
            return ConflictReason::KindClash;
        }
        let sorted = |keys: &[Key]| {
            let mut v: Vec<(u8, u64)> = keys.iter().map(key_ord).collect();
            v.sort_unstable();
            v
        };
        let sb = sorted(&ctx.seq_b[hunk.base.clone()]);
        if sb == sorted(&ctx.seq_o[hunk.ours.clone()])
            && sb == sorted(&ctx.seq_t[hunk.theirs.clone()])
        {
            return ConflictReason::ReorderConflict;
        }
        ConflictReason::UpdateUpdate
    }

    fn emit_conflict(
        &mut self,
        ctx: &ListCtx,
        hunk: &Hunk,
        reason: ConflictReason,
        out: &mut Vec<MergedId>,
        promo: &mut Promo,
    ) {
        let nodes = |side: Side, range: Range<usize>| -> Vec<NodeId> {
            range
                .map(|pos| ctx.items(side)[ctx.idx_of(side)[pos]].node)
                .collect()
        };
        let base_nodes = nodes(Side::Base, hunk.base.clone());
        let ours_nodes = nodes(Side::Ours, hunk.ours.clone());
        let theirs_nodes = nodes(Side::Theirs, hunk.theirs.clone());

        // A conflict may stay where it is only if every node in it is something
        // a reader can act on standalone. Otherwise it gets promoted.
        let coherent = |side: Side, ids: &[NodeId]| {
            ids.iter().all(|&n| {
                let node = self.tab(side).tree.node(n);
                node.is_named && self.cfg.is_boundary(node.kind)
            })
        };
        let inline = coherent(Side::Base, &base_nodes)
            && coherent(Side::Ours, &ours_nodes)
            && coherent(Side::Theirs, &theirs_nodes);

        let gap = ctx.conflict_lead(hunk);
        let id = self.conflict(
            (!base_nodes.is_empty()).then_some(base_nodes),
            ours_nodes,
            theirs_nodes,
            reason.clone(),
        );
        self.set_lead(id, gap);
        out.push(id);
        if !inline {
            *promo = promo.take().or(Some(reason));
        }
    }

    /// Emit a subtree that exists on one side only.
    ///
    /// Verbatim, unless [`Engine::plan_descents`] flagged it as containing
    /// something that has to be merged.
    fn emit_from(&mut self, side: Side, node: NodeId) -> (MergedId, Promo) {
        if !self.descend(side, node) {
            return (self.splice(side, node), None);
        }
        let items = self.tab(side).items(node);
        if items.is_empty() {
            return (self.splice(side, node), None);
        }
        let leads: Vec<_> = (0..items.len())
            .map(|i| self.tab(side).lead(node, &items, i))
            .collect();
        let home = self.container_home(side, node);

        let mut children = Vec::new();
        let mut promo: Promo = None;
        for (i, item) in items.iter().enumerate() {
            let child = if !item.is_element {
                self.splice(side, item.node)
            } else if let Some(b) = self.origin(side, item.node) {
                if self.home_of(b) != Some(home) {
                    continue; // it lives somewhere else now
                }
                let (id, p) = self.merge_at(b, self.m_ours.dst_of(b), self.m_theirs.dst_of(b));
                promo = promo.or(p);
                id
            } else {
                let (id, p) = self.emit_from(side, item.node);
                promo = promo.or(p);
                id
            };
            self.set_lead(
                child,
                Gap::Copied {
                    side,
                    range: leads[i].clone(),
                },
            );
            children.push(child);
        }
        let id = self.push(MergedNode::Rebuilt {
            side,
            node,
            children,
        });
        (id, promo)
    }

    // -------------------------------------------------------------- reporting

    fn summarise(&self, tree: &MergedTree) -> Vec<ConflictSummary> {
        let mut out = Vec::new();
        for id in tree.walk() {
            let MergedNode::Conflict(conflict) = tree.node(id) else {
                continue;
            };
            let span = |side: Side, ids: &[NodeId]| -> Option<Range<u32>> {
                let tab = self.tab(side);
                let mut it = ids.iter().map(|&n| tab.extent(n));
                let first = it.next()?;
                Some(it.fold(first, |acc, r| acc.start.min(r.start)..acc.end.max(r.end)))
            };
            let kind = conflict
                .base
                .as_ref()
                .and_then(|b| b.first())
                .map(|&n| self.base.tree.node(n).kind)
                .or_else(|| conflict.ours.first().map(|&n| self.ours.tree.node(n).kind))
                .or_else(|| {
                    conflict
                        .theirs
                        .first()
                        .map(|&n| self.theirs.tree.node(n).kind)
                })
                .unwrap_or("?");
            out.push(ConflictSummary {
                id,
                reason: conflict.reason.clone(),
                kind: kind.to_owned(),
                base: conflict.base.as_deref().and_then(|b| span(Side::Base, b)),
                ours: span(Side::Ours, &conflict.ours),
                theirs: span(Side::Theirs, &conflict.theirs),
            });
        }
        out
    }
}

/// Everything one child-list merge needs, gathered so the emission loop can
/// take `&mut self` freely.
struct ListCtx {
    /// The ancestor's container, which is what "home" is measured against.
    b: NodeId,
    frame: Side,
    container_kind: &'static str,
    items_b: Vec<Item>,
    items_o: Vec<Item>,
    items_t: Vec<Item>,
    seq_b: Vec<Key>,
    seq_o: Vec<Key>,
    seq_t: Vec<Key>,
    idx_b: Vec<usize>,
    idx_o: Vec<usize>,
    idx_t: Vec<usize>,
    lead_b: Vec<Range<u32>>,
    lead_o: Vec<Range<u32>>,
    lead_t: Vec<Range<u32>>,
    shape_b: Vec<GapShape>,
    shape_o: Vec<GapShape>,
    shape_t: Vec<GapShape>,
    /// Item index → position in that side's aligned sequence. `usize::MAX` for
    /// an item filtered out of the sequence (one that moved away).
    rpos_b: Vec<usize>,
    rpos_o: Vec<usize>,
    rpos_t: Vec<usize>,
    frame_index: HashMap<Key, usize>,
}

impl ListCtx {
    fn items(&self, side: Side) -> &[Item] {
        match side {
            Side::Base => &self.items_b,
            Side::Ours => &self.items_o,
            Side::Theirs => &self.items_t,
        }
    }

    fn seq_of(&self, side: Side) -> &[Key] {
        match side {
            Side::Base => &self.seq_b,
            Side::Ours => &self.seq_o,
            Side::Theirs => &self.seq_t,
        }
    }

    fn idx_of(&self, side: Side) -> &[usize] {
        match side {
            Side::Base => &self.idx_b,
            Side::Ours => &self.idx_o,
            Side::Theirs => &self.idx_t,
        }
    }

    fn lead_range(&self, side: Side, item_idx: usize) -> Range<u32> {
        match side {
            Side::Base => self.lead_b[item_idx].clone(),
            Side::Ours => self.lead_o[item_idx].clone(),
            Side::Theirs => self.lead_t[item_idx].clone(),
        }
    }

    fn shape(&self, side: Side, item_idx: usize) -> GapShape {
        match side {
            Side::Base => self.shape_b[item_idx],
            Side::Ours => self.shape_o[item_idx],
            Side::Theirs => self.shape_t[item_idx],
        }
    }

    fn rpos(&self, side: Side, item_idx: usize) -> Option<usize> {
        let pos = match side {
            Side::Base => self.rpos_b[item_idx],
            Side::Ours => self.rpos_o[item_idx],
            Side::Theirs => self.rpos_t[item_idx],
        };
        (pos != usize::MAX).then_some(pos)
    }

    /// What preceded `items[item_idx]` in revision `side` — i.e. the neighbour
    /// that side's leading gap for it was measured against.
    fn neighbour_in(&self, side: Side, item_idx: usize) -> Neighbour {
        match self.rpos(side, item_idx) {
            Some(0) => Neighbour::Start,
            Some(pos) => Neighbour::Item(self.seq_of(side)[pos - 1]),
            None => Neighbour::Opaque,
        }
    }

    /// Which revision and slot the whitespace-ownership rule picks, before the
    /// validity check. See the crate docs, §9.
    fn lead_source(&self, from: Side, item_idx: usize, key: Key) -> (Side, usize) {
        if from == self.frame {
            return (from, item_idx);
        }
        if let Some(&pos) = self.frame_index.get(&key) {
            return (self.frame, self.idx_of(self.frame)[pos]);
        }
        (from, item_idx)
    }

    /// The whitespace-ownership rule, plus its validity condition. See the
    /// crate docs, §9.
    ///
    /// `prev` is what the merged list actually puts in front of this item. A
    /// copied gap was *measured* against some predecessor in the revision it
    /// came from, and it only describes a relationship that still exists if
    /// that predecessor is still the one in front. When it is not:
    ///
    /// 1. Prefer a gap from a revision where this item really did follow this
    ///    predecessor — the layout some human actually wrote for this very
    ///    pair.
    /// 2. Failing that, keep the chosen gap, **unless it is empty**: an empty
    ///    gap holds no separator, so the two items are written with nothing
    ///    between them and their tokens fuse (`static` + `int` →
    ///    `staticint`). Then take any whitespace-only gap this item has
    ///    elsewhere in the list.
    ///
    /// A substituted gap is only ever whitespace or nothing, never a gap
    /// holding a floating comment — that would duplicate the comment. So the
    /// repair cannot invent, move or lose anything a reader would see, and the
    /// output remains a pure splice with zero synthesized bytes.
    fn lead_for(&self, from: Side, item_idx: usize, key: Key, prev: Neighbour) -> Gap {
        let (side, idx) = self.lead_source(from, item_idx, key);
        let chosen = Gap::Copied {
            side,
            range: self.lead_range(side, idx),
        };
        if matches!(prev, Neighbour::Opaque) || self.neighbour_in(side, idx).same_as(prev) {
            return chosen;
        }
        let shape = self.shape(side, idx);
        let repair = self
            .matching_gap(key, prev)
            .or_else(|| shape.empty.then(|| self.any_gap(key))?)
            // Same bytes out of a different revision is not a repair. Skipping
            // it keeps the framing side's provenance wherever the two agree,
            // which is what every other tie in this crate does.
            .filter(|&(s, i)| self.shape(s, i).hash != shape.hash);
        repair.map_or(chosen, |(s, i)| Gap::Copied {
            side: s,
            range: self.lead_range(s, i),
        })
    }

    /// The gap a revision wrote for exactly this `prev`→`key` pair.
    ///
    /// Framing side first, so this breaks the same way as every other tie in
    /// the crate; a gap carrying a comment is never eligible.
    fn matching_gap(&self, key: Key, prev: Neighbour) -> Option<(Side, usize)> {
        for side in self.search_order() {
            let seq = self.seq_of(side);
            for (pos, k) in seq.iter().enumerate() {
                if *k != key {
                    continue;
                }
                let here = if pos == 0 {
                    Neighbour::Start
                } else {
                    Neighbour::Item(seq[pos - 1])
                };
                if !here.same_as(prev) {
                    continue;
                }
                let idx = self.idx_of(side)[pos];
                let shape = self.shape(side, idx);
                if shape.empty || shape.separating {
                    return Some((side, idx));
                }
            }
        }
        None
    }

    /// Any whitespace-only gap this item has where it *did* have a predecessor
    /// — the last resort that stops two tokens fusing. The shortest, and
    /// newline-free by preference, so the repair is the smallest separator any
    /// revision wrote here.
    fn any_gap(&self, key: Key) -> Option<(Side, usize)> {
        let mut best: Option<(Side, usize, bool, u32)> = None;
        for side in self.search_order() {
            let seq = self.seq_of(side);
            for (pos, k) in seq.iter().enumerate().skip(1) {
                if *k != key {
                    continue;
                }
                let idx = self.idx_of(side)[pos];
                let shape = self.shape(side, idx);
                if !shape.separating {
                    continue;
                }
                if best.is_none_or(|(_, _, nl, len)| (shape.newline, shape.len) < (nl, len)) {
                    best = Some((side, idx, shape.newline, shape.len));
                }
            }
        }
        best.map(|(side, idx, _, _)| (side, idx))
    }

    fn search_order(&self) -> [Side; 3] {
        if self.frame == Side::Theirs {
            [Side::Theirs, Side::Ours, Side::Base]
        } else {
            [Side::Ours, Side::Theirs, Side::Base]
        }
    }

    /// A conflict's lead: our layout if we have the region, then the
    /// ancestor's, then theirs.
    fn conflict_lead(&self, hunk: &Hunk) -> Gap {
        for (side, range) in [
            (Side::Ours, &hunk.ours),
            (Side::Base, &hunk.base),
            (Side::Theirs, &hunk.theirs),
        ] {
            if !range.is_empty() {
                let idx = self.idx_of(side)[range.start];
                return Gap::Copied {
                    side,
                    range: self.lead_range(side, idx),
                };
            }
        }
        Gap::Copied {
            side: self.frame,
            range: 0..0,
        }
    }
}

fn filter(keys: &[Key], keep: &[bool]) -> Vec<Key> {
    keys.iter()
        .zip(keep)
        .filter_map(|(k, &keep)| keep.then_some(*k))
        .collect()
}

/// Classify a gap's bytes once, so [`ListCtx::lead_for`] can reason about it
/// without carrying a borrow of the source around.
fn shape_of(tree: &SourceTree, range: &Range<u32>) -> GapShape {
    let bytes = &tree.source()[range.start as usize..range.end as usize];
    GapShape {
        empty: bytes.is_empty(),
        separating: !bytes.is_empty() && bytes.iter().all(u8::is_ascii_whitespace),
        newline: bytes.contains(&b'\n'),
        len: bytes.len() as u32,
        hash: hash_bytes(bytes),
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Invert `indices`: sequence position for each item index, `usize::MAX` for an
/// item the filter dropped.
fn reverse(idx: &[usize], items: usize) -> Vec<usize> {
    let mut out = vec![usize::MAX; items];
    for (pos, &i) in idx.iter().enumerate() {
        out[i] = pos;
    }
    out
}

fn indices(keep: &[bool]) -> Vec<usize> {
    keep.iter()
        .enumerate()
        .filter_map(|(i, &k)| k.then_some(i))
        .collect()
}

fn key_ord(k: &Key) -> (u8, u64) {
    match *k {
        Key::Base(x) => (0, u64::from(x)),
        Key::Token(x) => (1, u64::from(x)),
        Key::New(s, x) => (2, (u64::from(s) << 32) | u64::from(x)),
        Key::Text(h) => (3, h),
    }
}

/// The identity of an atomic set element: its source text with all ASCII
/// whitespace removed, so that reindenting an import does not make it a
/// different import.
fn text_key(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        if b.is_ascii_whitespace() {
            continue;
        }
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::{side_of, tag_of, text_key};
    use crate::model::Side;

    #[test]
    fn side_tags_round_trip() {
        for s in Side::ALL {
            assert_eq!(side_of(tag_of(s)), s);
        }
    }

    #[test]
    fn text_keys_ignore_whitespace() {
        assert_eq!(
            text_key(b"import java.util.List;"),
            text_key(b"import  java.util.List ;")
        );
        assert_ne!(
            text_key(b"import java.util.List;"),
            text_key(b"import java.util.Set;")
        );
    }
}
