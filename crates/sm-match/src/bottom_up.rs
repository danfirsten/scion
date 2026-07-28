//! Phase 2: bottom-up container matching and the recovery pass (SPEC.md §4.3).
//!
//! Phase 1 matches things that are *identical*. Phase 2 matches the containers
//! that hold them: a method whose body changed, a class that gained a field, a
//! block that got wrapped in an `if`. It walks the source tree in post-order,
//! and for each still-unmatched internal node looks for the destination node
//! that shares the most already-matched descendants with it.
//!
//! Candidate selection follows all three reference implementations
//! (docs/prior-art.md §7): walk up from every matched descendant, and admit an
//! ancestor only if its **kind is equal** to the source node's, it is
//! unmatched, and it is not the root. Kind equality is a hard precondition, not
//! a tiebreak. The winner is the candidate with the highest dice, and it is
//! taken only if that dice exceeds [`crate::MatchConfig::min_dice`].

use std::collections::BTreeMap;

use sm_cst::NodeId;

use crate::context::Ctx;

pub(crate) fn run(ctx: &mut Ctx<'_>) {
    let (Some(src_root), Some(dst_root)) = (ctx.sm.root(), ctx.dm.root()) else {
        return;
    };
    let sm = ctx.sm;

    // A generation-stamped visited set over the destination arena. Walking up
    // from every matched descendant would otherwise re-tread the same ancestor
    // chain once per descendant; stopping at the first already-stamped node
    // makes the whole candidate search linear in the destination subtree.
    // Stamps rather than a cleared bitset so that resetting is O(1) per source
    // node instead of O(|dst|).
    let mut seen = vec![0u32; ctx.dst.len()];
    let mut generation = 0u32;

    for &a in sm.post_order() {
        if a == src_root {
            continue;
        }
        if ctx.matching.is_src_matched(a) {
            continue;
        }
        // Internal nodes only. An unmatched leaf has no matched descendants to
        // vote for a container, so it can only be recovered by the pass below.
        if sm.children(a).is_empty() {
            continue;
        }

        generation += 1;
        let candidates = candidates_for(ctx, a, dst_root, &mut seen, generation);

        let mut best: Option<(NodeId, f64)> = None;
        for c in candidates {
            let d = ctx.dice(a, c);
            // Strictly-greater keeps the smallest candidate ID on a tie, which
            // is what makes the choice deterministic.
            if best.is_none_or(|(_, bd)| d > bd) {
                best = Some((c, d));
            }
        }

        if let Some((c, d)) = best
            && d > ctx.cfg.min_dice
            && ctx.matching.insert(a, c)
        {
            recover(ctx, a, c);
        }
    }

    // The roots. GumTree excludes the root from bottom-up candidate sets, so
    // without this two files that share no isomorphic subtree at all would come
    // back with an empty matching and no anchor for M4 to merge under. Two
    // compilation units are the same compilation unit; there is nothing else
    // for either root to be.
    if !ctx.matching.is_src_matched(src_root)
        && !ctx.matching.is_dst_matched(dst_root)
        && ctx.same_kind(src_root, dst_root)
        && ctx.matching.insert(src_root, dst_root)
    {
        recover(ctx, src_root, dst_root);
    }
}

/// Unmatched, same-kind, non-root ancestors of `a`'s matched descendants.
///
/// Returned sorted and deduplicated so that the caller's tie-break is stable.
fn candidates_for(
    ctx: &Ctx<'_>,
    a: NodeId,
    dst_root: NodeId,
    seen: &mut [u32],
    generation: u32,
) -> Vec<NodeId> {
    let kind = ctx.src.node(a).kind;
    let mut out = Vec::new();

    for raw in ctx.sm.subtree_range(a).skip(1) {
        let d = NodeId(raw);
        if !ctx.sm.participates(d) {
            continue;
        }
        let Some(m) = ctx.matching.dst_of(d) else {
            continue;
        };
        let mut cur = ctx.dm.parent(m);
        while let Some(c) = cur {
            if seen[c.index()] == generation {
                break;
            }
            seen[c.index()] = generation;
            if c != dst_root && !ctx.matching.is_dst_matched(c) && ctx.dst.node(c).kind == kind {
                out.push(c);
            }
            cur = ctx.dm.parent(c);
        }
    }

    out.sort_unstable();
    out.dedup();
    out
}

/// The "last chance" recovery pass over a freshly matched pair.
///
/// # What is implemented
///
/// Not a tree edit distance. Two histogram passes, applied to the pair's
/// **unmatched participating children** and then recursively to whatever they
/// match:
///
/// 1. **By structural hash.** Group the unmatched children of each side by
///    structural hash. Where a hash appears on both sides with *exactly one*
///    node on each, the two are isomorphic: verify and map the whole subtrees
///    pairwise. Nothing more to do inside them.
/// 2. **By kind.** Among what pass 1 left over, group by kind. Where a kind
///    appears on both sides with exactly one node on each, match that pair of
///    *nodes* and push it, so the same two passes run one level down.
///
/// "Exactly one on each side" is the whole safety argument, and it is the rule
/// GumTree's own `histogramMatching` uses: an assignment that is forced is not
/// a guess. Anything with two plausible partners is left unmatched rather than
/// resolved by position — which for a merge tool costs a conflict at worst,
/// where guessing wrong costs a corrupted file (SPEC.md §0.4).
///
/// # Deviations from the paper, stated plainly
///
/// The ASE'14 paper and GumTree's `gumtree-classic` run an exact Zhang-Shasha
/// tree edit distance here, and Mergiraf runs RTED; all three cap it with a
/// size threshold, which is what [`crate::MatchConfig::max_size`] is
/// (docs/prior-art.md §1.2, §2.2). We run the cheap pass instead — the same
/// family as `gumtree-simple`'s `lastChanceMatch`, which is GumTree v4's
/// *default* matcher. The size cap is kept because it still bounds the work and
/// because M5 has to be able to sweep it against a TED implementation if one is
/// ever added. This is the single largest deliberate divergence from the paper
/// in this crate.
fn recover(ctx: &mut Ctx<'_>, a: NodeId, c: NodeId) {
    if ctx.sm.subtree_size(a) >= ctx.cfg.max_size || ctx.dm.subtree_size(c) >= ctx.cfg.max_size {
        return;
    }
    let (sm, dm) = (ctx.sm, ctx.dm);
    let mut stack = vec![(a, c)];

    while let Some((x, y)) = stack.pop() {
        // Pass 1 — unique structural hash on both sides.
        let mut left: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
        for &n in sm.children(x) {
            if !ctx.matching.is_src_matched(n) {
                left.entry(sm.hash(n)).or_default().push(n);
            }
        }
        let mut right: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
        for &n in dm.children(y) {
            if !ctx.matching.is_dst_matched(n) {
                right.entry(dm.hash(n)).or_default().push(n);
            }
        }
        for (hash, l) in &left {
            if l.len() != 1 {
                continue;
            }
            let Some(r) = right.get(hash) else { continue };
            if r.len() == 1 && ctx.isomorphic(l[0], r[0]) {
                ctx.map_isomorphic_subtrees(l[0], r[0]);
            }
        }

        // Pass 2 — unique kind among whatever is still unmatched.
        let mut left_kinds: BTreeMap<u16, Vec<NodeId>> = BTreeMap::new();
        for &n in sm.children(x) {
            if !ctx.matching.is_src_matched(n) {
                left_kinds
                    .entry(ctx.src.node(n).kind_id)
                    .or_default()
                    .push(n);
            }
        }
        let mut right_kinds: BTreeMap<u16, Vec<NodeId>> = BTreeMap::new();
        for &n in dm.children(y) {
            if !ctx.matching.is_dst_matched(n) {
                right_kinds
                    .entry(ctx.dst.node(n).kind_id)
                    .or_default()
                    .push(n);
            }
        }
        for (kind_id, l) in &left_kinds {
            if l.len() != 1 {
                continue;
            }
            let Some(r) = right_kinds.get(kind_id) else {
                continue;
            };
            if r.len() == 1 && ctx.same_kind(l[0], r[0]) && ctx.matching.insert(l[0], r[0]) {
                stack.push((l[0], r[0]));
            }
        }
    }
}
