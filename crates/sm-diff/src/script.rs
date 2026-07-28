//! The edit-script data model and its derivation from a [`Matching`]
//! (SPEC.md §4.4, milestone M3).
//!
//! Read [`derive`] for the algorithm and [`EditOp`] for the semantics of each
//! operation. The two definitions worth reading before consuming this crate are
//! [`EditOp::Update`] (what counts as a *local* change) and [`MoveKind`] (how
//! re-parenting is separated from reordering, and why reordering is computed
//! with a longest-increasing-subsequence).

use serde::Serialize;
use sm_cst::{ChildListKind, Language, NodeId, SourceTree};
use sm_match::{ContentItem, Matching, TreeMetrics};

use crate::lis::longest_increasing_subsequence;

/// Why a matched pair counts as a move.
///
/// `sm-match`'s [`sm_match::visualize::move_flags`] ORs these two together
/// because a side-by-side view only needs to draw one `M`. An edit script must
/// not: they mean different things to the merge (M4) and to the emitter (M5).
/// A re-parented node changes nesting depth and therefore needs
/// reindentation; a reordered node does not. A re-parent between two branches
/// is a hard conflict; two reorders inside an unordered container are not even
/// edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    /// The node's matched parent changed: its owner, and usually its nesting
    /// depth, is different. Includes the case where one side has a parent and
    /// the other does not, and the case where the parent is matched to some
    /// *other* node.
    Reparent,
    /// The parents correspond, but the node's position among the siblings that
    /// stayed with it changed. Only ever reported for children of an
    /// [`ChildListKind::Ordered`] list — see [`derive`].
    Reorder,
}

impl MoveKind {
    /// A short stable name, used by the renderer and the JSON dump.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reparent => "reparent",
            Self::Reorder => "reorder",
        }
    }
}

/// One operation of an edit script, over stable [`NodeId`]s.
///
/// Node IDs are only meaningful together with the tree they came from:
/// `Delete`/`Update.src`/`Move.src` index the *source* tree, and
/// `Insert`/`Update.dst`/`Move.dst` index the *destination* tree.
///
/// Every node named here **participates** in matching, in `sm-match`'s sense:
/// named, not a comment, not `MISSING`. Anonymous tokens and comments never
/// appear as an operand. Punctuation changes surface as an [`EditOp::Update`]
/// on the enclosing node; comments ride along with their trivia owner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditOp {
    /// A destination subtree with no counterpart in the source.
    ///
    /// Reported at the **highest** unmatched node only: a whole inserted method
    /// is one `Insert`, not one per descendant. Use
    /// [`crate::participating_subtree`] to expand it when a consumer wants the
    /// individual nodes.
    Insert {
        /// Root of the inserted subtree, in the destination tree.
        dst: NodeId,
        /// The inserted node's participating parent, or `None` when the whole
        /// destination root is new (an empty source file).
        dst_parent: Option<NodeId>,
        /// Index of `dst` among `dst_parent`'s participating children,
        /// matched and unmatched alike. `0` when `dst_parent` is `None`.
        position: usize,
    },
    /// A source subtree with no counterpart in the destination. The mirror of
    /// [`EditOp::Insert`]; likewise reported at the highest unmatched node only.
    Delete {
        /// Root of the deleted subtree, in the source tree.
        src: NodeId,
    },
    /// A matched pair whose **own** content changed.
    ///
    /// # The definition, exactly
    ///
    /// Give every participating node a *local signature*:
    ///
    /// 1. If the node has no participating children and its kind has
    ///    [`Language::significant_text`], its source bytes; otherwise nothing.
    /// 2. Followed by its [`TreeMetrics::content`] sequence, in order, with
    ///    - every [`ContentItem::Token`] kept as its grammar kind id,
    ///    - every [`ContentItem::Node`] that **stayed** replaced by an
    ///      anonymous `Slot`,
    ///    - every other [`ContentItem::Node`] dropped.
    ///
    /// A child *stayed* iff it is matched **and** its partner is still a
    /// participating child of this node's partner. So an unmatched child (one
    /// that was inserted or deleted) and a matched child that was re-parented
    /// somewhere else both contribute nothing.
    ///
    /// A matched pair is an `Update` iff its two local signatures differ.
    ///
    /// # Why each clause is there
    ///
    /// - Leaf bytes are what makes a **rename** an `Update` rather than a
    ///   `Delete` plus an `Insert` (`total` → `sum` on one `identifier`).
    /// - Tokens are what makes `a + b` → `a - b`, `x++` → `x--` and
    ///   `public` → `public static` `Update`s. The operator and the modifier
    ///   list are anonymous tokens, so a model that dropped them would call
    ///   those pairs unchanged — and hand M4 a silently wrong merge.
    /// - Children that stayed collapse to an *identity-free* `Slot` so that a
    ///   permutation of a container's children is **not** an `Update`. Moving
    ///   is [`EditOp::Move`]'s job; if `Slot` carried the partner's id, every
    ///   reorder would also report its parent as textually changed.
    /// - Children that did not stay are dropped so that inserting a statement
    ///   into a block does not also report the block as changed, and — the
    ///   case M3 exists for — so that wrapping a body in an `if` does not
    ///   report the enclosing method as changed. In the wrapped case the
    ///   method's old body moved *into* the new one: the source's `block` left
    ///   and the destination's `block` is new, both are dropped, and what is
    ///   left on each side is the signature and the parameter list, identical.
    ///   The two facts that remain are the `Insert` of the wrapper and the
    ///   `Move` of the body, which is exactly the story.
    ///
    /// The definition is therefore *local*: a change deep in a subtree never
    /// propagates to its ancestors, and `Update`, `Insert`/`Delete` and `Move`
    /// stay orthogonal — each fact is reported exactly once, by the operation
    /// that means it.
    ///
    /// # Known residue
    ///
    /// Dropping an unmatched child leaves behind any **separator token** that
    /// was there for it. `f(a)` → `f(a, b)` reports the `Insert` of `b` *and*
    /// an `Update` of the `argument_list`, because the destination's content
    /// keeps a `,` the source's does not have. This is confined to containers
    /// with infix separators (`argument_list`, `formal_parameters`,
    /// declarator lists); block-structured containers, where the separator is
    /// part of each child, are unaffected. The extra operation is true — the
    /// argument list's punctuation really did change — and the renderer nests
    /// the `Insert` inside it, so it costs a line of output, not a wrong
    /// answer.
    Update {
        /// The source node.
        src: NodeId,
        /// Its matched partner in the destination tree.
        dst: NodeId,
    },
    /// A matched pair that is in a different place. See [`MoveKind`].
    ///
    /// A pair may be **both** a `Move` and an [`EditOp::Update`] — a method that
    /// moved and was renamed produces one of each. They are separate
    /// operations, not a combined variant, so that a consumer that only cares
    /// about location (the emitter's reindentation) and one that only cares
    /// about text (M6's name resolution) can each filter for what they need.
    Move {
        /// The source node.
        src: NodeId,
        /// Its matched partner in the destination tree.
        dst: NodeId,
        /// Whether the owner changed or only the position.
        kind: MoveKind,
    },
}

impl EditOp {
    /// The source node this operation is about, if it has one.
    #[must_use]
    pub fn src(&self) -> Option<NodeId> {
        match *self {
            Self::Insert { .. } => None,
            Self::Delete { src } | Self::Update { src, .. } | Self::Move { src, .. } => Some(src),
        }
    }

    /// The destination node this operation is about, if it has one.
    #[must_use]
    pub fn dst(&self) -> Option<NodeId> {
        match *self {
            Self::Delete { .. } => None,
            Self::Insert { dst, .. } | Self::Update { dst, .. } | Self::Move { dst, .. } => {
                Some(dst)
            }
        }
    }

    /// A short stable name: `insert`, `delete`, `update`, `move`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Insert { .. } => "insert",
            Self::Delete { .. } => "delete",
            Self::Update { .. } => "update",
            Self::Move { .. } => "move",
        }
    }

    /// Tie-break rank within one anchor position. See [`EditScript::ops`].
    const fn rank(&self) -> u8 {
        match self {
            Self::Delete { .. } => 0,
            Self::Move { .. } => 1,
            Self::Update { .. } => 2,
            Self::Insert { .. } => 3,
        }
    }
}

/// The headline counts, which is all `--stat` and the M5 report need.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub inserts: usize,
    pub deletes: usize,
    pub updates: usize,
    /// `reparents + reorders`.
    pub moves: usize,
    pub reparents: usize,
    pub reorders: usize,
}

impl Summary {
    /// Whether the script is empty, i.e. the two trees are structurally the
    /// same modulo formatting and comments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inserts == 0 && self.deletes == 0 && self.updates == 0 && self.moves == 0
    }

    fn count(&mut self, op: &EditOp) {
        match op {
            EditOp::Insert { .. } => self.inserts += 1,
            EditOp::Delete { .. } => self.deletes += 1,
            EditOp::Update { .. } => self.updates += 1,
            EditOp::Move { kind, .. } => {
                self.moves += 1;
                match kind {
                    MoveKind::Reparent => self.reparents += 1,
                    MoveKind::Reorder => self.reorders += 1,
                }
            }
        }
    }
}

/// An ordered list of [`EditOp`]s plus the counts.
///
/// A plain data model over stable node IDs: it borrows nothing, so it can be
/// stored, serialised and replayed. Everything that needs the trees back — the
/// renderer, the merge — takes them as separate arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditScript {
    /// The operations, in a **stable, deterministic order**.
    ///
    /// The sort key is `(src_anchor, dst_anchor, rank)`, where
    ///
    /// - `src_anchor` is the operation's own source node for `Delete`,
    ///   `Update` and `Move`, and for an `Insert` the source partner of its
    ///   `dst_parent` (`u32::MAX` when there is none, which sorts such
    ///   operations last);
    /// - `dst_anchor` is the operation's own destination node for `Insert`,
    ///   `Update` and `Move`, and for a `Delete` the destination partner of its
    ///   source parent (`u32::MAX` when there is none);
    /// - `rank` breaks the remaining ties as `Delete < Move < Update < Insert`.
    ///
    /// Because `sm-cst` numbers nodes in preorder, this reads as a walk down
    /// the source file, with the operations that touch one container grouped
    /// together and the container's insertions listed at the container itself.
    /// Nothing in the derivation depends on hash iteration order, so two runs
    /// over the same inputs produce byte-identical scripts.
    pub ops: Vec<EditOp>,
    /// Counts over [`EditScript::ops`].
    pub summary: Summary,
}

impl EditScript {
    /// Whether there are no operations at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Number of operations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Every operation of one kind, in script order.
    pub fn iter_kind(&self, name: &'static str) -> impl Iterator<Item = &EditOp> {
        self.ops.iter().filter(move |op| op.name() == name)
    }
}

/// Every participating node of `root`'s subtree, `root` included, in preorder.
///
/// The expansion helper for [`EditOp::Insert`] and [`EditOp::Delete`], which are
/// reported at the highest unmatched node only. A consumer that wants "every
/// node that came into existence" calls this on each `Insert`.
#[must_use]
pub fn participating_subtree(metrics: &TreeMetrics, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        out.push(id);
        stack.extend(metrics.children(id).iter().rev().copied());
    }
    out
}

/// Derive an edit script from a matching.
///
/// `src`, `dst` and `m` must be the exact arguments and result of one
/// [`sm_match::match_trees`] call: the node IDs in a matching only mean anything
/// against the trees that produced it.
///
/// # The four passes
///
/// 1. **Deletes.** Every participating source node that is unmatched and whose
///    participating parent is either absent or matched. Unmatchedness only ever
///    grows upwards, so this picks out exactly the roots of the unmatched
///    regions.
/// 2. **Inserts.** The same, on the destination side.
/// 3. **Updates.** Every matched pair whose local signature changed — see
///    [`EditOp::Update`] for the definition.
/// 4. **Moves.** Every matched pair whose participating parent is not matched
///    to its partner's participating parent is a [`MoveKind::Reparent`].
///    The rest are candidates for [`MoveKind::Reorder`], decided per container:
///    - [`ChildListKind::Unordered`] containers are skipped entirely. Two
///      branches reshuffling their imports or their class members is not an
///      edit, and calling it one would manufacture the exact false conflict
///      this project exists to remove.
///    - [`ChildListKind::PartiallyUnordered`] containers exempt children of the
///      listed kinds and check the rest. In Java that means imports may be
///      permuted freely inside `program` while the `package_declaration` and
///      the type declarations may not.
///    - [`ChildListKind::Ordered`] containers take the children that stayed
///      (matched, and whose partner is still a child of the partner container),
///      read off their destination ranks, and keep a **longest strictly
///      increasing subsequence**. Everything outside it is a `Reorder`. That is
///      what makes moving one element out of ten cost one operation instead of
///      nine.
///
/// # Cost
///
/// Linear in the two trees plus `O(k log k)` per container of `k` matched
/// children.
#[must_use]
pub fn derive(src: &SourceTree, dst: &SourceTree, m: &Matching, lang: &dyn Language) -> EditScript {
    let sm = TreeMetrics::compute(src, lang);
    let dm = TreeMetrics::compute(dst, lang);
    derive_with_metrics(src, &sm, dst, &dm, m, lang)
}

/// [`derive`], reusing side tables the caller has already built.
///
/// A three-way merge computes `TreeMetrics` for base once and matches it against
/// two sides; this is how it avoids paying for base twice.
#[must_use]
pub fn derive_with_metrics(
    src: &SourceTree,
    src_metrics: &TreeMetrics,
    dst: &SourceTree,
    dst_metrics: &TreeMetrics,
    m: &Matching,
    lang: &dyn Language,
) -> EditScript {
    let ctx = Ctx {
        src,
        sm: src_metrics,
        dst,
        dm: dst_metrics,
        lang,
        m,
    };
    let mut ops: Vec<EditOp> = Vec::new();

    // 1. Delete roots.
    for &id in src_metrics.post_order() {
        if m.is_src_matched(id) {
            continue;
        }
        if src_metrics.parent(id).is_none_or(|p| m.is_src_matched(p)) {
            ops.push(EditOp::Delete { src: id });
        }
    }

    // 2. Insert roots.
    for &id in dst_metrics.post_order() {
        if m.is_dst_matched(id) {
            continue;
        }
        let parent = dst_metrics.parent(id);
        if parent.is_none_or(|p| m.is_dst_matched(p)) {
            ops.push(EditOp::Insert {
                dst: id,
                dst_parent: parent,
                position: parent.map_or(0, |_| dst_metrics.index_in_parent(id) as usize),
            });
        }
    }

    // 3. Updates, and 4a. re-parents. Both are per-pair.
    for (a, b) in m.iter() {
        if ctx.is_update(a, b) {
            ops.push(EditOp::Update { src: a, dst: b });
        }
        let reparented = match (src_metrics.parent(a), dst_metrics.parent(b)) {
            (None, None) => false,
            (Some(pa), Some(pb)) => m.dst_of(pa) != Some(pb),
            _ => true,
        };
        if reparented {
            ops.push(EditOp::Move {
                src: a,
                dst: b,
                kind: MoveKind::Reparent,
            });
        }
    }

    // 4b. Reorders, per matched container.
    collect_reorders(&ctx, &mut ops);

    sort_ops(src_metrics, m, &mut ops);

    let mut summary = Summary::default();
    for op in &ops {
        summary.count(op);
    }
    EditScript { ops, summary }
}

/// One element of a local signature. See [`EditOp::Update`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sig {
    /// A child that stayed, deliberately without its identity.
    Slot,
    /// An anonymous token, by grammar kind id.
    Tok(u16),
}

/// The content of `id`, with the children that stayed with it collapsed to
/// `Slot` and every other child dropped. See [`EditOp::Update`].
fn local_signature(metrics: &TreeMetrics, id: NodeId, stayed: impl Fn(NodeId) -> bool) -> Vec<Sig> {
    metrics
        .content(id)
        .iter()
        .filter_map(|item| match *item {
            ContentItem::Token(k) => Some(Sig::Tok(k)),
            ContentItem::Node(c) => stayed(c).then_some(Sig::Slot),
        })
        .collect()
}

/// The two trees, their side tables, the language and the matching — everything
/// the derivation needs about *both* sides at once.
///
/// Grouped for the reason `sm-match`'s `Ctx` is: six parameters threaded
/// through every helper is six chances to transpose the source and the
/// destination silently, and a transposition here would invert every operation
/// without failing to compile.
struct Ctx<'a> {
    src: &'a SourceTree,
    sm: &'a TreeMetrics,
    dst: &'a SourceTree,
    dm: &'a TreeMetrics,
    lang: &'a dyn Language,
    m: &'a Matching,
}

impl Ctx<'_> {
    /// See [`EditOp::Update`] for what this is deciding and why.
    fn is_update(&self, a: NodeId, b: NodeId) -> bool {
        // Matched pairs are kind-equal (`sm-match` guarantees it), so one
        // lookup answers for both sides.
        let kind = self.src.node(a).kind;
        let a_leaf = self.sm.children(a).is_empty();
        let b_leaf = self.dm.children(b).is_empty();
        if self.lang.significant_text(kind)
            && a_leaf
            && b_leaf
            && self.src.node_bytes(a) != self.dst.node_bytes(b)
        {
            return true;
        }

        let from_src = local_signature(self.sm, a, |c| {
            self.m
                .dst_of(c)
                .is_some_and(|d| self.dm.parent(d) == Some(b))
        });
        let from_dst = local_signature(self.dm, b, |d| {
            self.m
                .src_of(d)
                .is_some_and(|c| self.sm.parent(c) == Some(a))
        });
        from_src != from_dst
    }
}

/// Reorder detection, one matched container at a time.
fn collect_reorders(ctx: &Ctx<'_>, ops: &mut Vec<EditOp>) {
    let (src, src_metrics, dst_metrics, m, lang) = (ctx.src, ctx.sm, ctx.dm, ctx.m, ctx.lang);
    for (pa, pb) in m.iter() {
        let kids = src_metrics.children(pa);
        if kids.len() < 2 {
            continue;
        }

        let exempt: &[&str] = match lang.child_list_kind(src.node(pa).kind) {
            // Order carries no meaning here; a permutation is not an edit.
            ChildListKind::Unordered => continue,
            ChildListKind::PartiallyUnordered { unordered_kinds } => unordered_kinds,
            ChildListKind::Ordered => &[],
        };

        // The children that *stayed*: matched, and whose partner is still a
        // child of `pb`. A child that left is already a `Reparent`, and
        // counting it here would make the elements around it look reordered.
        let stayed: Vec<(NodeId, NodeId)> = kids
            .iter()
            .filter(|&&c| !exempt.contains(&src.node(c).kind))
            .filter_map(|&c| {
                let d = m.dst_of(c)?;
                (dst_metrics.parent(d) == Some(pb)).then_some((c, d))
            })
            .collect();
        if stayed.len() < 2 {
            continue;
        }

        // Destination ranks *within the stayed set*. Node IDs are preorder, so
        // sorting by id is document order; ranking within the set rather than
        // among all of `pb`'s children is what stops an unrelated insertion
        // from shifting everything.
        let mut by_dst: Vec<usize> = (0..stayed.len()).collect();
        by_dst.sort_by_key(|&i| stayed[i].1);
        let mut rank = vec![0u32; stayed.len()];
        for (r, &i) in by_dst.iter().enumerate() {
            rank[i] = r as u32;
        }

        let keep = longest_increasing_subsequence(&rank);
        let mut kept = vec![false; stayed.len()];
        for &i in &keep {
            kept[i] = true;
        }
        for (i, &(c, d)) in stayed.iter().enumerate() {
            if !kept[i] {
                ops.push(EditOp::Move {
                    src: c,
                    dst: d,
                    kind: MoveKind::Reorder,
                });
            }
        }
    }
}

/// The documented total order. See [`EditScript::ops`].
fn sort_ops(src_metrics: &TreeMetrics, m: &Matching, ops: &mut [EditOp]) {
    let key = |op: &EditOp| -> (u32, u32, u8) {
        let src_anchor = match *op {
            EditOp::Insert { dst_parent, .. } => dst_parent
                .and_then(|p| m.src_of(p))
                .map_or(u32::MAX, |id| id.0),
            EditOp::Delete { src } | EditOp::Update { src, .. } | EditOp::Move { src, .. } => src.0,
        };
        let dst_anchor = match *op {
            EditOp::Delete { src } => src_metrics
                .parent(src)
                .and_then(|p| m.dst_of(p))
                .map_or(u32::MAX, |id| id.0),
            EditOp::Insert { dst, .. } | EditOp::Update { dst, .. } | EditOp::Move { dst, .. } => {
                dst.0
            }
        };
        (src_anchor, dst_anchor, op.rank())
    };
    ops.sort_by_key(key);
}
