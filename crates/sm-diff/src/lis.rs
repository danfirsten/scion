//! Longest strictly increasing subsequence, with reconstruction.
//!
//! This is what keeps reorder reporting *minimal*. A container whose matched
//! children were permuted has, in general, many possible explanations as a set
//! of moves; the one a human recognises is "these few elements moved, the rest
//! stayed". Formally: keep a maximum-size subsequence that is already in the
//! right relative order, and call everything else a move. Moving one element out
//! of ten then costs one `Move`, not nine — which is the whole difference
//! between an edit script a person can read and one they cannot.
//!
//! Patience sorting, `O(n log n)`. Deterministic: `partition_point` always
//! returns the leftmost pile whose tail is `>= x`, so the tails array, the
//! predecessor links and therefore the reconstructed subsequence are a pure
//! function of the input.

/// Indices of a longest strictly increasing subsequence of `values`, ascending.
///
/// `values` must be distinct for the result to be unique in the sense the
/// caller wants; with duplicates the answer is still deterministic, just not
/// the only maximum.
#[must_use]
pub fn longest_increasing_subsequence(values: &[u32]) -> Vec<usize> {
    if values.is_empty() {
        return Vec::new();
    }

    // `tails[k]` = index into `values` of the smallest possible tail of an
    // increasing subsequence of length `k + 1` seen so far.
    let mut tails: Vec<usize> = Vec::with_capacity(values.len());
    // `prev[i]` = index of the element before `i` in the best subsequence
    // ending at `i`.
    let mut prev: Vec<Option<usize>> = vec![None; values.len()];

    for (i, &x) in values.iter().enumerate() {
        let pos = tails.partition_point(|&t| values[t] < x);
        prev[i] = if pos == 0 { None } else { Some(tails[pos - 1]) };
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }

    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = prev[i];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::longest_increasing_subsequence as lis;

    #[test]
    fn empty_and_singleton() {
        assert!(lis(&[]).is_empty());
        assert_eq!(lis(&[7]), vec![0]);
    }

    #[test]
    fn already_sorted_keeps_everything() {
        assert_eq!(lis(&[0, 1, 2, 3, 4]), vec![0, 1, 2, 3, 4]);
    }

    /// The property the `Move` classifier depends on: one element out of ten
    /// leaves nine in place.
    #[test]
    fn one_element_moved_to_the_front_costs_one() {
        // The tenth element moved to the front: its dst ranks read 9,0,1,…,8.
        let seq: Vec<u32> = std::iter::once(9).chain(0..9).collect();
        let keep = lis(&seq);
        assert_eq!(keep.len(), 9, "kept {keep:?}");
        assert!(!keep.contains(&0), "the moved element must not be kept");
    }

    #[test]
    fn one_element_moved_to_the_back_costs_one() {
        let seq: Vec<u32> = (1..10).chain(std::iter::once(0)).collect();
        let keep = lis(&seq);
        assert_eq!(keep.len(), 9);
        assert!(!keep.contains(&9));
    }

    #[test]
    fn a_full_reversal_keeps_exactly_one() {
        assert_eq!(lis(&[4, 3, 2, 1, 0]).len(), 1);
    }

    #[test]
    fn is_deterministic_under_ties() {
        let seq = [3, 1, 4, 1, 5, 9, 2, 6];
        assert_eq!(lis(&seq), lis(&seq));
    }
}
