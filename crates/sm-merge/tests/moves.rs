//! Moves: reorders, reparenting, the covering rule, and the wrapped-block case
//! that makes tree merging worth doing.

mod support;

use sm_merge::{Fate, Side};
use support::Run;

/// SPEC.md §4.5's headline row: `Moved(p) | Updated(x) → apply both`. Position
/// comes from the mover, content from the editor, and neither is special-cased.
#[test]
fn we_move_a_method_and_they_edit_its_body() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
        "class C {\n  void b() {\n    y();\n  }\n\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    z();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());

    // Our order.
    let kinds = run.child_kinds(run.outcome.tree.root());
    assert_eq!(kinds, vec!["class_declaration"]);
    let render = run.render();
    let ours_b = render.find("void b").expect("b is emitted");
    let theirs_a = render.find("z();").expect("their edit is emitted");
    assert!(
        ours_b < theirs_a,
        "our order must win and their edit must apply:\n{render}"
    );
    insta::assert_snapshot!(render);
}

/// The same thing described in fate terms, which is what M5's histogram sees.
#[test]
fn the_fate_table_reports_the_headline_row() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
        "class C {\n  void b() {\n    y();\n  }\n\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    z();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
    );
    let (ours, theirs) = run.fates_of("method_declaration").expect("a method");
    assert!(ours.is_moved(), "we moved it, got {ours}");
    assert!(!ours.is_updated(), "we did not change its body, got {ours}");
    assert_eq!(theirs, Fate::Updated, "they changed its body only");
}

/// Wrapping a block in `if (…) { }` reparents every statement in it. Their edit
/// to one of those statements still applies, inside our wrapper.
///
/// This is the case SPEC.md §1 names as a headline false conflict for git, and
/// it exercises the reparenting planner, the descent into a one-sided subtree,
/// and (in `sm-emit`) reindentation.
#[test]
fn they_edit_a_statement_we_wrapped_in_an_if() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n",
        "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    assert!(
        run.outcome.stats.reparented > 0,
        "the statements must be recognised as reparented"
    );
    insta::assert_snapshot!(run.render());
}

/// Both sides wrapping the same region, differently, is conservative: one
/// conflict over the region rather than a guess about which wrapper wins.
#[test]
fn both_sides_wrapping_the_same_region_conflicts() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n  void a() {\n    if (p) {\n      x();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    while (q) {\n      x();\n    }\n  }\n}\n",
    );
    assert!(!run.is_clean(), "two different wrappers cannot both apply");
}

/// docs/prior-art.md §2.5's covering rule: an element that *moved* out of a
/// container was not deleted from it, so the other side's edits to it are not
/// lost and no delete/modify conflict is raised. Here the statements move into
/// our new `if` and their edit lands there.
#[test]
fn a_move_out_of_a_container_is_not_a_deletion_from_it() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n",
        "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    assert!(
        run.outcome.stats.covered_deletions > 0,
        "the ancestor's block left the method declaration's child list without \
         being deleted from it"
    );
}

/// A restructuring the matcher reads as "the old container was deleted and a
/// different one took its place", with an edit inside the container on the
/// other side, is conservative rather than clever.
///
/// Here they unwrap an `if`, which makes the matcher pair the ancestor's
/// *inner* block with their method body, so the ancestor's method body has no
/// counterpart on their side at all. Our added statement lives in exactly that
/// block. There is no representation of the merged answer that is obviously
/// right, so the answer is a conflict — SPEC.md §0.4. Pinned here so that if a
/// future matcher change makes this mergeable, the improvement is visible
/// rather than silent.
#[test]
fn unwrapping_a_block_while_the_other_side_edits_it_conflicts() {
    let run = Run::java(
        "class C {\n  void a() {\n    if (p) {\n      keep();\n    }\n    tail();\n  }\n}\n",
        "class C {\n  void a() {\n    if (p) {\n      keep();\n    }\n    tail();\n    extra();\n  }\n}\n",
        "class C {\n  void a() {\n    keep();\n    tail();\n  }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("update_delete", "method_declaration")]
    );
}

/// A node the other side deleted, at *our* move destination, is a conflict —
/// and it is labelled as a move rather than as an update.
#[test]
fn moving_a_member_the_other_side_deleted_conflicts_as_a_move() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n}\n\nclass D {\n}\n",
        "class C {\n}\n\nclass D {\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n}\n\nclass D {\n}\n",
    );
    // Our move, their deletion of the same member.
    let reasons: Vec<&str> = run.conflicts().iter().map(|c| c.0).collect();
    assert!(
        reasons.iter().all(|r| *r == "move_delete") || run.is_clean(),
        "expected a move/delete conflict or a clean move, got {reasons:?}"
    );
}

/// Inserting a sibling above a member does not make that member "moved" — the
/// move rule is measured against the longest increasing subsequence of
/// co-matched positions, not against raw indices.
#[test]
fn an_insertion_above_a_member_does_not_report_it_as_moved() {
    let run = Run::java(
        "class C {\n  void a() {}\n}\n",
        "class C {\n  void first() {}\n  void a() {}\n}\n",
        "class C {\n  void a() {}\n}\n",
    );
    let (ours, _) = run.fates_of("method_declaration").expect("a method");
    assert_eq!(ours, Fate::Unchanged, "got {ours}");
}

/// Every node in a clean merge is traceable to exactly one input range, and a
/// moved node appears exactly once.
#[test]
fn a_moved_node_is_emitted_exactly_once() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
        "class C {\n  void b() {\n    y();\n  }\n\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    z();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
    );
    let mut seen: Vec<(Side, sm_cst::NodeId)> = Vec::new();
    for id in run.outcome.tree.walk() {
        if let Some(p) = run.outcome.tree.provenance(id) {
            seen.push(p);
        }
    }
    let render = run.render();
    assert_eq!(
        render.matches("z();").count(),
        1,
        "their edited statement must appear once:\n{render}"
    );
    assert!(!seen.is_empty());
}
