//! Child-list merging: ordered sequences, commutative groups, atomic sets.

mod support;

use sm_merge::MergeConfig;
use support::{Run, java, parse};

// ------------------------------------------------------------------- ordered

/// Insertions from both sides at *different* anchors are disjoint changes.
#[test]
fn disjoint_insertions_into_a_statement_list_both_apply() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    first();\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    q();\n    last();\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    insta::assert_snapshot!(run.render());
}

/// Insertions from both sides at the *same* anchor of an ordered list conflict.
/// SPEC.md §4.5: do not guess an interleaving.
#[test]
fn insertions_at_the_same_anchor_of_an_ordered_list_conflict() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    ours();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    theirs();\n    q();\n  }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("ordered_insert_collision", "expression_statement")]
    );
}

/// The same insertion made on both sides is one insertion, not a collision.
///
/// Each side also adds something of its own, so that the block cannot simply
/// converge: if the two revisions were identical the root would be taken
/// wholesale and no child list would ever be aligned.
#[test]
fn identical_insertions_at_one_anchor_deduplicate() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    both();\n    q();\n    onlyOurs();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    both();\n    q();\n    onlyTheirs();\n  }\n}\n",
    );
    assert_eq!(
        run.outcome.stats.deduplicated_insertions, 1,
        "`both()` must be recognised as one insertion"
    );
    assert_eq!(
        run.conflicts(),
        vec![("ordered_insert_collision", "expression_statement")],
        "only the two different trailing insertions collide"
    );
    insta::assert_snapshot!(run.render());
}

/// The same statement inserted at the same anchor, with nothing else to
/// disagree about, needs no conflict and no duplicate.
#[test]
fn a_wholly_convergent_insertion_needs_no_alignment_at_all() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    both();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    both();\n  }\n}\n",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.outcome.tree.provenance(run.outcome.tree.root()),
        Some((sm_merge::Side::Ours, sm_cst::NodeId::ROOT)),
        "convergence at the root short-circuits the whole descent"
    );
}

/// Enum constants are an ordered list on purpose — ordinals are persisted
/// (PROGRESS.md decision 11) — so two additions at one anchor must conflict
/// rather than commute.
#[test]
fn enum_constants_do_not_commute() {
    let run = Run::java(
        "enum E { A, B }\n",
        "enum E { A, X, B }\n",
        "enum E { A, Y, B }\n",
    );
    assert!(!run.is_clean(), "enum constants must not be set-merged");
    assert_eq!(run.conflicts()[0].0, "ordered_insert_collision");
}

/// A reorder on one side and an edit to a reordered element on the other are
/// independent: order from the reorderer, content from the editor.
#[test]
fn a_reorder_and_an_edit_to_the_reordered_element_both_apply() {
    let run = Run::java(
        "class C {\n  void a() {\n    x();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
        "class C {\n  void b() {\n    y();\n  }\n\n  void a() {\n    x();\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    z();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    insta::assert_snapshot!(run.render());
}

/// Two different permutations of the same ordered region cannot be reconciled.
#[test]
fn two_different_reorders_conflict() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n    r();\n  }\n}\n",
        "class C {\n  void a() {\n    q();\n    p();\n    r();\n  }\n}\n",
        "class C {\n  void a() {\n    r();\n    q();\n    p();\n  }\n}\n",
    );
    assert!(!run.is_clean());
}

/// A parameter list is positional; adding a parameter on each side is not a
/// set union.
#[test]
fn formal_parameters_are_ordered() {
    let run = Run::java(
        "class C { void a(int p) {} }\n",
        "class C { void a(int p, int ours) {} }\n",
        "class C { void a(int p, int theirs) {} }\n",
    );
    assert!(!run.is_clean(), "parameter order is the calling convention");
}

// ---------------------------------------------------------------- unordered

/// The headline case: two branches adding different members to one class.
#[test]
fn disjoint_member_additions_both_apply() {
    let run = Run::java(
        "class C {\n  void a() {}\n}\n",
        "class C {\n  void a() {}\n\n  void b() {}\n}\n",
        "class C {\n  void a() {}\n\n  void c() {}\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    assert_eq!(run.outcome.stats.set_merged_regions, 1);
    insta::assert_snapshot!(run.render());
}

/// Both branches adding the *same* member is one member. Each side also adds a
/// member of its own, so the class body has to be aligned rather than converged.
#[test]
fn identical_member_additions_deduplicate() {
    let run = Run::java(
        "class C {\n  void a() {}\n}\n",
        "class C {\n  void a() {}\n\n  void shared() { s(); }\n\n  void ourOwn() { o(); }\n}\n",
        "class C {\n  void a() {}\n\n  void shared() { s(); }\n\n  void theirOwn() { t(); }\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    assert_eq!(run.outcome.stats.deduplicated_insertions, 1);
    insta::assert_snapshot!(run.render());
}

/// A set merge must not launder a delete/modify past the check either.
#[test]
fn a_set_merge_still_conflicts_on_delete_versus_modify() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n}\n",
        "class C {\n  void n() {}\n}\n",
        "class C {\n  void a() { x(); z(); }\n  void m() {}\n}\n",
    );
    assert!(!run.is_clean(), "deleting an edited member must conflict");
}

/// TypeScript's `class_body` is unordered for the same reason Java's is.
#[test]
fn typescript_class_members_commute() {
    let run = Run::typescript(
        "class C {\n  a() {}\n}\n",
        "class C {\n  a() {}\n  b() {}\n}\n",
        "class C {\n  a() {}\n  c() {}\n}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
}

// ------------------------------------------------------- commutative *groups*

/// The spec's headline false conflict: two branches adding an import.
#[test]
fn both_branches_adding_an_import_merges_cleanly() {
    let run = Run::java(
        "import java.util.List;\n\nclass C {}\n",
        "import java.util.List;\nimport java.util.Map;\n\nclass C {}\n",
        "import java.util.List;\nimport java.util.Set;\n\nclass C {}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    insta::assert_snapshot!(run.render());
}

/// Imports commute *among imports*. A region that also contains the class
/// declaration is not a commutative group, so nothing can float past it.
///
/// docs/prior-art.md §2.4: a flat "top level is a set" model would let an
/// import be emitted after a class.
#[test]
fn an_import_cannot_commute_past_a_type_declaration() {
    let run = Run::java(
        "package p;\n\nimport a.B;\n\nclass C {}\n",
        "package p;\n\nimport a.B;\nimport a.D;\n\nclass C { int ours; }\n",
        "package p;\n\nimport a.B;\nimport a.E;\n\nclass C { int theirs; }\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    assert_eq!(
        run.root_child_kinds(),
        vec![
            "package_declaration",
            "import_declaration",
            "import_declaration",
            "import_declaration",
            "class_declaration",
        ],
        "the class must stay last: it is not in the imports' commutative group"
    );
    insta::assert_snapshot!(run.render());
}

/// The package declaration is positionally fixed relative to the import group.
#[test]
fn the_package_declaration_stays_first() {
    let run = Run::java(
        "package p;\nimport a.B;\nclass C {}\n",
        "package p;\nimport a.B;\nimport a.D;\nclass C {}\n",
        "package p;\nimport a.B;\nimport a.E;\nclass C {}\n",
    );
    assert!(run.is_clean());
    assert_eq!(
        run.root_child_kinds().first().map(String::as_str),
        Some("package_declaration")
    );
}

// -------------------------------------------------------------- atomic sets

/// The handoff note from the matcher: its recovery pass will pair
/// `import java.util.HashMap;` with `import java.util.Optional;` because they
/// are the same shape. Reading that as an *update* is a semantically wrong
/// merge, so imports are keyed by text instead.
#[test]
fn imports_are_keyed_by_text_not_by_the_matchers_pairing() {
    let run = Run::java(
        "import java.util.HashMap;\n\nclass C {}\n",
        "import java.util.Optional;\n\nclass C {}\n",
        "import java.util.HashMap;\nimport java.util.List;\n\nclass C {}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
    let render = run.render();
    assert!(
        render.contains("import java.util.Optional;"),
        "our replacement import must survive:\n{render}"
    );
    assert!(
        render.contains("import java.util.List;"),
        "their addition must survive:\n{render}"
    );
    assert!(
        !render.contains("import java.util.HashMap;"),
        "the import we deleted must not come back:\n{render}"
    );
}

/// Whitespace is not part of an import's identity, so reindenting one is not a
/// different import.
#[test]
fn an_import_reindented_on_one_side_is_the_same_import() {
    let run = Run::java(
        "import  a.B ;\n\nclass C {}\n",
        "import a.B;\n\nclass C {}\n",
        "import  a.B ;\nimport a.D;\n\nclass C {}\n",
    );
    assert!(run.is_clean(), "{:?}", run.conflicts());
}

/// The atomic-kind list is configuration, and turning it off changes the
/// answer — which is what makes it worth having.
#[test]
fn atomic_keying_is_configurable() {
    let cfg = MergeConfig {
        atomic_set_kinds: Vec::new(),
        ..MergeConfig::default()
    };
    let base = parse("import java.util.HashMap;\n\nclass C {}\n", java());
    let ours = parse("import java.util.Optional;\n\nclass C {}\n", java());
    let theirs = parse(
        "import java.util.HashMap;\nimport java.util.List;\n\nclass C {}\n",
        java(),
    );
    let outcome = sm_merge::merge(&base, &ours, &theirs, java(), &cfg);
    // Without text keying the matcher's HashMap↔Optional pair is believed, and
    // the merge treats a replacement as an in-place update of one import.
    assert!(outcome.is_clean());
    assert_eq!(outcome.stats.set_merged_regions, 0);
}
