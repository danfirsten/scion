//! The [`Matching`] type: a bidirectional, injective `NodeId` ↔ `NodeId` map.

use std::fmt::Write as _;

use sm_cst::{NodeId, SourceTree};

/// A partial bijection between the nodes of two trees.
///
/// This is the contract M3 (edit scripts) and M4 (merge) code against. The
/// guarantees, all of which the test suite asserts:
///
/// 1. **Injective both ways.** A node appears in at most one pair.
/// 2. **Kind-equal.** Both members of a pair have the same `kind`. Every path
///    that can insert a pair enforces this — see [`crate::match_trees`].
/// 3. **Deterministic.** Two runs of [`crate::match_trees`] on the same inputs
///    produce the identical set of pairs, in the identical order.
/// 4. **Participating nodes only.** Anonymous tokens, comments and `MISSING`
///    nodes are never members of a pair. See the [`crate::metrics`] module
///    docs for why, and what M3/M4 should do instead.
///
/// Backed by two `Vec<Option<NodeId>>` sized to the two arenas, which is the
/// side-table pattern `sm-cst`'s dense preorder IDs are designed for: lookups
/// are an array index, and there is no hash iteration order to be
/// non-deterministic about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matching {
    src_to_dst: Vec<Option<NodeId>>,
    dst_to_src: Vec<Option<NodeId>>,
    len: usize,
}

impl Matching {
    /// An empty matching sized for two arenas of `src_len` and `dst_len` nodes.
    #[must_use]
    pub fn new(src_len: usize, dst_len: usize) -> Self {
        Self {
            src_to_dst: vec![None; src_len],
            dst_to_src: vec![None; dst_len],
            len: 0,
        }
    }

    /// The node in the destination tree that `src` is matched to.
    #[must_use]
    pub fn dst_of(&self, src: NodeId) -> Option<NodeId> {
        *self.src_to_dst.get(src.index())?
    }

    /// The node in the source tree that `dst` is matched to.
    #[must_use]
    pub fn src_of(&self, dst: NodeId) -> Option<NodeId> {
        *self.dst_to_src.get(dst.index())?
    }

    /// Every matched pair, in ascending source-ID order.
    ///
    /// Because IDs are assigned in preorder, this is also a topological order:
    /// a matched parent is always yielded before its matched descendants.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, NodeId)> + '_ {
        self.src_to_dst
            .iter()
            .enumerate()
            .filter_map(|(i, dst)| dst.map(|d| (NodeId(i as u32), d)))
    }

    /// Number of matched pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is matched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether `src` is already matched.
    #[must_use]
    pub fn is_src_matched(&self, src: NodeId) -> bool {
        self.dst_of(src).is_some()
    }

    /// Whether `dst` is already matched.
    #[must_use]
    pub fn is_dst_matched(&self, dst: NodeId) -> bool {
        self.src_of(dst).is_some()
    }

    /// Record `src` ↔ `dst`.
    ///
    /// Returns `false` and changes nothing if either endpoint is already
    /// matched: injectivity is enforced here rather than assumed by callers,
    /// so that no amount of greedy search in the phases above can violate it.
    pub fn insert(&mut self, src: NodeId, dst: NodeId) -> bool {
        if self.is_src_matched(src) || self.is_dst_matched(dst) {
            return false;
        }
        self.src_to_dst[src.index()] = Some(dst);
        self.dst_to_src[dst.index()] = Some(src);
        self.len += 1;
        true
    }

    /// The inverse matching, i.e. `match(b, a)` derived from `match(a, b)`.
    ///
    /// Used by the symmetry tests and by anything that needs to walk from the
    /// destination side without paying for a second run.
    #[must_use]
    pub fn inverted(&self) -> Self {
        Self {
            src_to_dst: self.dst_to_src.clone(),
            dst_to_src: self.src_to_dst.clone(),
            len: self.len,
        }
    }
}

/// Render a matching as a stable, sorted, one-pair-per-line text form.
///
/// The format is `src_id:kind@start..end <-> dst_id:kind@start..end`, sorted by
/// source ID. This is what the snapshot tests assert on: it is diffable, it
/// carries enough context to see *what* moved where, and it contains nothing
/// that varies between runs.
#[must_use]
pub fn render_pairs(matching: &Matching, src: &SourceTree, dst: &SourceTree) -> String {
    let mut out = String::with_capacity(matching.len() * 64);
    for (a, b) in matching.iter() {
        let na = src.node(a);
        let nb = dst.node(b);
        let _ = writeln!(
            out,
            "{}:{}@{}..{} <-> {}:{}@{}..{}",
            a.0,
            na.kind,
            na.byte_range.start,
            na.byte_range.end,
            b.0,
            nb.kind,
            nb.byte_range.start,
            nb.byte_range.end,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Matching;
    use sm_cst::NodeId;

    #[test]
    fn insert_is_injective_in_both_directions() {
        let mut m = Matching::new(4, 4);
        assert!(m.insert(NodeId(1), NodeId(2)));
        assert!(!m.insert(NodeId(1), NodeId(3)), "src already matched");
        assert!(!m.insert(NodeId(3), NodeId(2)), "dst already matched");
        assert_eq!(m.len(), 1);
        assert_eq!(m.dst_of(NodeId(1)), Some(NodeId(2)));
        assert_eq!(m.src_of(NodeId(2)), Some(NodeId(1)));
        assert_eq!(m.dst_of(NodeId(3)), None);
    }

    #[test]
    fn iter_is_ascending_in_src_order() {
        let mut m = Matching::new(8, 8);
        for (a, b) in [(5u32, 1u32), (2, 7), (0, 0)] {
            assert!(m.insert(NodeId(a), NodeId(b)));
        }
        let pairs: Vec<_> = m.iter().collect();
        assert_eq!(
            pairs,
            vec![
                (NodeId(0), NodeId(0)),
                (NodeId(2), NodeId(7)),
                (NodeId(5), NodeId(1)),
            ]
        );
    }

    #[test]
    fn out_of_range_lookups_are_none_not_panics() {
        let m = Matching::new(2, 2);
        assert_eq!(m.dst_of(NodeId(99)), None);
        assert_eq!(m.src_of(NodeId(99)), None);
        assert!(m.is_empty());
    }

    #[test]
    fn inverted_swaps_both_directions() {
        let mut m = Matching::new(3, 3);
        assert!(m.insert(NodeId(0), NodeId(2)));
        let inv = m.inverted();
        assert_eq!(inv.dst_of(NodeId(2)), Some(NodeId(0)));
        assert_eq!(inv.src_of(NodeId(0)), Some(NodeId(2)));
        assert_eq!(inv.len(), 1);
    }
}
