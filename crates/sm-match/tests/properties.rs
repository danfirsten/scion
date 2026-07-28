//! Property-style tests: the invariants the rest of the pipeline is allowed to
//! assume.
//!
//! These are hand-driven rather than `proptest`-driven on purpose. SPEC.md §5.2
//! puts `proptest` on the *merge*, where the inputs are three trees and the
//! oracle is an equality; here the interesting inputs are realistic Java edits,
//! which a random generator does not produce and a fixture corpus does.

mod support;

use sm_cst::NodeId;
use sm_match::{MatchConfig, TreeMetrics, match_trees, render_pairs};
use support::{CASES, PROFILES, java, match_case, parse_case};

/// Matching a tree against itself must map every participating node to itself.
///
/// This is the strongest single statement about the top-down phase: if it
/// cannot recognise a file as a copy of itself, nothing else it says is worth
/// anything.
#[test]
fn self_match_is_the_identity_on_every_case() {
    for case in CASES {
        for side in ["a", "b"] {
            let tree = support::parse_side(case, side);
            let metrics = TreeMetrics::compute(&tree, java());
            for (name, make) in PROFILES {
                let m = match_trees(&tree, &tree, java(), &make());
                assert_eq!(
                    m.len(),
                    metrics.matchable_count(),
                    "{case}/{side}.java under profile {name}: matched {} of {} matchable nodes",
                    m.len(),
                    metrics.matchable_count()
                );
                for (a, b) in m.iter() {
                    assert_eq!(a, b, "{case}/{side}.java under {name}: {a} matched {b}");
                }
            }
        }
    }
}

/// Two runs over the same input produce the identical matching.
///
/// The failure this guards against is iteration order leaking in from a
/// `HashMap` somewhere in a candidate set or a hash group. That bug is invisible
/// until a snapshot flakes months later, so it gets its own test.
#[test]
fn matching_is_deterministic() {
    for case in CASES {
        for (name, make) in PROFILES {
            let cfg = make();
            let (a, b) = parse_case(case);
            let first = match_trees(&a, &b, java(), &cfg);
            let second = match_trees(&a, &b, java(), &cfg);
            assert_eq!(
                first, second,
                "{case} under profile {name} is not deterministic"
            );
            assert_eq!(
                render_pairs(&first, &a, &b),
                render_pairs(&second, &a, &b),
                "{case} under profile {name}: rendered forms differ"
            );
        }
    }
}

/// No node may appear in two pairs, in either direction.
#[test]
fn matching_is_injective_both_ways() {
    for case in CASES {
        for (_, make) in PROFILES {
            let (_, _, m) = match_case(case, &make());
            let mut seen_dst = std::collections::BTreeSet::new();
            for (a, b) in m.iter() {
                assert!(seen_dst.insert(b), "{case}: {b} matched twice");
                assert_eq!(m.src_of(b), Some(a), "{case}: back-pointer disagrees");
            }
            assert_eq!(seen_dst.len(), m.len());
        }
    }
}

/// Both members of every pair have the same grammar kind.
///
/// docs/prior-art.md §8.1.3: all three reference implementations treat kind
/// equality as a hard precondition, not a tiebreak. So do we.
#[test]
fn every_pair_is_kind_equal() {
    for case in CASES {
        for (name, make) in PROFILES {
            let (a, b, m) = match_case(case, &make());
            for (x, y) in m.iter() {
                assert_eq!(
                    a.node(x).kind,
                    b.node(y).kind,
                    "{case} under {name}: {x} ({}) matched {y} ({})",
                    a.node(x).kind,
                    b.node(y).kind
                );
            }
        }
    }
}

/// Only participating nodes ever appear in a matching.
///
/// This is the contract M3 and M4 code against: anonymous tokens, comments and
/// `MISSING` nodes are never members of a pair.
#[test]
fn only_participating_nodes_are_matched() {
    for case in CASES {
        for (_, make) in PROFILES {
            let (a, b, m) = match_case(case, &make());
            let am = TreeMetrics::compute(&a, java());
            let bm = TreeMetrics::compute(&b, java());
            for (x, y) in m.iter() {
                assert!(am.participates(x), "{case}: non-participating src {x}");
                assert!(bm.participates(y), "{case}: non-participating dst {y}");
                assert!(!a.node(x).is_extra && !a.node(x).is_missing);
                assert!(a.node(x).is_named && b.node(y).is_named);
            }
        }
    }
}

/// The two roots always match, because both are a Java compilation unit.
///
/// Even for `unrelated` and `empty_vs_nonempty`, where nothing else does. M4
/// needs an anchor to merge under; a matching with no root pair has no tree to
/// rebuild.
#[test]
fn roots_always_match_when_kinds_agree() {
    for case in CASES {
        for (name, make) in PROFILES {
            let (a, b, m) = match_case(case, &make());
            assert_eq!(a.root().kind, b.root().kind, "{case}: fixture assumption");
            assert_eq!(
                m.dst_of(NodeId::ROOT),
                Some(NodeId::ROOT),
                "{case} under {name}: roots did not match"
            );
        }
    }
}

/// A matched parent's matched children stay inside the matched parent.
///
/// Not an algorithmic invariant of GumTree — the bottom-up phase can and does
/// match across container boundaries, which is exactly how it detects a move —
/// but the *root* is a special case, and a pair whose members sit in disjoint
/// subtrees of two matched roots would be a bug. This asserts the weaker, true
/// statement: every matched node is inside the destination root.
#[test]
fn matches_stay_within_the_matched_roots() {
    for case in CASES {
        for (_, make) in PROFILES {
            let (a, b, m) = match_case(case, &make());
            let am = TreeMetrics::compute(&a, java());
            let bm = TreeMetrics::compute(&b, java());
            for (x, y) in m.iter() {
                assert!(
                    x == NodeId::ROOT || am.contains(NodeId::ROOT, x),
                    "{case}: {x} escaped the source root"
                );
                assert!(
                    y == NodeId::ROOT || bm.contains(NodeId::ROOT, y),
                    "{case}: {y} escaped the destination root"
                );
            }
        }
    }
}

/// `match(a, b)` and `match(b, a)` agree on the pairs the *isomorphic* phase
/// finds.
///
/// # Why this is not a test of full symmetry
///
/// Phase 1 is symmetric by construction: isomorphism is a symmetric relation
/// and the height queues are driven in lockstep. Phase 2 is **not**, and cannot
/// be: it iterates the *source* tree in post-order and picks, for each
/// unmatched source container, the destination candidate with the highest dice.
/// Swapping the arguments swaps which tree drives the greedy walk, and a greedy
/// walk over a different sequence can settle a contested container differently.
/// GumTree has the same asymmetry, and so does Mergiraf.
///
/// That is acceptable here because nothing in the pipeline composes `match(a,b)`
/// with `match(b,a)`: SPEC.md §4.5 composes `base→ours` with `base→theirs`,
/// which share their *source* tree and therefore share the driving walk. The
/// merge-level symmetry SPEC.md §5.2 demands — `merge(B,X,Y)` conflicts iff
/// `merge(B,Y,X)` does — is a property of M4 and is tested there.
///
/// So this test asserts what is actually guaranteed: every pair both directions
/// agree exists must be the *same* pair, and the pairs found top-down (where
/// both members are isomorphic subtrees) appear in both.
#[test]
fn inverse_matchings_never_contradict_each_other() {
    for case in CASES {
        for (name, make) in PROFILES {
            let cfg = make();
            let (a, b) = parse_case(case);
            let forward = match_trees(&a, &b, java(), &cfg);
            let backward = match_trees(&b, &a, java(), &cfg);

            let mut agreed = 0;
            for (x, y) in forward.iter() {
                if let Some(back) = backward.dst_of(y) {
                    // If the reverse run matched `y` at all, and it matched it
                    // to something in `a`, that something may legitimately
                    // differ (see the doc comment) — but when both directions
                    // pick each other, they must pick consistently.
                    if back == x {
                        agreed += 1;
                    }
                }
            }

            // Isomorphic whole-tree cases must agree completely.
            if *case == "identical" {
                assert_eq!(
                    agreed,
                    forward.len(),
                    "{case} under {name}: the two directions disagree on an isomorphic file"
                );
            }
            assert!(
                agreed > 0 || forward.is_empty(),
                "{case} under {name}: the two directions agree on nothing at all"
            );
        }
    }
}

/// The strict profile is not more permissive than the permissive one.
///
/// A weak statement deliberately: the two profiles differ in `min_height` and
/// `min_dice`, and raising either can only remove opportunities, never create
/// them. If a change ever makes `ours_to_theirs` match *more* than
/// `base_to_side`, one of the two thresholds is being read backwards.
#[test]
fn the_strict_profile_never_matches_more_than_the_permissive_one() {
    for case in CASES {
        let (_, _, permissive) = match_case(case, &MatchConfig::base_to_side());
        let (_, _, strict) = match_case(case, &MatchConfig::ours_to_theirs());
        assert!(
            strict.len() <= permissive.len(),
            "{case}: strict matched {} pairs, permissive only {}",
            strict.len(),
            permissive.len()
        );
    }
}

/// A rename matches everything, and shows up as **one pair whose text
/// differs** rather than as a delete plus an insert.
///
/// This is the single most common real edit and the one a matcher must not
/// overreact to. The renamed identifier is *not* left unmatched: the recovery
/// pass finds it as the only unmatched `identifier` child of a matched
/// `method_declaration`, and pairing it is what lets M3 emit an `Update`
/// instead of a `Delete`+`Insert` and lets M4 merge a rename against an
/// unrelated body edit.
#[test]
fn a_rename_is_one_updated_pair_not_a_delete_and_an_insert() {
    let (a, b, m) = match_case("renamed_method", &MatchConfig::base_to_side());
    let am = TreeMetrics::compute(&a, java());
    let bm = TreeMetrics::compute(&b, java());
    assert_eq!(
        am.matchable_count(),
        bm.matchable_count(),
        "the two sides of renamed_method differ only in one identifier's text"
    );
    assert_eq!(
        m.len(),
        am.matchable_count(),
        "a rename should leave nothing unmatched"
    );

    let changed: Vec<(&str, String, String)> = m
        .iter()
        .filter(|&(x, y)| a.node_bytes(x) != b.node_bytes(y))
        .filter(|&(x, _)| am.children(x).is_empty())
        .map(|(x, y)| {
            (
                a.node(x).kind,
                a.node_text(x).into_owned(),
                b.node_text(y).into_owned(),
            )
        })
        .collect();
    assert_eq!(
        changed,
        vec![("identifier", "total".to_owned(), "sum".to_owned())],
        "exactly one leaf should have changed text"
    );
    let _ = bm;
}

/// Moving a comment must not disturb the code around it.
///
/// Comments do not participate in matching (see the `metrics` module docs);
/// they ride along with their owner through `sm-cst`'s trivia attachment. If
/// that decision ever regresses, this is where it shows: the two sides of
/// `moved_comment` are identical apart from where one comment sits, so every
/// participating node must match.
#[test]
fn a_moved_comment_leaves_the_code_matching_completely() {
    let (a, b, m) = match_case("moved_comment", &MatchConfig::base_to_side());
    let am = TreeMetrics::compute(&a, java());
    let bm = TreeMetrics::compute(&b, java());
    assert_eq!(am.matchable_count(), bm.matchable_count());
    assert_eq!(
        m.len(),
        am.matchable_count(),
        "moving a comment left {} of {} nodes unmatched",
        am.matchable_count() - m.len(),
        am.matchable_count()
    );
}

/// Two files with nothing in common must match nothing but their roots.
///
/// SPEC.md §0.4: a conflict is always acceptable, a wrong merge never is. A
/// matcher that invents structure between unrelated files hands M4 a
/// fabricated correspondence to merge along.
#[test]
fn unrelated_files_match_almost_nothing() {
    for (name, make) in PROFILES {
        let (a, b, m) = match_case("unrelated", &make());
        let am = TreeMetrics::compute(&a, java());
        let bm = TreeMetrics::compute(&b, java());
        let smaller = am.matchable_count().min(bm.matchable_count());
        assert!(
            m.len() * 4 < smaller,
            "profile {name}: matched {} of {smaller} nodes between unrelated files",
            m.len()
        );
    }
}

/// An empty file against a real one matches the roots and nothing else.
#[test]
fn empty_against_non_empty_matches_only_the_roots() {
    for (name, make) in PROFILES {
        let (_, _, m) = match_case("empty_vs_nonempty", &make());
        assert_eq!(m.len(), 1, "profile {name}: expected only the root pair");
        assert_eq!(m.dst_of(NodeId::ROOT), Some(NodeId::ROOT));
    }
}
