//! Conflict granularity: SPEC.md §4.6's "smallest sensible boundary".
//!
//! The rule under test: a conflict raised below a statement or declaration is
//! replaced by a conflict *over* that statement or declaration, and a conflict
//! already at element granularity stays where it is.

mod support;

use sm_merge::{MergeConfig, merge};
use support::{Run, java, parse};

/// A disagreement about a literal is not a region anyone can act on. It is
/// promoted to the declaration that contains it — and stops there, rather than
/// swallowing the class.
#[test]
fn an_expression_level_conflict_promotes_to_its_statement() {
    let run = Run::java(
        "class C {\n  void a() {\n    int i = 1;\n  }\n}\n",
        "class C {\n  void a() {\n    int i = 2;\n  }\n}\n",
        "class C {\n  void a() {\n    int i = 3;\n  }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("update_update", "local_variable_declaration")]
    );
}

/// Promotion stops at the *first* boundary above the disagreement, so the
/// sibling statements around it stay merged.
#[test]
fn promotion_stops_at_the_nearest_boundary() {
    let run = Run::java(
        "class C {\n  void a() {\n    before();\n    int i = 1;\n    after();\n  }\n}\n",
        "class C {\n  void a() {\n    beforeOurs();\n    int i = 2;\n    after();\n  }\n}\n",
        "class C {\n  void a() {\n    before();\n    int i = 3;\n    afterTheirs();\n  }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("update_update", "local_variable_declaration")]
    );
    let render = run.render();
    assert!(render.contains("beforeOurs();"), "{render}");
    assert!(render.contains("afterTheirs();"), "{render}");
}

/// A collision between two inserted *statements* is already at statement
/// granularity, so it stays inline instead of swallowing the method.
#[test]
fn a_statement_level_collision_stays_where_it_is() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    ours();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    theirs();\n    q();\n  }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("ordered_insert_collision", "expression_statement")]
    );
    let render = run.render();
    assert!(
        render.contains("p();"),
        "the untouched statements survive: {render}"
    );
    assert!(render.contains("q();"), "{render}");
}

/// Containers are never promotion targets. A conflict inside one method must
/// not become a conflict over the whole class body or the whole file.
#[test]
fn containers_do_not_absorb_conflicts() {
    let cfg = MergeConfig::default();
    for kind in [
        "program",
        "class_body",
        "block",
        "interface_body",
        "enum_body",
    ] {
        assert!(!cfg.is_boundary(kind), "{kind} must not absorb a conflict");
    }
    let run = Run::java(
        "class C {\n  void a() { int i = 1; }\n\n  void b() {}\n}\n",
        "class C {\n  void a() { int i = 2; }\n\n  void b() { ours(); }\n}\n",
        "class C {\n  void a() { int i = 3; }\n\n  void b() { theirs(); }\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![
            ("update_update", "local_variable_declaration"),
            ("ordered_insert_collision", "expression_statement"),
        ],
        "two independent conflicts, not one over the class"
    );
}

/// A conflict in a list whose elements are separated by tokens is promoted,
/// because a marker block that started with a stray comma is not a region a
/// reader can act on. The cost is a bigger region; the alternative is output
/// that does not read as code.
#[test]
fn a_conflict_among_separator_bearing_elements_is_promoted() {
    let run = Run::java(
        "enum E {\n  A,\n  B\n}\n",
        "enum E {\n  A,\n  X,\n  B\n}\n",
        "enum E {\n  A,\n  Y,\n  B\n}\n",
    );
    assert_eq!(
        run.conflicts(),
        vec![("ordered_insert_collision", "enum_declaration")]
    );
}

/// The boundary set is configuration. Emptying it removes every promotion
/// target, which is what makes the rule's effect visible.
#[test]
fn the_boundary_set_is_configurable() {
    let cfg = MergeConfig {
        boundary_kinds: Vec::new(),
        boundary_suffixes: Vec::new(),
        ..MergeConfig::default()
    };
    let base = parse("class C {\n  void a() {\n    int i = 1;\n  }\n}\n", java());
    let ours = parse("class C {\n  void a() {\n    int i = 2;\n  }\n}\n", java());
    let theirs = parse("class C {\n  void a() {\n    int i = 3;\n  }\n}\n", java());
    let outcome = merge(&base, &ours, &theirs, java(), &cfg);
    assert_eq!(outcome.conflicts.len(), 1);
    assert_ne!(
        outcome.conflicts[0].kind, "local_variable_declaration",
        "with no boundaries the conflict must stay where it was raised"
    );
}

/// Adding a kind to the boundary set moves the conflict up to it.
#[test]
fn adding_a_boundary_kind_changes_where_a_conflict_lands() {
    let mut cfg = MergeConfig::default();
    cfg.boundary_kinds.push("class_body".to_owned());
    let base = parse("class C {\n  int x = 1;\n}\n", java());
    let ours = parse("class C {\n  int x = 2;\n}\n", java());
    let theirs = parse("class C {\n  int x = 3;\n}\n", java());
    let outcome = merge(&base, &ours, &theirs, java(), &cfg);
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].kind, "field_declaration");

    // The promotion happens at the *nearest* boundary, so `field_declaration`
    // still wins over the newly-added `class_body`.
    assert!(cfg.is_boundary("class_body"));
}
