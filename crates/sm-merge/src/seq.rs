//! Sequence alignment and the diff3 chunker used for child lists.
//!
//! Two pieces:
//!
//! 1. [`align`] — a longest-common-subsequence alignment between two key
//!    sequences. Identity is the key, and the key of an element is the *base
//!    node it descends from*, not its text or its position (SPEC.md §4.5). So
//!    a statement that was edited in place still aligns with its ancestor, and
//!    two textually identical statements do not get confused for one another.
//! 2. [`diff3`] — the classic three-way chunker: align base↔ours and
//!    base↔theirs, take the base positions both alignments agree on as sync
//!    points, and classify each region between sync points.
//!
//! # The alignment budget
//!
//! The LCS table is `O(|a| · |b|)`. Real child lists are tens of elements, but
//! generated code exists, so above [`crate::MergeConfig::alignment_budget`]
//! cells the aligner falls back to a patience-style alignment: keys that occur
//! exactly once in each sequence are candidate anchors, the longest increasing
//! subsequence of those is taken, and the runs between anchors are matched by
//! common prefix and suffix. That is strictly weaker — it finds fewer pairs —
//! which costs conflicts, never correctness.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

/// One region of a three-way sequence merge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Hunk {
    pub kind: HunkKind,
    pub base: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
}

/// How a region was resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HunkKind {
    /// All three agree. Emit the base region (from whichever side is framing).
    Stable,
    /// Only we changed it. Emit our region.
    OursOnly,
    /// Only they changed it. Emit their region.
    TheirsOnly,
    /// Both changed it the same way. Emit our region.
    BothSame,
    /// Both changed it, differently.
    Conflicting,
}

/// A longest-common-subsequence alignment: pairs `(i, j)` with `a[i] == b[j]`,
/// strictly increasing in both coordinates.
pub(crate) fn align<K: Eq + Hash + Clone>(a: &[K], b: &[K], budget: usize) -> Vec<(usize, usize)> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    if a.len().saturating_mul(b.len()) <= budget {
        lcs_dp(a, b)
    } else {
        patience(a, b)
    }
}

/// Textbook LCS by dynamic programming, then a backtrack.
fn lcs_dp<K: Eq>(a: &[K], b: &[K]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    // `table[i * (m + 1) + j]` = LCS length of `a[i..]` and `b[j..]`.
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if a[i] == b[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Patience-style alignment for sequences too large for the DP table.
fn patience<K: Eq + Hash + Clone>(a: &[K], b: &[K]) -> Vec<(usize, usize)> {
    let mut counts_a: HashMap<&K, (usize, usize)> = HashMap::new();
    for (i, k) in a.iter().enumerate() {
        let e = counts_a.entry(k).or_insert((0, 0));
        e.0 += 1;
        e.1 = i;
    }
    let mut counts_b: HashMap<&K, (usize, usize)> = HashMap::new();
    for (j, k) in b.iter().enumerate() {
        let e = counts_b.entry(k).or_insert((0, 0));
        e.0 += 1;
        e.1 = j;
    }

    // Unique-in-both keys, in `a` order.
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for (i, k) in a.iter().enumerate() {
        if counts_a.get(k).is_some_and(|c| c.0 == 1)
            && let Some(&(count, j)) = counts_b.get(k)
            && count == 1
        {
            candidates.push((i, j));
        }
    }

    // Longest increasing subsequence on the `b` coordinate. `tails[l]` is the
    // smallest `b` index ending an increasing run of length `l + 1`.
    let mut tails: Vec<usize> = Vec::new();
    let mut back: Vec<Option<usize>> = vec![None; candidates.len()];
    let mut tail_idx: Vec<usize> = Vec::new();
    for (c, &(_, j)) in candidates.iter().enumerate() {
        let pos = tails.partition_point(|&t| t < j);
        if pos == tails.len() {
            tails.push(j);
            tail_idx.push(c);
        } else {
            tails[pos] = j;
            tail_idx[pos] = c;
        }
        back[c] = if pos == 0 {
            None
        } else {
            Some(tail_idx[pos - 1])
        };
    }
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    let mut cur = tail_idx.last().copied();
    while let Some(c) = cur {
        anchors.push(candidates[c]);
        cur = back[c];
    }
    anchors.reverse();

    // Fill the runs between anchors by common prefix and suffix.
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut prev = (0usize, 0usize);
    for &(ai, bj) in anchors.iter().chain(std::iter::once(&(a.len(), b.len()))) {
        fill_run(a, b, prev.0..ai, prev.1..bj, &mut out);
        if ai < a.len() && bj < b.len() {
            out.push((ai, bj));
        }
        prev = (ai + 1, bj + 1);
    }
    out
}

fn fill_run<K: Eq>(
    a: &[K],
    b: &[K],
    ra: Range<usize>,
    rb: Range<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let (mut i, mut j) = (ra.start, rb.start);
    while i < ra.end && j < rb.end && a[i] == b[j] {
        out.push((i, j));
        i += 1;
        j += 1;
    }
    let (mut x, mut y) = (ra.end, rb.end);
    let mut suffix = Vec::new();
    while x > i && y > j && a[x - 1] == b[y - 1] {
        x -= 1;
        y -= 1;
        suffix.push((x, y));
    }
    suffix.reverse();
    out.extend(suffix);
}

/// The three-way chunker.
///
/// Sync points are base indices that *both* alignments map, which makes the
/// triples monotone in all three coordinates for free (each alignment is
/// individually monotone). Everything between two sync points is one region,
/// classified by comparing the three slices.
pub(crate) fn diff3<K: Eq + Hash + Clone>(
    base: &[K],
    ours: &[K],
    theirs: &[K],
    budget: usize,
) -> Vec<Hunk> {
    let mut to_ours: Vec<Option<usize>> = vec![None; base.len()];
    for (i, j) in align(base, ours, budget) {
        to_ours[i] = Some(j);
    }
    let mut to_theirs: Vec<Option<usize>> = vec![None; base.len()];
    for (i, j) in align(base, theirs, budget) {
        to_theirs[i] = Some(j);
    }

    let syncs: Vec<(usize, usize, usize)> = (0..base.len())
        .filter_map(|i| Some((i, to_ours[i]?, to_theirs[i]?)))
        .collect();

    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut bi, mut oi, mut ti) = (0usize, 0usize, 0usize);
    for &(b, o, t) in &syncs {
        push_region(&mut hunks, base, ours, theirs, bi..b, oi..o, ti..t);
        // Merge consecutive sync points into one Stable hunk.
        match hunks.last_mut() {
            Some(h) if h.kind == HunkKind::Stable && h.base.end == b => {
                h.base.end = b + 1;
                h.ours.end = o + 1;
                h.theirs.end = t + 1;
            }
            _ => hunks.push(Hunk {
                kind: HunkKind::Stable,
                base: b..b + 1,
                ours: o..o + 1,
                theirs: t..t + 1,
            }),
        }
        bi = b + 1;
        oi = o + 1;
        ti = t + 1;
    }
    push_region(
        &mut hunks,
        base,
        ours,
        theirs,
        bi..base.len(),
        oi..ours.len(),
        ti..theirs.len(),
    );
    hunks
}

#[allow(clippy::too_many_arguments)]
fn push_region<K: Eq>(
    hunks: &mut Vec<Hunk>,
    base: &[K],
    ours: &[K],
    theirs: &[K],
    b: Range<usize>,
    o: Range<usize>,
    t: Range<usize>,
) {
    if b.is_empty() && o.is_empty() && t.is_empty() {
        return;
    }
    let ours_same = base[b.clone()] == ours[o.clone()];
    let theirs_same = base[b.clone()] == theirs[t.clone()];
    let kind = match (ours_same, theirs_same) {
        (true, true) => HunkKind::Stable,
        (true, false) => HunkKind::TheirsOnly,
        (false, true) => HunkKind::OursOnly,
        (false, false) if ours[o.clone()] == theirs[t.clone()] => HunkKind::BothSame,
        (false, false) => HunkKind::Conflicting,
    };
    hunks.push(Hunk {
        kind,
        base: b,
        ours: o,
        theirs: t,
    });
}

#[cfg(test)]
mod tests {
    use super::{HunkKind, align, diff3};

    fn keys(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn kinds(base: &str, ours: &str, theirs: &str) -> Vec<(HunkKind, String, String, String)> {
        let (b, o, t) = (keys(base), keys(ours), keys(theirs));
        diff3(&b, &o, &t, 1 << 20)
            .into_iter()
            .map(|h| {
                (
                    h.kind,
                    b[h.base].iter().collect(),
                    o[h.ours].iter().collect(),
                    t[h.theirs].iter().collect(),
                )
            })
            .collect()
    }

    #[test]
    fn lcs_finds_a_monotone_common_subsequence() {
        let pairs = align(&keys("abcde"), &keys("axcye"), 1 << 20);
        let picked: String = pairs.iter().map(|&(i, _)| keys("abcde")[i]).collect();
        assert_eq!(picked, "ace");
        for w in pairs.windows(2) {
            assert!(w[0].0 < w[1].0 && w[0].1 < w[1].1);
        }
    }

    #[test]
    fn identical_sequences_are_one_stable_hunk() {
        assert_eq!(
            kinds("abc", "abc", "abc"),
            vec![(HunkKind::Stable, "abc".into(), "abc".into(), "abc".into())]
        );
    }

    #[test]
    fn a_one_sided_insertion_is_taken() {
        let h = kinds("ac", "abc", "ac");
        assert!(h.contains(&(HunkKind::OursOnly, String::new(), "b".into(), String::new())));
        let h = kinds("ac", "ac", "abc");
        assert!(h.contains(&(
            HunkKind::TheirsOnly,
            String::new(),
            String::new(),
            "b".into()
        )));
    }

    #[test]
    fn disjoint_insertions_both_apply() {
        let h = kinds("ac", "abc", "acd");
        assert!(h.iter().all(|(k, ..)| *k != HunkKind::Conflicting));
        assert!(h.contains(&(HunkKind::OursOnly, String::new(), "b".into(), String::new())));
        assert!(h.contains(&(
            HunkKind::TheirsOnly,
            String::new(),
            String::new(),
            "d".into()
        )));
    }

    #[test]
    fn insertions_at_the_same_anchor_conflict() {
        let h = kinds("ac", "abc", "axc");
        assert!(h.contains(&(HunkKind::Conflicting, String::new(), "b".into(), "x".into())));
    }

    #[test]
    fn identical_insertions_at_the_same_anchor_are_both_same() {
        let h = kinds("ac", "abc", "abc");
        assert!(h.contains(&(HunkKind::BothSame, String::new(), "b".into(), "b".into())));
    }

    #[test]
    fn one_sided_deletion_is_taken() {
        let h = kinds("abc", "ac", "abc");
        assert!(h.contains(&(HunkKind::OursOnly, "b".into(), String::new(), "b".into())));
    }

    #[test]
    fn patience_fallback_still_aligns_the_common_run() {
        // Budget of 1 forces the fallback on anything longer than a single item.
        let a = keys("abcdefgh");
        let b = keys("abcXefgh");
        let pairs = align(&a, &b, 1);
        let picked: String = pairs.iter().map(|&(i, _)| a[i]).collect();
        assert_eq!(picked, "abcefgh");
        for w in pairs.windows(2) {
            assert!(w[0].0 < w[1].0 && w[0].1 < w[1].1);
        }
    }

    #[test]
    fn empty_inputs_are_handled() {
        assert!(align::<char>(&[], &[], 16).is_empty());
        assert!(diff3::<char>(&[], &[], &[], 16).is_empty());
        assert_eq!(
            kinds("", "a", ""),
            vec![(HunkKind::OursOnly, String::new(), "a".into(), String::new())]
        );
    }
}
