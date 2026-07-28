//! Phase 1: greedy top-down isomorphic subtree matching (SPEC.md §4.3).
//!
//! Height-indexed priority queues over both trees, always popping the tallest
//! unmatched subtrees. When both sides' tallest are at the same height, that
//! whole height batch is grouped by structural hash:
//!
//! - **One candidate on each side** → verify isomorphism and map the two
//!   subtrees onto each other, node for node.
//! - **Several candidates** → resolve by ranking (see [`resolve_ambiguous`]),
//!   or, for a group too large to rank pairwise, by relative position (see
//!   [`align_by_position`]). Whatever neither can settle unambiguously is left
//!   for phase 2, and its children are pushed back onto the queue so smaller
//!   matches inside it are still reachable.
//! - **No candidate on the other side** → push the children back and continue
//!   at the next height down.
//!
//! Subtrees shorter than [`crate::MatchConfig::min_height`] are never
//! considered; the loop stops as soon as *both* queues' tallest entries fall
//! below it.

use std::collections::BTreeMap;

use sm_cst::NodeId;

use crate::context::Ctx;
use crate::metrics::TreeMetrics;

/// Ceiling on `|left candidates| × |right candidates|` for one ambiguous hash
/// group.
///
/// A pathologically repetitive file — a generated class with a thousand
/// identical accessors — would otherwise make the pairwise ranking quadratic in
/// a way that shows up in the latency budget. Above the ceiling, the group is
/// settled by [`align_by_position`] instead, which is linear.
///
/// The first version of this code *deferred* oversized groups to phase 2, and
/// that was badly wrong in a way worth recording: deferring re-opens the
/// children, whose subtrees are equally duplicated, so the ambiguity recurs at
/// every height and nothing is ever matched. Phase 2 then has no matched
/// descendants to compute a dice from, so it matches nothing either. A file of
/// 300 identical methods came back with **four** matched nodes out of 3 007.
/// Deferring is only conservative when something else can still decide.
const MAX_AMBIGUITY_PAIRS: usize = 1 << 14;

/// A max-priority queue of node IDs keyed by subtree height.
///
/// `BTreeMap` rather than `HashMap` on purpose: the phase pops "everything at
/// the current maximum height" and then iterates it, so iteration order is an
/// input to the result. A `HashMap` here would make the matcher
/// non-deterministic in a way that only shows up as a flaky snapshot months
/// later.
struct HeightQueue<'m> {
    metrics: &'m TreeMetrics,
    by_height: BTreeMap<u32, Vec<NodeId>>,
}

impl<'m> HeightQueue<'m> {
    fn new(metrics: &'m TreeMetrics) -> Self {
        Self {
            metrics,
            by_height: BTreeMap::new(),
        }
    }

    fn push(&mut self, id: NodeId) {
        self.by_height
            .entry(self.metrics.height(id))
            .or_default()
            .push(id);
    }

    /// The tallest height currently queued.
    fn peek_max(&self) -> Option<u32> {
        self.by_height.last_key_value().map(|(&h, _)| h)
    }

    /// Remove and return every node at the tallest height, sorted by ID.
    fn pop_max(&mut self) -> Vec<NodeId> {
        let Some((&h, _)) = self.by_height.last_key_value() else {
            return Vec::new();
        };
        let mut batch = self.by_height.remove(&h).unwrap_or_default();
        batch.sort_unstable();
        batch
    }

    /// Replace a node by its participating children.
    fn open(&mut self, id: NodeId) {
        for &c in self.metrics.children(id) {
            self.push(c);
        }
    }
}

pub(crate) fn run(ctx: &mut Ctx<'_>) {
    let (Some(src_root), Some(dst_root)) = (ctx.sm.root(), ctx.dm.root()) else {
        return;
    };

    let mut q1 = HeightQueue::new(ctx.sm);
    let mut q2 = HeightQueue::new(ctx.dm);
    q1.push(src_root);
    q2.push(dst_root);

    loop {
        let (Some(h1), Some(h2)) = (q1.peek_max(), q2.peek_max()) else {
            break;
        };
        // Once either side has nothing left tall enough, no pair can be tall
        // enough either. Mergiraf's stopping rule (docs/prior-art.md §2.2).
        if h1 < ctx.cfg.min_height || h2 < ctx.cfg.min_height {
            break;
        }

        if h1 > h2 {
            for id in q1.pop_max() {
                q1.open(id);
            }
            continue;
        }
        if h2 > h1 {
            for id in q2.pop_max() {
                q2.open(id);
            }
            continue;
        }

        let batch1 = q1.pop_max();
        let batch2 = q2.pop_max();

        let mut groups1: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
        for &id in &batch1 {
            groups1.entry(ctx.sm.hash(id)).or_default().push(id);
        }
        let mut groups2: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
        for &id in &batch2 {
            groups2.entry(ctx.dm.hash(id)).or_default().push(id);
        }

        for (hash, left) in &groups1 {
            let Some(right) = groups2.get(hash) else {
                continue;
            };
            if left.len() == 1 && right.len() == 1 {
                let (a, b) = (left[0], right[0]);
                if ctx.isomorphic(a, b) {
                    ctx.map_isomorphic_subtrees(a, b);
                }
            } else {
                resolve_ambiguous(ctx, left, right);
            }
        }

        // Anything the batch did not settle gets re-opened, so that a smaller
        // isomorphic subtree inside it can still be found at the next height.
        for id in batch1 {
            if !ctx.matching.is_src_matched(id) {
                q1.open(id);
            }
        }
        for id in batch2 {
            if !ctx.matching.is_dst_matched(id) {
                q2.open(id);
            }
        }
    }
}

/// The ranking key for one ambiguous candidate pair. Lower is better.
///
/// Lexicographic over three components, in this order:
///
/// 1. **Parent-context dissimilarity** — `1 - ancestor_dice`, quantised to an
///    integer so that the key is `Ord` (and so that two mathematically equal
///    dice values cannot differ by a float rounding hair). This is SPEC.md
///    §4.3's disambiguation rule.
/// 2. **Sibling-index distance** — how far apart the two nodes sit in their
///    respective parents' child lists.
/// 3. **Relative-position distance** — how far apart they sit in the two files
///    as a whole, compared by cross-multiplication so no division is involved.
///
/// Components 2 and 3 are the cheap tail of GumTree's five-level
/// `FullMappingComparator` (docs/prior-art.md §1.2). SPEC.md §4.3 asks only for
/// component 1, but on real code component 1 is *zero for every candidate* near
/// the top of the tree, where nothing has been matched yet — so without a
/// positional tail, "rank by parent context" degenerates to "everything ties,
/// defer everything", and a file of similar-looking methods matches nothing at
/// all. The tail is what turns the rule into something that decides.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct RankKey {
    context_penalty: u32,
    sibling_delta: u32,
    position_delta: u64,
}

fn rank_key(ctx: &Ctx<'_>, a: NodeId, b: NodeId, src_len: u64, dst_len: u64) -> RankKey {
    // 1e6 quantisation buckets: far finer than any dice value the ancestor
    // chains can produce (their denominators are tree depths), and integral so
    // that ties are exact.
    let dice = ctx.ancestor_dice(a, b).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let context_penalty = ((1.0 - dice) * 1_000_000.0).round() as u32;

    let ia = ctx.sm.index_in_parent(a);
    let ib = ctx.dm.index_in_parent(b);
    let sibling_delta = ia.abs_diff(ib);

    // |a/src_len - b/dst_len| without floats.
    let position_delta = (u64::from(a.0) * dst_len).abs_diff(u64::from(b.0) * src_len);

    RankKey {
        context_penalty,
        sibling_delta,
        position_delta,
    }
}

/// Settle a hash group with more than one candidate on at least one side.
///
/// Repeatedly matches **mutual, strictly-best** pairs: `(a, b)` is taken only
/// when `b` is the uniquely best remaining candidate for `a` *and* `a` is the
/// uniquely best remaining candidate for `b`. Removing a pair can turn a
/// previously tied candidate into a unique one, so the search runs to a
/// fixpoint.
///
/// Anything still ambiguous when the fixpoint is reached — a genuine tie on all
/// three ranking components, which means two candidates that are structurally
/// identical, equally placed among their siblings and equally placed in the
/// file — is **deferred to phase 2**, exactly as SPEC.md §4.3 asks. Phase 2
/// decides it from container context, which is more information than this phase
/// has. This is the conservative reading docs/prior-art.md §8.1.4 recommends
/// for a *merge* tool: Mergiraf refuses every ambiguous top-down match
/// outright; we refuse only the ones we cannot break a tie on.
fn resolve_ambiguous(ctx: &mut Ctx<'_>, left: &[NodeId], right: &[NodeId]) {
    if left.len().saturating_mul(right.len()) > MAX_AMBIGUITY_PAIRS {
        align_by_position(ctx, left, right);
        return;
    }

    let src_len = ctx.src.len().max(1) as u64;
    let dst_len = ctx.dst.len().max(1) as u64;

    // Verify isomorphism once per pair rather than once per fixpoint round.
    // `left` and `right` share a hash, so this is normally all-true; it is here
    // to keep a 64-bit collision from producing a wrong match.
    let mut viable: Vec<(NodeId, NodeId, RankKey)> = Vec::new();
    for &a in left {
        for &b in right {
            if ctx.isomorphic(a, b) {
                viable.push((a, b, rank_key(ctx, a, b, src_len, dst_len)));
            }
        }
    }

    loop {
        viable.retain(|&(a, b, _)| {
            !ctx.matching.is_src_matched(a) && !ctx.matching.is_dst_matched(b)
        });
        if viable.is_empty() {
            return;
        }

        // Best (and uniquely-best) remaining partner for each endpoint.
        let mut best_for_a: BTreeMap<NodeId, (RankKey, NodeId, bool)> = BTreeMap::new();
        let mut best_for_b: BTreeMap<NodeId, (RankKey, NodeId, bool)> = BTreeMap::new();
        for &(a, b, key) in &viable {
            update_best(&mut best_for_a, a, key, b);
            update_best(&mut best_for_b, b, key, a);
        }

        let mut accepted: Vec<(NodeId, NodeId)> = Vec::new();
        for &(a, b, _) in &viable {
            let Some(&(_, pick_b, unique_a)) = best_for_a.get(&a) else {
                continue;
            };
            let Some(&(_, pick_a, unique_b)) = best_for_b.get(&b) else {
                continue;
            };
            if unique_a && unique_b && pick_b == b && pick_a == a {
                accepted.push((a, b));
            }
        }
        if accepted.is_empty() {
            // A genuine tie. Leave it to phase 2.
            return;
        }
        for (a, b) in accepted {
            if !ctx.matching.is_src_matched(a) && !ctx.matching.is_dst_matched(b) {
                ctx.map_isomorphic_subtrees(a, b);
            }
        }
    }
}

/// Track the best partner for one endpoint, and whether it is unique.
fn update_best(
    best: &mut BTreeMap<NodeId, (RankKey, NodeId, bool)>,
    endpoint: NodeId,
    key: RankKey,
    partner: NodeId,
) {
    match best.get_mut(&endpoint) {
        None => {
            best.insert(endpoint, (key, partner, true));
        }
        Some(slot) => {
            if key < slot.0 {
                *slot = (key, partner, true);
            } else if key == slot.0 && slot.1 != partner {
                slot.2 = false;
            }
        }
    }
}

/// Settle an oversized ambiguous group in linear time, by relative position.
///
/// Every node in the group is isomorphic to every node in the other side's
/// group, so *which* pairing is chosen cannot change what is matched — only
/// which pairs later look like moves. The best pairing is therefore the one
/// that keeps things where they are, and a two-pointer walk that greedily
/// aligns by relative position in the file does exactly that.
///
/// It is also what the full ranking would converge to at this scale: in a group
/// of hundreds of identical siblings the parent-context term is the same for
/// every candidate, so the key collapses to its positional tail.
///
/// The surplus on the longer side is skipped rather than force-paired, which is
/// what keeps a deletion in the middle of a run from shifting every subsequent
/// pair by one.
fn align_by_position(ctx: &mut Ctx<'_>, left: &[NodeId], right: &[NodeId]) {
    let src_len = ctx.src.len().max(1) as u64;
    let dst_len = ctx.dst.len().max(1) as u64;
    // |a/src_len - b/dst_len|, cross-multiplied so no division is involved.
    let distance =
        |a: NodeId, b: NodeId| (u64::from(a.0) * dst_len).abs_diff(u64::from(b.0) * src_len);

    let (mut i, mut j) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        let (a, b) = (left[i], right[j]);
        let (remaining_left, remaining_right) = (left.len() - i, right.len() - j);

        // Spend the surplus where it buys a closer alignment. Both indexing
        // steps are in range: a strictly larger remainder is at least 2.
        if remaining_left > remaining_right && distance(left[i + 1], b) < distance(a, b) {
            i += 1;
            continue;
        }
        if remaining_right > remaining_left && distance(a, right[j + 1]) < distance(a, b) {
            j += 1;
            continue;
        }

        if !ctx.matching.is_src_matched(a)
            && !ctx.matching.is_dst_matched(b)
            && ctx.isomorphic(a, b)
        {
            ctx.map_isomorphic_subtrees(a, b);
        }
        i += 1;
        j += 1;
    }
}
