//! The structural invariants of the arena, checked over the TypeScript fixtures.
//!
//! A deliberate parallel of `tests/invariants.rs` rather than an extension of
//! it: that file iterates `FIXTURES`, which is a list of `.java` base names, and
//! folding two extensions and three grammars into one loop would have made both
//! suites harder to read. Everything checked here is checked by the *same*
//! language-agnostic helpers in `sm_cst::invariants` — which is the point. The
//! CST core has no idea a second language arrived.

mod support;

use sm_cst::invariants;
use support::{BROKEN_TS_FIXTURES, TS_FIXTURES, parse_ts_fixture, ts_fixture_bytes, ts_lang};

/// (a) A node's byte range contains all of its children's ranges.
#[test]
fn containment_holds_for_every_fixture() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let violations = invariants::check_containment(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (b) Sibling ranges are non-overlapping and in source order.
#[test]
fn sibling_ranges_are_ordered_and_disjoint() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let violations = invariants::check_sibling_order(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (c) Leaf tokens plus the gaps between them reproduce the input byte for byte.
#[test]
fn every_byte_is_accounted_for() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let violations = invariants::check_byte_coverage(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

/// (c) again, written out explicitly rather than through the helper — the
/// helper could be wrong in the same way twice, and this is the one property
/// the whole emitter rests on.
#[test]
fn leaves_and_gaps_reconstruct_the_source_exactly() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
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
        assert!(
            rebuilt == ts_fixture_bytes(name),
            "{name}: reconstruction differs from the file on disk"
        );
    }
}

/// (d) Preorder numbering: parents are numbered before their children, parent
/// links agree with child lists, and only the root is parentless.
#[test]
fn node_ids_are_preorder() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
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
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let violations = invariants::check_all(&tree);
        assert!(violations.is_empty(), "{name}: {violations:#?}");
    }
}

#[test]
fn has_errors_matches_expectations() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let expected = BROKEN_TS_FIXTURES.contains(name);
        assert_eq!(
            tree.has_errors(),
            expected,
            "{name}: has_errors() should be {expected}"
        );
    }
}

/// A tree with syntax errors is still a well-formed arena — the flag is a flag,
/// not an `Err` (decision 8), and that has to stay true for a second language.
#[test]
fn broken_fixture_still_covers_every_byte() {
    for name in BROKEN_TS_FIXTURES {
        let tree = parse_ts_fixture(name);
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
    let tree = parse_ts_fixture("ts_empty.ts");
    assert_eq!(tree.source().len(), 0);
    assert_eq!(tree.len(), 1, "an empty file should be exactly one node");
    assert_eq!(tree.root().kind, "program");
    assert!(tree.root().is_empty());
    assert!(!tree.has_errors());
}

/// A file that is only comments still holds every byte, and every comment
/// floats — there is nothing in the file for the attachment pass to give them
/// to.
#[test]
fn comment_only_file_keeps_its_comments() {
    let name = "ts_only_comment.ts";
    let tree = parse_ts_fixture(name);
    let lang = ts_lang(name);
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
    let name = "ts_unicode.ts";
    let tree = parse_ts_fixture(name);
    let lang = ts_lang(name);
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
    let text = String::from_utf8(source.to_vec()).expect("fixture is valid UTF-8");
    assert!(text.contains('π') && text.contains("こんにちは、世界"));
    assert!(
        text.contains("🎉"),
        "the fixture should contain astral-plane characters"
    );

    let identifiers: Vec<String> = tree
        .ids()
        .filter(|&id| lang.is_identifier(tree.node(id).kind))
        .map(|id| tree.node_text(id).into_owned())
        .collect();
    for expected in ["π", "τ", "ϕ", "grüße", "Übersetzung", "résultat"] {
        assert!(
            identifiers.iter().any(|i| i == expected),
            "expected the non-ASCII identifier {expected} among {identifiers:?}"
        );
    }

    // `ключ` is a *member* name, so it is deliberately not a lexical identifier
    // (see `TypeScriptLanguage::IDENTIFIERS`) — but its text is still
    // significant, because the matcher has to tell one property from another.
    let members: Vec<String> = tree
        .ids()
        .filter(|&id| tree.node(id).kind == "property_identifier")
        .map(|id| tree.node_text(id).into_owned())
        .collect();
    assert!(
        members.iter().any(|m| m == "ключ"),
        "expected ключ among the property identifiers {members:?}"
    );
    assert!(!identifiers.iter().any(|i| i == "ключ"));
    assert!(lang.significant_text("property_identifier"));
}

/// `named_children` must be a strict subset of `children`, in the same order.
#[test]
fn named_children_is_a_subsequence_of_children() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
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
#[test]
fn descendants_form_a_contiguous_id_range() {
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
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

/// The two dialects are two grammars, and the registry hands each fixture the
/// right one. A `.tsx` file parsed with the plain grammar would be full of
/// errors; a clean parse of both proves the dispatch works.
#[test]
fn each_fixture_gets_the_grammar_its_extension_asks_for() {
    assert_eq!(ts_lang("ts_typical.ts").name(), "typescript");
    assert_eq!(ts_lang("ts_react.tsx").name(), "tsx");

    for name in ["ts_typical.ts", "ts_react.tsx", "ts_unicode.ts"] {
        let tree = parse_ts_fixture(name);
        assert!(!tree.has_errors(), "{name} should parse cleanly");
    }

    // The same TSX bytes through the plain TypeScript grammar do *not* parse,
    // which is why there are two registry entries rather than one.
    let tsx_source = ts_fixture_bytes("ts_react.tsx");
    let as_plain_ts = sm_cst::parse(&tsx_source, support::typescript()).expect("still parses");
    assert!(
        as_plain_ts.has_errors(),
        "the plain TypeScript grammar should not accept JSX"
    );
}

/// The TypeScript trees are the size we expect them to be — a smoke test that
/// the fixtures are exercising real structure rather than a handful of nodes.
#[test]
fn fixtures_are_substantial() {
    let typical = parse_ts_fixture("ts_typical.ts");
    assert!(
        typical.len() > 500,
        "ts_typical.ts has {} nodes",
        typical.len()
    );
    let react = parse_ts_fixture("ts_react.tsx");
    assert!(react.len() > 250, "ts_react.tsx has {} nodes", react.len());
}
