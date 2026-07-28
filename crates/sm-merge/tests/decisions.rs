//! The node-level decision table from the crate docs, row by row.
//!
//! Each test names the row it pins. Where a row's answer is "take one side's
//! bytes", the assertion is on the *provenance* rather than on rendered text —
//! that is the claim SPEC.md §4.6 makes, and it is stronger than an equality of
//! strings.

mod support;

use sm_merge::{MergeConfig, MergedNode, Side, merge};
use support::{Run, java, parse};

/// `merge(B, X, B)` — one side untouched.
#[test]
fn row_theirs_unchanged_takes_our_whole_tree() {
    let run = Run::java(
        "class C { void a() { x(); } }",
        "class C { void a() { y(); } }",
        "class C { void a() { x(); } }",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((Side::Ours, sm_cst::NodeId::ROOT)),
        "an untouched theirs must reduce to a single splice of ours"
    );
}

/// `merge(B, B, Y)`.
#[test]
fn row_ours_unchanged_takes_their_whole_tree() {
    let run = Run::java(
        "class C { void a() { x(); } }",
        "class C { void a() { x(); } }",
        "class C { void a() { y(); } }",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((Side::Theirs, sm_cst::NodeId::ROOT))
    );
}

/// `merge(B, B, B)` — nothing happened. Ours wins the tie by the side
/// preference rule, and the two are identical anyway.
#[test]
fn row_nobody_changed_anything() {
    let src = "class C { void a() { x(); } }";
    let run = Run::java(src, src, src);
    assert!(run.is_clean());
    assert!(matches!(
        run.outcome.tree.node(run.outcome.tree.root()),
        MergedNode::Splice { .. }
    ));
}

/// `Updated(x) | Updated(x) → take our bytes`. The two sides converge on the
/// same code but wrote it with different whitespace, which is exactly the case
/// SPEC.md §4.5's "take x" cannot express.
#[test]
fn row_convergent_edits_take_our_bytes() {
    let run = Run::java(
        "class C { int f() { return 1; } }",
        "class C { int f() { return 2; } }",
        "class C {\n  int f() {\n    return 2;\n  }\n}",
    );
    assert!(run.is_clean(), "convergent edits must not conflict");
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((Side::Ours, sm_cst::NodeId::ROOT)),
        "side preference: ours"
    );
}

/// Same scenario with the sides swapped: still ours, which is the point of a
/// *deterministic* preference rather than a symmetric one.
#[test]
fn row_convergent_edits_take_our_bytes_when_we_are_the_reformatted_one() {
    let run = Run::java(
        "class C { int f() { return 1; } }",
        "class C {\n  int f() {\n    return 2;\n  }\n}",
        "class C { int f() { return 2; } }",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((Side::Ours, sm_cst::NodeId::ROOT))
    );
}

/// `Formatting | Updated(y) → take theirs`. Conflicting on a reindent would be
/// a regression against git on a change git does not even see.
#[test]
fn row_a_reformat_loses_to_a_content_change() {
    let run = Run::java(
        "class C { int f() { return 1; } }",
        "class C {\n  int f() {\n    return 1;\n  }\n}",
        "class C { int f() { return 2; } }",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((Side::Theirs, sm_cst::NodeId::ROOT))
    );
}

/// `Updated(x) | Updated(y), x≠y` at a leaf → conflict, promoted to the
/// enclosing declaration.
#[test]
fn row_divergent_updates_conflict_at_the_enclosing_declaration() {
    let run = Run::java(
        "class C { int x = 1; }\n",
        "class C { int x = 2; }\n",
        "class C { int x = 3; }\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("update_update", "field_declaration")]
    );
}

/// Disjoint edits inside one method body merge without conflict — the sequence
/// merge sees two independent statements, not two edits to one region.
#[test]
fn disjoint_edits_in_one_method_merge_cleanly() {
    let run = Run::java(
        "class C {\n  void a() {\n    int i = 1;\n    int j = 2;\n  }\n}\n",
        "class C {\n  void a() {\n    int i = 11;\n    int j = 2;\n  }\n}\n",
        "class C {\n  void a() {\n    int i = 1;\n    int j = 22;\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
}

/// `Deleted | Updated → conflict`, and it lands on the declaration rather than
/// on the class body.
#[test]
fn row_delete_update_conflicts_on_the_declaration() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
        "class C {\n  void a() { x(); z(); }\n  void b() {}\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("delete_update", "method_declaration")]
    );
}

/// The mirror image, with the reason mirrored too.
#[test]
fn row_update_delete_conflicts_on_the_declaration() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n  void b() {}\n}\n",
        "class C {\n  void a() { x(); z(); }\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("update_delete", "method_declaration")]
    );
}

/// `Deleted | Deleted → delete`, cleanly.
#[test]
fn row_both_delete_the_same_member() {
    let run = Run::java(
        "class C {\n  void a() {}\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
}

/// `Deleted | Unchanged → delete`.
#[test]
fn row_delete_versus_untouched_deletes() {
    let run = Run::java(
        "class C {\n  void a() {}\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
        "class C {\n  void a() {}\n  void b() {}\n}\n",
    );
    assert!(run.is_clean());
}

/// Both sides changed the same node's anonymous tokens differently. Without
/// tokens in the content sequence this would silently emit one side's operator.
#[test]
fn divergent_operators_are_a_kind_clash_not_a_silent_pick() {
    let run = Run::java(
        "class C { void f(int i) { i = 1; } }\n",
        "class C { void f(int i) { i += 1; } }\n",
        "class C { void f(int i) { i -= 1; } }\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("kind_clash", "expression_statement")],
        "an operator disagreement must reach the surface"
    );
}

/// Root pairing failure is representable, and produces one whole-file conflict
/// rather than a panic. Reached here by handing the merge an empty matching.
#[test]
fn a_root_that_does_not_pair_is_a_single_conflict() {
    let base = parse("class C {}\n", java());
    let ours = parse("class D {}\n", java());
    let theirs = parse("class E {}\n", java());
    let empty = sm_match::Matching::new(base.len(), ours.len());
    let empty2 = sm_match::Matching::new(base.len(), theirs.len());
    let outcome = sm_merge::merge_with_matchings(
        &base,
        &ours,
        &theirs,
        &empty,
        &empty2,
        java(),
        &MergeConfig::default(),
    );
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].reason.tag(), "root_mismatch");
}

/// A comment rewritten on one side and code edited on the other is two
/// disjoint changes, not a conflict — and both survive.
#[test]
fn a_comment_only_change_merges_against_a_code_change() {
    let run = Run::java(
        "class C {\n  // old\n  void a() { x(); }\n}\n",
        "class C {\n  // new\n  void a() { x(); }\n}\n",
        "class C {\n  // old\n  void a() { x(); z(); }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    insta::assert_snapshot!(run.render());
}

/// Both sides rewriting the same attached comment differently is a conflict on
/// the owner. Reconciling two prose edits is not a merge algorithm's job.
#[test]
fn both_sides_editing_one_comment_conflicts_on_its_owner() {
    let run = Run::java(
        "class C {\n  // old\n  void a() { x(); }\n}\n",
        "class C {\n  // ours\n  void a() { x1(); }\n}\n",
        "class C {\n  // theirs\n  void a() { x2(); }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("comment_edit", "method_declaration")]
    );
}

/// Empty files, and files that are nothing but a comment, must not panic and
/// must not invent structure.
#[test]
fn degenerate_files_are_handled() {
    for (b, o, t) in [
        ("", "", ""),
        ("", "class C {}\n", ""),
        ("class C {}\n", "", "class C {}\n"),
        ("// only a comment\n", "// only a comment\n", "\n"),
    ] {
        let run = Run::java(b, o, t);
        assert!(!run.outcome.tree.is_empty());
    }
}

/// TypeScript goes through the same code path with no language-specific
/// branches, which is the claim SPEC.md §3 makes about the `Language` trait.
#[test]
fn the_decision_table_is_language_agnostic() {
    let run = Run::typescript(
        "class C {\n  a() { return 1; }\n}\n",
        "class C {\n  a() { return 2; }\n}\n",
        "class C {\n  a() { return 1; }\n  b() { return 3; }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());

    let conflicting = Run::typescript("const x = 1;\n", "const x = 2;\n", "const x = 3;\n");
    assert_eq!(
        conflicting.conflicts(),
        vec![("update_update", "lexical_declaration")]
    );
}

/// Turning move detection off is a supported configuration (M5 sweeps it), and
/// it degrades to delete-plus-insert rather than misbehaving.
#[test]
fn move_detection_can_be_disabled() {
    let cfg = MergeConfig {
        detect_moves: false,
        ..MergeConfig::default()
    };
    let base = parse("class C {\n  void a() {\n    x();\n  }\n}\n", java());
    let ours = parse(
        "class C {\n  void a() {\n    if (ok) {\n      x();\n    }\n  }\n}\n",
        java(),
    );
    let theirs = parse("class C {\n  void a() {\n    x();\n  }\n}\n", java());
    let outcome = merge(&base, &ours, &theirs, java(), &cfg);
    // Theirs is untouched, so the answer is ours whatever move detection does.
    assert!(outcome.is_clean());
}
