//! The structural invariants of the arena, checked over every fixture.
//!
//! These are the assertions that make "the CST retains every byte of the source"
//! a fact rather than a claim (SPEC.md §4.2).

mod support;

use sm_cst::invariants;
use support::{BROKEN_FIXTURES, FIXTURES, parse_fixture};

/// (a) A node's byte range contains all of its children's ranges.
#[test]
fn containment_holds_for_every_fixture() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let violations = invariants::check_containment(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (b) Sibling ranges are non-overlapping and in source order.
#[test]
fn sibling_ranges_are_ordered_and_disjoint() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let violations = invariants::check_sibling_order(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (c) Leaf tokens plus the gaps between them reproduce the input byte for byte.
#[test]
fn every_byte_is_accounted_for() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let violations = invariants::check_byte_coverage(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (c) again, written out explicitly rather than through the helper — the
/// helper could be wrong in the same way twice, and this is the one property
/// the whole emitter rests on.
#[test]
fn leaves_and_gaps_reconstruct_the_source_exactly() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let source = tree.source();
        let mut rebuilt = Vec::with_capacity(source.len());
        let mut cursor = 0usize;
        for leaf in tree.leaves() {
            let span = tree.node(leaf).span();
            assert!(
                span.start >= cursor,
                "{name}: leaf {leaf} starts at {} but the previous leaf ended at {cursor}",
                span.start
            );
            rebuilt.extend_from_slice(&source[cursor..span.start]);
            rebuilt.extend_from_slice(&source[span.clone()]);
            cursor = span.end;
        }
        rebuilt.extend_from_slice(&source[cursor..]);
        assert_eq!(
            rebuilt.len(),
            source.len(),
            "{name}: reconstruction has the wrong length"
        );
        assert!(
            rebuilt == source,
            "{name}: reconstruction differs from source"
        );
    }
}

/// (d) Preorder numbering: parents are numbered before their children, parent
/// links agree with child lists, and only the root is parentless.
#[test]
fn node_ids_are_preorder() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let violations = invariants::check_preorder_ids(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");

        for id in tree.ids() {
            for child in tree.children(id) {
                assert!(child > id, "{name}: child {child} <= parent {id}");
            }
        }
    }
}

#[test]
fn check_all_is_clean_for_every_fixture() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let violations = invariants::check_all(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// A tree with syntax errors is still a well-formed arena. That is the whole
/// reason `has_errors()` is a flag rather than an `Err`.
#[test]
fn has_errors_matches_expectations() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        let expected = BROKEN_FIXTURES.contains(name);
        assert_eq!(
            tree.has_errors(),
            expected,
            "{name}: has_errors() should be {expected}"
        );
    }
}

#[test]
fn broken_fixture_still_covers_every_byte() {
    for name in BROKEN_FIXTURES {
        let tree = parse_fixture(name);
        assert!(tree.has_errors(), "{name} should have errors");
        assert!(
            invariants::check_all(&tree).is_empty(),
            "{name}: a tree with syntax errors must still satisfy every structural invariant"
        );
        assert!(
            tree.nodes().any(|n| n.is_error || n.is_missing),
            "{name}: expected at least one ERROR or MISSING node"
        );
    }
}

#[test]
fn empty_file_parses_to_a_single_empty_root() {
    let tree = parse_fixture("empty");
    assert_eq!(tree.source().len(), 0);
    assert_eq!(tree.len(), 1, "an empty file should be exactly one node");
    assert_eq!(tree.root().kind, "program");
    assert!(tree.root().is_empty());
    assert!(!tree.has_errors());
}

/// A file that is only comments still holds every byte, and every comment is
/// still floating — nothing attaches trivia yet, and there is nothing here to
/// attach it to anyway.
#[test]
fn comment_only_file_keeps_its_comments() {
    let tree = parse_fixture("only_comment");
    let lang = support::java();
    let comments: Vec<_> = tree
        .ids()
        .filter(|&id| lang.is_comment(tree.node(id).kind))
        .collect();
    assert_eq!(comments.len(), 3, "expected three comments");
    for id in comments {
        assert!(tree.node(id).is_extra, "comments should be extra nodes");
        assert_eq!(tree.node(id).attachment, sm_cst::Attachment::Floating);
    }
    assert!(invariants::check_all(&tree).is_empty());
}

/// Non-ASCII source: byte ranges must be byte ranges, not char ranges.
#[test]
fn unicode_ranges_are_byte_ranges() {
    let tree = parse_fixture("unicode");
    let source = tree.source();
    assert!(
        source.len() > source.iter().filter(|b| b.is_ascii()).count(),
        "the unicode fixture should actually contain multi-byte characters"
    );
    for id in tree.ids() {
        let node = tree.node(id);
        assert!(
            node.byte_range.end as usize <= source.len(),
            "node {id} ({}) runs past the end of the source",
            node.kind
        );
    }
    // Every node's bytes must slice cleanly out of the source; if any range
    // straddled a UTF-8 boundary the lossy decode would show a replacement
    // character that is not in the original.
    let text = String::from_utf8(source.to_vec()).expect("fixture is valid UTF-8");
    assert!(text.contains('π') && text.contains("こんにちは、世界"));

    let identifiers: Vec<String> = tree
        .ids()
        .filter(|&id| support::java().is_identifier(tree.node(id).kind))
        .map(|id| tree.node_text(id).into_owned())
        .collect();
    assert!(
        identifiers.iter().any(|i| i == "π"),
        "expected the non-ASCII identifier π among {identifiers:?}"
    );
    assert!(
        identifiers.iter().any(|i| i == "Übersetzung"),
        "expected the non-ASCII identifier Übersetzung"
    );
}

/// `named_children` must be a strict subset of `children`, in the same order.
#[test]
fn named_children_is_a_subsequence_of_children() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        for id in tree.ids() {
            let all: Vec<_> = tree.children(id).collect();
            let named: Vec<_> = tree.named_children(id).collect();
            let mut it = all.iter();
            for n in &named {
                assert!(
                    it.any(|a| a == n),
                    "{name}: named child {n} of {id} is out of order or missing"
                );
            }
            assert!(named.len() <= all.len());
        }
    }
}

/// Preorder numbering means a node's descendants form a contiguous ID range.
/// Later milestones will exploit that; assert it now so it cannot quietly stop
/// being true.
#[test]
fn descendants_form_a_contiguous_id_range() {
    for name in FIXTURES {
        let tree = parse_fixture(name);
        for id in tree.ids() {
            let descendants: Vec<_> = tree.descendants(id).collect();
            for (i, d) in descendants.iter().enumerate() {
                assert_eq!(
                    d.0,
                    id.0 + 1 + i as u32,
                    "{name}: descendants of {id} are not contiguous"
                );
            }
        }
    }
}

#[test]
fn ancestors_walk_to_the_root() {
    let tree = parse_fixture("typical");
    let deepest = tree
        .ids()
        .max_by_key(|&id| tree.ancestors(id).count())
        .expect("non-empty tree");
    let chain: Vec<_> = tree.ancestors(deepest).collect();
    assert!(!chain.is_empty());
    assert_eq!(*chain.last().unwrap(), tree.root_id());
}

#[test]
fn source_too_large_is_rejected_rather_than_truncated() {
    // Constructing a 4 GiB buffer is not something a test should do, so assert
    // the boundary constant instead: it must be exactly what a u32 range can
    // address, or `parse` would be handing out truncated offsets.
    assert_eq!(sm_cst::MAX_SOURCE_LEN, u32::MAX as usize);
}
