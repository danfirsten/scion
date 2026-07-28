//! Property-style tests: the invariants M4, M5 and M6 are allowed to assume of
//! an edit script.
//!
//! Hand-driven rather than `proptest`-driven, for the reason `sm-match`'s
//! suite gives: SPEC.md §5.2 puts `proptest` on the *merge*, where the inputs
//! are three trees and the oracle is an equality. Here the interesting inputs
//! are realistic source edits, which a random generator does not produce and a
//! fixture corpus does. What is generated instead is the *shape* of an edit —
//! see [`one_statement_moved_out_of_ten_costs_one_reorder`], which builds its
//! inputs programmatically because the property is about a permutation, not
//! about Java.

mod support;

use std::collections::BTreeSet;

use sm_cst::NodeId;
use sm_diff::{EditOp, MoveKind, derive, participating_subtree};
use sm_match::{MatchConfig, TreeMetrics, match_trees};
use support::{CASES, PROFILES, language_for, load_case, load_snippets, parse_side};

/// Diffing a file against itself must produce nothing.
///
/// The strongest single statement about the derivation: if it cannot see a
/// file as a copy of itself, nothing else it says is worth anything.
#[test]
fn self_diff_is_empty_on_every_case() {
    for &(case, ext) in CASES {
        for side in ["a", "b"] {
            let tree = parse_side(case, ext, side);
            let lang = language_for(ext);
            for (name, make) in PROFILES {
                let m = match_trees(&tree, &tree, lang, &make());
                let script = derive(&tree, &tree, &m, lang);
                assert!(
                    script.is_empty(),
                    "{case}/{side}.{ext} under {name} diffed against itself as {:#?}",
                    script.ops
                );
            }
        }
    }
}

/// Reindentation alone is not an edit.
///
/// SPEC.md §1 names this as a headline false conflict, and it is the case the
/// viewer's `[body unchanged, reindented]` tag exists for. Here the *whole*
/// file is re-laid-out, so even the tag should not appear: nothing moved, so
/// there is nothing at all to report.
#[test]
fn pure_reformatting_is_not_an_edit() {
    let tight = "class C{int f(int a,int b){int s=a+b;return s;}}";
    let loose =
        "class C {\n\n    int f(int a, int b) {\n\t\tint s = a + b;\n\t\treturn s;\n    }\n\n}\n";
    let loaded = load_snippets(tight, loose, "java", &MatchConfig::base_to_side());
    assert!(
        loaded.script.is_empty(),
        "reformatting produced {:#?}",
        loaded.script.ops
    );
}

/// Comments are not edits either: they ride along with their trivia owner.
#[test]
fn adding_a_comment_is_not_an_edit() {
    let bare = "class C {\n    int f() {\n        return 1;\n    }\n}\n";
    let noisy =
        "// header\nclass C {\n    /** why */\n    int f() {\n        return 1; // yes\n    }\n}\n";
    let loaded = load_snippets(bare, noisy, "java", &MatchConfig::base_to_side());
    assert!(
        loaded.script.is_empty(),
        "comments produced {:#?}",
        loaded.script.ops
    );
}

/// Every operand is a real, participating node of the tree it claims to index,
/// and says what the matching says.
#[test]
fn every_operand_is_valid_and_agrees_with_the_matching() {
    for &(case, ext) in CASES {
        for (profile, make) in PROFILES {
            let loaded = load_case(case, ext, &make());
            let (a, b, m) = (&loaded.a, &loaded.b, &loaded.matching);
            let (am, bm) = (&loaded.am, &loaded.bm);
            let where_ = format!("{case} under {profile}");

            for op in &loaded.script.ops {
                if let Some(src) = op.src() {
                    assert!(src.index() < a.len(), "{where_}: {src} out of range");
                    assert!(am.participates(src), "{where_}: {src} does not participate");
                }
                if let Some(dst) = op.dst() {
                    assert!(dst.index() < b.len(), "{where_}: {dst} out of range");
                    assert!(bm.participates(dst), "{where_}: {dst} does not participate");
                }
                match *op {
                    EditOp::Insert {
                        dst,
                        dst_parent,
                        position,
                    } => {
                        assert!(
                            !m.is_dst_matched(dst),
                            "{where_}: inserted {dst} is matched"
                        );
                        assert_eq!(
                            dst_parent,
                            bm.parent(dst),
                            "{where_}: {dst} reports the wrong parent"
                        );
                        if let Some(p) = dst_parent {
                            assert!(
                                m.is_dst_matched(p),
                                "{where_}: {dst}'s parent {p} is unmatched, so {dst} is not the \
                                 highest unmatched node"
                            );
                            assert_eq!(
                                bm.children(p).get(position).copied(),
                                Some(dst),
                                "{where_}: {dst} is not at position {position} of {p}"
                            );
                        }
                    }
                    EditOp::Delete { src } => {
                        assert!(!m.is_src_matched(src), "{where_}: deleted {src} is matched");
                        if let Some(p) = am.parent(src) {
                            assert!(
                                m.is_src_matched(p),
                                "{where_}: {src}'s parent {p} is unmatched too"
                            );
                        }
                    }
                    EditOp::Update { src, dst } | EditOp::Move { src, dst, .. } => {
                        assert_eq!(
                            m.dst_of(src),
                            Some(dst),
                            "{where_}: {src} is not matched to {dst}"
                        );
                        assert_eq!(
                            a.node(src).kind,
                            b.node(dst).kind,
                            "{where_}: a matched pair with different kinds"
                        );
                    }
                }
            }
        }
    }
}

/// A `Reparent` really has a different parent pair, and a `Reorder` really has
/// the same one.
#[test]
fn move_kinds_say_what_they_mean() {
    for &(case, ext) in CASES {
        for (profile, make) in PROFILES {
            let loaded = load_case(case, ext, &make());
            for op in &loaded.script.ops {
                let EditOp::Move { src, dst, kind } = *op else {
                    continue;
                };
                let pa = loaded.am.parent(src);
                let pb = loaded.bm.parent(dst);
                let same_parent = match (pa, pb) {
                    (Some(pa), Some(pb)) => loaded.matching.dst_of(pa) == Some(pb),
                    (None, None) => true,
                    _ => false,
                };
                match kind {
                    MoveKind::Reparent => assert!(
                        !same_parent,
                        "{case}/{profile}: {src} is a Reparent but its parent pair corresponds"
                    ),
                    MoveKind::Reorder => assert!(
                        same_parent,
                        "{case}/{profile}: {dst} is a Reorder but its parent pair does not \
                         correspond"
                    ),
                }
            }
        }
    }
}

/// Two derivations over the same inputs are byte-identical, script and render
/// alike.
///
/// The failure this guards against is a `HashMap` iteration order leaking into
/// an operation order. That bug stays invisible until a snapshot flakes months
/// later, so it gets its own test.
#[test]
fn derivation_and_rendering_are_deterministic() {
    for &(case, ext) in CASES {
        for (profile, make) in PROFILES {
            let first = load_case(case, ext, &make());
            let second = load_case(case, ext, &make());
            assert_eq!(
                first.script, second.script,
                "{case} under {profile} is not deterministic"
            );
            assert_eq!(
                sm_diff::render(&first.view(), &sm_diff::RenderOptions::default()),
                sm_diff::render(&second.view(), &sm_diff::RenderOptions::default()),
                "{case} under {profile} renders differently on a second run"
            );
        }
    }
}

/// The counts are the operations.
#[test]
fn the_summary_counts_the_operations() {
    for &(case, ext) in CASES {
        let loaded = load_case(case, ext, &MatchConfig::base_to_side());
        let s = loaded.script.summary;
        assert_eq!(
            s.inserts,
            loaded.script.iter_kind("insert").count(),
            "{case}"
        );
        assert_eq!(
            s.deletes,
            loaded.script.iter_kind("delete").count(),
            "{case}"
        );
        assert_eq!(
            s.updates,
            loaded.script.iter_kind("update").count(),
            "{case}"
        );
        assert_eq!(s.moves, loaded.script.iter_kind("move").count(), "{case}");
        assert_eq!(s.moves, s.reparents + s.reorders, "{case}");
        assert_eq!(s.is_empty(), loaded.script.is_empty(), "{case}");
    }
}

/// Inserts and deletes together account for every unmatched participating node,
/// exactly once, once expanded.
///
/// This is the statement that "reported at the highest unmatched node only" is
/// a compression and not a loss. The expansion may cover *more* than the
/// unmatched nodes — an inserted `if` wrapper contains the matched block that
/// moved into it — and every such extra node must be one that is matched, i.e.
/// reported by a `Move` rather than lost.
#[test]
fn expanding_inserts_and_deletes_recovers_every_unmatched_node() {
    for &(case, ext) in CASES {
        let loaded = load_case(case, ext, &MatchConfig::base_to_side());

        for (label, metrics, script_is_insert) in
            [("dst", &loaded.bm, true), ("src", &loaded.am, false)]
        {
            let mut covered: Vec<NodeId> = Vec::new();
            for op in &loaded.script.ops {
                match *op {
                    EditOp::Insert { dst, .. } if script_is_insert => {
                        covered.extend(participating_subtree(metrics, dst));
                    }
                    EditOp::Delete { src } if !script_is_insert => {
                        covered.extend(participating_subtree(metrics, src));
                    }
                    _ => {}
                }
            }
            let unique: BTreeSet<NodeId> = covered.iter().copied().collect();
            assert_eq!(
                unique.len(),
                covered.len(),
                "{case}/{label}: a node was covered by two operations"
            );

            let matched = |id: NodeId| {
                if script_is_insert {
                    loaded.matching.is_dst_matched(id)
                } else {
                    loaded.matching.is_src_matched(id)
                }
            };
            let expected: BTreeSet<NodeId> = metrics
                .post_order()
                .iter()
                .copied()
                .filter(|&id| !matched(id))
                .collect();
            assert!(
                expected.is_subset(&unique),
                "{case}/{label}: {:?} were unmatched but no operation covers them",
                expected.difference(&unique).collect::<Vec<_>>()
            );
            for id in unique.difference(&expected) {
                assert!(
                    matched(*id),
                    "{case}/{label}: {id} is covered but is neither unmatched nor matched"
                );
            }
        }
    }
}

/// A whole new declaration is **one** operation.
#[test]
fn a_new_method_is_one_insert_not_one_per_node() {
    let loaded = load_case("insert_method", "java", &MatchConfig::base_to_side());
    assert_eq!(loaded.script.summary.inserts, 1);
    let EditOp::Insert { dst, .. } = loaded.script.ops[0] else {
        panic!("expected an Insert, got {:?}", loaded.script.ops[0]);
    };
    assert_eq!(loaded.b.node(dst).kind, "method_declaration");
    assert!(
        participating_subtree(&loaded.bm, dst).len() > 8,
        "the expansion should recover the whole method"
    );
}

/// The LIS property, stated on exactly the shape it is there for.
///
/// Ten statements in an ordered block; the tenth moves to the front. A
/// position-by-position comparison would call nine of them moved. The longest
/// increasing subsequence keeps those nine and reports the one that actually
/// moved.
#[test]
fn one_statement_moved_out_of_ten_costs_one_reorder() {
    let body = |order: &[usize]| {
        let calls: String = order.iter().map(|i| format!("        s{i}();\n")).collect();
        format!("class C {{\n    void run() {{\n{calls}    }}\n}}\n")
    };
    let straight: Vec<usize> = (0..10).collect();

    for moved in [0usize, 3, 9] {
        let mut permuted: Vec<usize> = straight.iter().copied().filter(|&i| i != moved).collect();
        // Put the moved statement somewhere it certainly was not.
        let target = if moved < 5 { permuted.len() } else { 0 };
        permuted.insert(target, moved);

        let loaded = load_snippets(
            &body(&straight),
            &body(&permuted),
            "java",
            &MatchConfig::base_to_side(),
        );
        let s = loaded.script.summary;
        assert_eq!(
            (s.reorders, s.reparents, s.inserts, s.deletes, s.updates),
            (1, 0, 0, 0, 0),
            "moving s{moved} produced {:#?}",
            loaded.script.ops
        );

        let EditOp::Move { src, kind, .. } = loaded
            .script
            .ops
            .iter()
            .copied()
            .find(|op| matches!(op, EditOp::Move { .. }))
            .expect("a move")
        else {
            unreachable!()
        };
        assert_eq!(kind, MoveKind::Reorder);
        assert!(
            loaded.a.node_text(src).contains(&format!("s{moved}()")),
            "the reported move is not the statement that moved"
        );
    }
}

/// A full reversal is the worst case, and the LIS still reports the minimum:
/// `n - 1` moves, because only one element can stay.
#[test]
fn a_reversed_block_reports_n_minus_one_reorders() {
    let body = |order: Box<dyn Iterator<Item = usize>>| {
        let calls: String = order.map(|i| format!("        s{i}();\n")).collect();
        format!("class C {{\n    void run() {{\n{calls}    }}\n}}\n")
    };
    let loaded = load_snippets(
        &body(Box::new(0..5)),
        &body(Box::new((0..5).rev())),
        "java",
        &MatchConfig::base_to_side(),
    );
    assert_eq!(loaded.script.summary.reorders, 4);
    assert_eq!(loaded.script.summary.reparents, 0);
}

/// Reordering inside an **unordered** child list is not an edit.
///
/// `class_body` is `Unordered` and `program` is `PartiallyUnordered` over
/// `import_declaration` (PROGRESS.md decisions 9 and 10), so permuting class
/// members or imports must derive to nothing. This is the single largest source
/// of real-world false conflicts, and it is the reason the derivation consults
/// `Language::child_list_kind` at all.
#[test]
fn reordering_an_unordered_container_is_not_an_edit() {
    for case in ["moved_method", "import_shuffle"] {
        for (profile, make) in PROFILES {
            let loaded = load_case(case, "java", &make());
            assert!(
                loaded.script.is_empty(),
                "{case} under {profile} produced {:#?}",
                loaded.script.ops
            );
        }
    }

    // Fields as well as methods, and both at once.
    let a = "class C {\n    int x;\n    String y;\n    void f() {}\n    void g() {}\n}\n";
    let b = "class C {\n    void g() {}\n    String y;\n    void f() {}\n    int x;\n}\n";
    let loaded = load_snippets(a, b, "java", &MatchConfig::base_to_side());
    assert!(
        loaded.script.is_empty(),
        "permuted class members produced {:#?}",
        loaded.script.ops
    );
}

/// Reordering inside an **ordered** list *is* an edit, so the test above is
/// measuring a decision rather than a hole.
#[test]
fn reordering_an_ordered_container_is_an_edit() {
    let a = "class C {\n    void f() {\n        one();\n        two();\n    }\n}\n";
    let b = "class C {\n    void f() {\n        two();\n        one();\n    }\n}\n";
    let loaded = load_snippets(a, b, "java", &MatchConfig::base_to_side());
    assert_eq!(loaded.script.summary.reorders, 1);
}

/// A rename is one `Update` on one identifier — not a `Delete` plus an
/// `Insert`, and not an `Update` of every ancestor.
#[test]
fn a_rename_is_a_single_local_update() {
    let loaded = load_case("renamed_method", "java", &MatchConfig::base_to_side());
    let s = loaded.script.summary;
    assert_eq!((s.inserts, s.deletes, s.moves, s.updates), (0, 0, 0, 1));

    let EditOp::Update { src, dst } = loaded.script.ops[0] else {
        panic!("expected an Update");
    };
    assert_eq!(loaded.a.node(src).kind, "identifier");
    assert_eq!(loaded.a.node_text(src), "total");
    assert_eq!(loaded.b.node_text(dst), "sum");
}

/// An anonymous-token change is an `Update`, because the token is part of the
/// node's own content. This is the `a + b` versus `a - b` case that a model
/// which dropped anonymous tokens would call unchanged.
#[test]
fn an_operator_change_is_a_local_update() {
    for (before, after) in [
        (
            "int f(int a, int b) { return a + b; }",
            "int f(int a, int b) { return a - b; }",
        ),
        ("void f(int i) { i++; }", "void f(int i) { i--; }"),
        ("public int x = 1;", "public static int x = 1;"),
    ] {
        let loaded = load_snippets(
            &format!("class C {{ {before} }}"),
            &format!("class C {{ {after} }}"),
            "java",
            &MatchConfig::base_to_side(),
        );
        assert!(
            loaded.script.summary.updates >= 1,
            "{before} → {after} produced {:#?}",
            loaded.script.ops
        );
    }
}

/// Inserting a statement into a block does **not** also mark the block, the
/// method or the class as changed. `Update` is local; `Insert` already says
/// what happened, at the right granularity.
#[test]
fn inserting_a_child_does_not_update_its_ancestors() {
    let a = "class C {\n    void f() {\n        one();\n        three();\n    }\n}\n";
    let b =
        "class C {\n    void f() {\n        one();\n        two();\n        three();\n    }\n}\n";
    let loaded = load_snippets(a, b, "java", &MatchConfig::base_to_side());
    assert_eq!(
        (
            loaded.script.summary.inserts,
            loaded.script.summary.updates,
            loaded.script.summary.moves
        ),
        (1, 0, 0),
        "{:#?}",
        loaded.script.ops
    );
}

/// Wrapping a body in an `if` does not mark the enclosing method as changed
/// either — the case the "children that stayed" clause of the `Update`
/// definition exists for.
#[test]
fn wrapping_a_body_does_not_update_the_enclosing_method() {
    let loaded = load_case("wrapped_in_if", "java", &MatchConfig::base_to_side());
    assert_eq!(
        (
            loaded.script.summary.inserts,
            loaded.script.summary.deletes,
            loaded.script.summary.updates,
            loaded.script.summary.reparents
        ),
        (1, 0, 0, 1),
        "{:#?}",
        loaded.script.ops
    );
}

/// The documented order really is the order emitted, and it reads down the
/// source file.
///
/// `EditScript::ops` promises a sort by `(src_anchor, dst_anchor, rank)`. The
/// part of that a consumer will actually rely on is the first component: the
/// operations with a source node come out in source preorder, so walking the
/// script is walking the file.
#[test]
fn the_operation_order_is_source_preorder() {
    for &(case, ext) in CASES {
        let loaded = load_case(case, ext, &MatchConfig::base_to_side());

        // Re-deriving from the same matching reproduces the same order.
        let again = derive(&loaded.a, &loaded.b, &loaded.matching, loaded.lang);
        assert_eq!(again.ops, loaded.script.ops, "{case}");

        let anchors: Vec<u32> = loaded
            .script
            .ops
            .iter()
            .filter_map(|op| op.src().map(|id| id.0))
            .collect();
        let mut sorted = anchors.clone();
        sorted.sort_unstable();
        assert_eq!(anchors, sorted, "{case}: source anchors are not ascending");
    }
}

/// `derive` and `derive_with_metrics` are the same function.
#[test]
fn the_two_entry_points_agree() {
    for &(case, ext) in CASES {
        let lang = language_for(ext);
        let a = parse_side(case, ext, "a");
        let b = parse_side(case, ext, "b");
        let am = TreeMetrics::compute(&a, lang);
        let bm = TreeMetrics::compute(&b, lang);
        let m = match_trees(&a, &b, lang, &MatchConfig::base_to_side());
        assert_eq!(
            derive(&a, &b, &m, lang),
            sm_diff::derive_with_metrics(&a, &am, &b, &bm, &m, lang),
            "{case}"
        );
    }
}
