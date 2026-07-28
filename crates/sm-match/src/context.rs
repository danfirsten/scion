//! The state both matching phases share.

use sm_cst::{Language, NodeId, SourceTree};

use crate::config::MatchConfig;
use crate::matching::Matching;
use crate::metrics::{TreeMetrics, structurally_equal};

/// Everything the two phases operate on, gathered so neither phase has to take
/// eight parameters.
pub(crate) struct Ctx<'a> {
    pub(crate) src: &'a SourceTree,
    pub(crate) dst: &'a SourceTree,
    pub(crate) sm: &'a TreeMetrics,
    pub(crate) dm: &'a TreeMetrics,
    pub(crate) lang: &'a dyn Language,
    pub(crate) cfg: &'a MatchConfig,
    pub(crate) matching: Matching,
}

impl Ctx<'_> {
    /// Whether these two subtrees are isomorphic.
    ///
    /// Hash equality first (one comparison), then a full structural walk. See
    /// [`structurally_equal`] for why the second step is worth its cost.
    pub(crate) fn isomorphic(&self, a: NodeId, b: NodeId) -> bool {
        self.sm.hash(a) == self.dm.hash(b)
            && structurally_equal(self.src, self.sm, self.dst, self.dm, self.lang, a, b)
    }

    /// Map two isomorphic subtrees onto each other, node for node.
    ///
    /// Only ever called on a pair [`Ctx::isomorphic`] has approved, so the
    /// projected child lists are the same length at every level and the zip
    /// below is total. Returns the number of pairs recorded.
    pub(crate) fn map_isomorphic_subtrees(&mut self, a: NodeId, b: NodeId) -> usize {
        let mut recorded = 0;
        let mut stack = vec![(a, b)];
        while let Some((x, y)) = stack.pop() {
            if self.matching.insert(x, y) {
                recorded += 1;
            }
            let cx = self.sm.children(x);
            let cy = self.dm.children(y);
            debug_assert_eq!(
                cx.len(),
                cy.len(),
                "map_isomorphic_subtrees called on a non-isomorphic pair"
            );
            stack.extend(cx.iter().copied().zip(cy.iter().copied()));
        }
        recorded
    }

    /// Dice similarity over already-matched descendants:
    /// `2·|shared| / (|desc(a)| + |desc(b)|)`.
    ///
    /// `|desc|` counts *participating* descendants only, consistently with
    /// everything else in this crate. Returns 0 when both subtrees are leaves,
    /// which would otherwise be `0/0`.
    pub(crate) fn dice(&self, a: NodeId, b: NodeId) -> f64 {
        let total = self.sm.descendant_count(a) + self.dm.descendant_count(b);
        if total == 0 {
            return 0.0;
        }
        let mut shared = 0u32;
        for raw in self.sm.subtree_range(a).skip(1) {
            let d = NodeId(raw);
            if !self.sm.participates(d) {
                continue;
            }
            if let Some(m) = self.matching.dst_of(d)
                && self.dm.contains(b, m)
            {
                shared += 1;
            }
        }
        2.0 * f64::from(shared) / f64::from(total)
    }

    /// Dice over the two nodes' already-matched *ancestors*.
    ///
    /// This is SPEC.md §4.3's tiebreak for the top-down phase ("disambiguate by
    /// parent-context similarity (dice of already-matched ancestors)"). It is
    /// cheap — the chains are `O(depth)` — and it is the only signal available
    /// during a phase that has, by construction, matched nothing below the
    /// nodes in question.
    pub(crate) fn ancestor_dice(&self, a: NodeId, b: NodeId) -> f64 {
        let mut total = 0u32;
        let mut shared = 0u32;
        for p in self.sm.ancestors(a) {
            total += 1;
            if let Some(m) = self.matching.dst_of(p)
                && self.dm.contains(m, b)
            {
                shared += 1;
            }
        }
        for q in self.dm.ancestors(b) {
            total += 1;
            if let Some(m) = self.matching.src_of(q)
                && self.sm.contains(m, a)
            {
                shared += 1;
            }
        }
        if total == 0 {
            return 0.0;
        }
        f64::from(shared) / f64::from(total)
    }

    /// Whether these two nodes have the same grammar kind.
    ///
    /// Kind equality is a *hard* precondition everywhere in this crate, not a
    /// tiebreak: GumTree, Mergiraf and Spork all agree on that
    /// (docs/prior-art.md §7, §8.1.3). Compared as strings rather than as
    /// `kind_id`s so the check stays honest even if the two trees somehow came
    /// from different grammar tables.
    pub(crate) fn same_kind(&self, a: NodeId, b: NodeId) -> bool {
        self.src.node(a).kind == self.dst.node(b).kind
    }
}
