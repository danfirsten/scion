//! Fate classification: what one branch did to one base node.
//!
//! # The definitions, exactly
//!
//! Let `b` be a participating base node and `s = M(b)` its image under the
//! base→side matching (`None` if the matcher found no counterpart).
//!
//! - **content changed** — `full_hash(b) != full_hash(s)`. That is
//!   [`sm_match::TreeMetrics::hash`] (code modulo whitespace and comments)
//!   combined with a hash over every comment in the subtree and every comment
//!   attached to the node. Reindenting is therefore *not* a content change;
//!   rewriting a comment *is*. See [`crate::tables`].
//! - **moved** — either **reparented** (`s`'s participating parent is not the
//!   image of `b`'s participating parent) or **reordered** (same parent, but
//!   `b` is not on the longest increasing subsequence of the co-matched
//!   siblings' positions). Reordered is measured against the LIS rather than
//!   against raw indices so that inserting a sibling above `b` does not report
//!   `b` as moved.
//!
//! | matched? | content changed | moved | fate |
//! |---|---|---|---|
//! | no | — | — | [`Fate::Deleted`] |
//! | yes | no | no | [`Fate::Unchanged`] |
//! | yes | yes | no | [`Fate::Updated`] |
//! | yes | no | yes | [`Fate::Moved`] |
//! | yes | yes | yes | [`Fate::MovedAndUpdated`] |
//!
//! # What the fate is and is not used for
//!
//! The fate of a node is a *report*, not the merge algorithm's control flow.
//! The merge is a recursive descent, and it re-derives the same facts locally,
//! at higher precision, as it goes: it distinguishes a whitespace-only change
//! from no change at all, which the fate table cannot express, and it resolves
//! position through the parent's child-list merge rather than through a global
//! move set. Fates are computed because SPEC.md §4.5 specifies them, because
//! M5's report wants the histogram, and because they make the merge's decisions
//! checkable from the outside — the scenario tests assert on them.

use serde::{Deserialize, Serialize};
use sm_cst::NodeId;
use sm_match::Matching;

use crate::model::FateCounts;
use crate::tables::SideTables;

/// What one branch did to one base node (SPEC.md §4.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fate {
    /// Same content, same place.
    Unchanged,
    /// Same place, different content.
    Updated,
    /// Same content, different place.
    Moved,
    /// Different content *and* different place. The two components are
    /// resolved independently — see the crate docs' decision table.
    MovedAndUpdated,
    /// No counterpart on this side.
    Deleted,
    /// Not a participating node (an anonymous token, a comment, a `MISSING`
    /// repair), so the question does not apply.
    NotApplicable,
}

impl Fate {
    /// A short stable identifier, for histograms and snapshot tests.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Updated => "updated",
            Self::Moved => "moved",
            Self::MovedAndUpdated => "moved_and_updated",
            Self::Deleted => "deleted",
            Self::NotApplicable => "n/a",
        }
    }

    /// Whether this side changed the node's content.
    #[must_use]
    pub const fn is_updated(self) -> bool {
        matches!(self, Self::Updated | Self::MovedAndUpdated)
    }

    /// Whether this side changed the node's position.
    #[must_use]
    pub const fn is_moved(self) -> bool {
        matches!(self, Self::Moved | Self::MovedAndUpdated)
    }
}

impl std::fmt::Display for Fate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.tag())
    }
}

/// Classify every base node's fate on one side.
///
/// The result is indexed by base [`NodeId`]; non-participating nodes get
/// [`Fate::NotApplicable`].
pub(crate) fn classify(
    base: &SideTables<'_>,
    side: &SideTables<'_>,
    matching: &Matching,
) -> Vec<Fate> {
    let mut out = vec![Fate::NotApplicable; base.tree.len()];
    let moved = moved_set(base, side, matching);

    for id in base.tree.ids() {
        if !base.metrics.participates(id) {
            continue;
        }
        out[id.index()] = match matching.dst_of(id) {
            None => Fate::Deleted,
            Some(s) => {
                let updated = base.full_hash(id) != side.full_hash(s);
                match (updated, moved[id.index()]) {
                    (false, false) => Fate::Unchanged,
                    (true, false) => Fate::Updated,
                    (false, true) => Fate::Moved,
                    (true, true) => Fate::MovedAndUpdated,
                }
            }
        };
    }
    out
}

/// Which base nodes changed position on this side.
///
/// Reparenting is a direct comparison. Reordering is measured per container:
/// take the base container's children that are matched into the *same* side
/// container, read off their positions there, and mark everything outside the
/// longest increasing subsequence as moved. That is the standard Chawathe/
/// GumTree move rule, and it is what stops "a sibling was inserted above me"
/// from reading as a move.
fn moved_set(base: &SideTables<'_>, side: &SideTables<'_>, matching: &Matching) -> Vec<bool> {
    let mut moved = vec![false; base.tree.len()];

    for container in base.tree.ids() {
        if !base.metrics.participates(container) {
            continue;
        }
        let children = base.metrics.children(container);
        if children.is_empty() {
            continue;
        }
        let side_container = matching.dst_of(container);

        // (base child, its index in the side container) for children that
        // stayed under this container.
        let mut kept: Vec<(NodeId, u32)> = Vec::new();
        for &child in children {
            let Some(image) = matching.dst_of(child) else {
                continue;
            };
            if side.metrics.parent(image) == side_container && side_container.is_some() {
                kept.push((child, side.metrics.index_in_parent(image)));
            } else {
                moved[child.index()] = true;
            }
        }

        for &(child, _) in &kept {
            moved[child.index()] = false;
        }
        for child in longest_increasing_complement(&kept) {
            moved[child.index()] = true;
        }
    }

    moved
}

/// The elements *not* on a longest increasing subsequence of the second
/// coordinate.
fn longest_increasing_complement(kept: &[(NodeId, u32)]) -> Vec<NodeId> {
    if kept.len() < 2 {
        return Vec::new();
    }
    let mut tails: Vec<u32> = Vec::new();
    let mut tail_idx: Vec<usize> = Vec::new();
    let mut back: Vec<Option<usize>> = vec![None; kept.len()];
    for (i, &(_, pos)) in kept.iter().enumerate() {
        let at = tails.partition_point(|&t| t < pos);
        if at == tails.len() {
            tails.push(pos);
            tail_idx.push(i);
        } else {
            tails[at] = pos;
            tail_idx[at] = i;
        }
        back[i] = if at == 0 {
            None
        } else {
            Some(tail_idx[at - 1])
        };
    }
    let mut on_lis = vec![false; kept.len()];
    let mut cur = tail_idx.last().copied();
    while let Some(i) = cur {
        on_lis[i] = true;
        cur = back[i];
    }
    kept.iter()
        .zip(&on_lis)
        .filter_map(|(&(id, _), &keep)| (!keep).then_some(id))
        .collect()
}

/// Tally a fate vector.
#[must_use]
pub(crate) fn count(fates: &[Fate]) -> FateCounts {
    let mut c = FateCounts::default();
    for f in fates {
        match f {
            Fate::Unchanged => c.unchanged += 1,
            Fate::Updated => c.updated += 1,
            Fate::Moved => c.moved += 1,
            Fate::MovedAndUpdated => c.moved_and_updated += 1,
            Fate::Deleted => c.deleted += 1,
            Fate::NotApplicable => {}
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::longest_increasing_complement;
    use sm_cst::NodeId;

    fn complement(positions: &[u32]) -> Vec<u32> {
        let kept: Vec<(NodeId, u32)> = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| (NodeId(i as u32), p))
            .collect();
        longest_increasing_complement(&kept)
            .into_iter()
            .map(|id| id.0)
            .collect()
    }

    #[test]
    fn an_order_preserving_list_moves_nothing() {
        assert!(complement(&[0, 1, 2, 3]).is_empty());
        // Positions shifted by an insertion above are still increasing.
        assert!(complement(&[1, 2, 5]).is_empty());
    }

    #[test]
    fn a_swap_moves_exactly_one_element() {
        assert_eq!(complement(&[1, 0, 2]).len(), 1);
    }

    #[test]
    fn moving_one_element_to_the_end_moves_only_it() {
        assert_eq!(complement(&[2, 0, 1]), vec![0]);
    }
}
