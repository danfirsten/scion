//! Reindentation, end to end (SPEC.md §4.6).
//!
//! The unit-level rules live in the `reindent` module's own tests. These pin
//! the behaviour on real merges, where the shift is *derived* rather than
//! supplied.

mod support;

use sm_emit::EmitOptions;
use support::{java, run, run_with, typescript};

/// The case SPEC.md §1 names: we wrap statements in `if (…) { }`, they edit one
/// of the statements. Both apply, and the result is indented for its new depth.
#[test]
fn a_wrapped_block_reindents_the_other_sides_edit() {
    let m = run(
        "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n",
        "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n",
        java(),
    );
    assert!(m.is_clean(), "{:?}", m.reasons());
    assert!(m.result.reindented_lines > 0);
    insta::assert_snapshot!(m.text());
}

/// Two levels deep, to prove the shift is derived from the observed
/// indentation rather than from a guessed "one indent unit".
#[test]
fn a_doubly_wrapped_block_shifts_by_the_observed_amount() {
    let m = run(
        "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n",
        "class C {\n  void a() {\n    if (p) {\n      if (q) {\n        x();\n        y();\n      }\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n",
        java(),
    );
    assert!(m.is_clean(), "{:?}", m.reasons());
    insta::assert_snapshot!(m.text());
}

/// Tabs are not special-cased: the shift is whatever whitespace the two lines
/// actually carry.
#[test]
fn tab_indented_files_reindent_with_tabs() {
    let m = run(
        "class C {\n\tvoid a() {\n\t\tx();\n\t\ty();\n\t}\n}\n",
        "class C {\n\tvoid a() {\n\t\tif (ok) {\n\t\t\tx();\n\t\t\ty();\n\t\t}\n\t}\n}\n",
        "class C {\n\tvoid a() {\n\t\tx();\n\t\ty2();\n\t}\n}\n",
        java(),
    );
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert!(text.contains("\n\t\t\ty2();\n"), "{text:?}");
    assert!(!text.contains("    "), "no spaces may appear:\n{text:?}");
}

/// A Java text block's interior whitespace is part of the value. Nothing inside
/// one is ever reindented, however far the surrounding code moved.
#[test]
fn a_text_block_interior_is_never_touched() {
    let base = "class C {\n  void a() {\n    String s = \"\"\"\n        keep\n          me\n        \"\"\";\n  }\n}\n";
    let ours = "class C {\n  void a() {\n    if (ok) {\n      String s = \"\"\"\n        keep\n          me\n        \"\"\";\n    }\n  }\n}\n";
    let theirs = "class C {\n  void a() {\n    String s = \"\"\"\n        keep\n          me\n        \"\"\";\n    after();\n  }\n}\n";
    let m = run(base, ours, theirs, java());
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert!(
        text.contains("\n        keep\n          me\n        \"\"\";"),
        "the text block interior moved:\n{text}"
    );
    assert!(
        text.contains("\n      after();"),
        "their statement should have been reindented:\n{text}"
    );
    assert!(!m.reparse().has_errors());
    insta::assert_snapshot!(text);
}

/// The same guarantee for TypeScript template literals.
#[test]
fn a_template_literal_interior_is_never_touched() {
    let base = "function f() {\n  const s = `line one\nline two`;\n}\n";
    let ours = "function f() {\n  if (ok) {\n    const s = `line one\nline two`;\n  }\n}\n";
    let theirs = "function f() {\n  const s = `line one\nline two`;\n  after();\n}\n";
    let m = run(base, ours, theirs, typescript());
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert!(
        text.contains("`line one\nline two`"),
        "the template literal moved:\n{text}"
    );
}

/// A block comment *is* reindented: its interior whitespace is not part of any
/// value, and a Javadoc left at its old depth reads as broken.
#[test]
fn a_block_comment_interior_is_reindented() {
    let base = "class C {\n  void a() {\n    /* one\n       two */\n    x();\n  }\n}\n";
    let ours = "class C {\n  void a() {\n    if (ok) {\n      /* one\n         two */\n      x();\n    }\n  }\n}\n";
    let theirs = "class C {\n  void a() {\n    /* one\n       two */\n    x2();\n  }\n}\n";
    let m = run(base, ours, theirs, java());
    assert!(m.is_clean(), "{:?}", m.reasons());
    insta::assert_snapshot!(m.text());
}

/// Turning the transform off leaves output a pure byte splice — useful for
/// isolating the reindenter, and for anyone who would rather have wrong
/// indentation than a rewritten byte.
#[test]
fn reindentation_can_be_switched_off() {
    let base = "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n";
    let ours = "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n";
    let theirs = "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n";
    let m = run_with(
        base,
        ours,
        theirs,
        java(),
        &EmitOptions {
            reindent: false,
            ..EmitOptions::default()
        },
    );
    assert_eq!(m.result.reindented_lines, 0);
    insta::assert_snapshot!(m.text());
}

/// Nothing is reindented when nothing moved — which is most merges, and it is
/// the guarantee that "never reformat untouched code" rests on.
#[test]
fn an_ordinary_merge_reindents_nothing() {
    for sc in support::scenarios() {
        if sc.name.contains("wrapped") || sc.name.contains("text_block") {
            continue;
        }
        let m = support::run_scenario(&sc);
        assert_eq!(
            m.result.reindented_lines,
            0,
            "{}: nothing moved, so nothing may be reindented:\n{}",
            sc.name,
            m.text()
        );
    }
}

/// A CRLF file's line endings survive the transform: only the leading
/// whitespace run of a line is ever rewritten, and `\r` sits at the *end* of
/// the previous line.
#[test]
fn reindenting_a_crlf_file_keeps_crlf() {
    let base = support::crlf("class C {\n  void a() {\n    x();\n    y();\n  }\n}\n");
    let ours = support::crlf(
        "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n",
    );
    let theirs = support::crlf("class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n");
    let m = support::run_bytes(&base, &ours, &theirs, java(), &EmitOptions::default());
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert_eq!(
        text.matches('\n').count(),
        text.matches("\r\n").count(),
        "a bare LF appeared:\n{text:?}"
    );
    assert!(text.contains("\r\n      y2();\r\n"), "{text:?}");
}

/// A line shallower than the node's own base indentation — a continuation line
/// aligned under an opening paren, say — is left alone rather than guessed at.
#[test]
fn a_line_shallower_than_the_shift_is_left_alone() {
    let base = "class C {\n  void a() {\n    call(one,\ntwo);\n  }\n}\n";
    let ours = "class C {\n  void a() {\n    if (ok) {\n      call(one,\ntwo);\n    }\n  }\n}\n";
    let theirs = "class C {\n  void a() {\n    call(one,\ntwo);\n    after();\n  }\n}\n";
    let m = run(base, ours, theirs, java());
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert!(
        text.contains("\ntwo);"),
        "the flush-left line moved:\n{text}"
    );
}
